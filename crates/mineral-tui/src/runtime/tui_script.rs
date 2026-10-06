//! Local tui.lua loading and callback effects on the UI thread.
//!
//! Each successful load replaces this App's config and Lua VM. File evaluation,
//! setup and config overrides commit together; failures keep the previous runtime.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crate::config::TuiConfig;
use mineral_config::ConfigWarning;
use mineral_script::{ConfigOverrideOp, CopyTemplateCtx, TuiCommand, TuiRuntime, WatchdogConfig};

use crate::app::App;
use crate::components::toast::card::{plain_body, plain_line};
use crate::components::toast::notifications::TextTint;
use crate::components::toast::push::{apply_event, tint_of};

/// Minimum interval between local file checks.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Reload failure card, dismissed after a successful load or file deletion.
pub(crate) const ERROR_CARD_ID: &str = "tui.script.error";

/// Config, callbacks and file polling owned by one TUI instance.
#[derive(Default)]
pub(crate) struct TuiScript {
    /// Last successful local file tree, before session overrides.
    base: Option<serde_json::Value>,

    /// Local overrides accumulated by setup and copy callbacks.
    overrides: Vec<ConfigOverrideOp>,

    /// Local callback VM; deleting tui.lua drops its callbacks.
    runtime: Option<TuiRuntime>,

    /// Full window title override; None uses the local config's title template.
    pub(crate) title: Option<String>,

    /// File polling enabled only by startup or an explicit test entry.
    entry: Option<TuiEntry>,
}

/// A single startup evaluation, kept until the terminal and App are ready.
pub(crate) struct TuiStartup {
    /// Effective local settings used to construct TUI resources.
    pub(crate) config: Arc<TuiConfig>,

    /// The same VM and file version used to produce the startup settings.
    script: TuiScript,

    /// Validated setup effects, applied when the App is ready.
    commands: Vec<TuiCommand>,

    /// User-file failure to report after the notification layer starts.
    error: Option<ReloadError>,
}

/// A fully validated replacement, before committing to the running TUI.
struct PreparedTui {
    /// Local file configuration without session overrides.
    base: serde_json::Value,

    /// Runtime retaining local copy callbacks.
    runtime: Option<TuiRuntime>,

    /// Validated local session overrides.
    overrides: Vec<ConfigOverrideOp>,

    /// File and overrides after TUI-only schema validation.
    config: TuiConfig,

    /// Successful setup effects in invocation order.
    commands: Vec<TuiCommand>,
}

/// Check progress for one local file, including failed versions.
struct TuiEntry {
    /// This TUI's tui.lua path.
    path: PathBuf,

    /// Last check time, limiting filesystem work.
    last_checked: Instant,

    /// Last observed version; None means not checked yet.
    observed: Option<FileVersion>,
}

/// File presence and modification time used by reload polling.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FileVersion {
    /// Deleting the file restores local defaults.
    Missing,

    /// Modification time of an existing file.
    Present(SystemTime),
}

/// Local loading or callback failure; details belong in logs, not UI text.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ReloadError {
    /// File metadata could not be inspected.
    #[error("inspect tui.lua")]
    Metadata(#[source] std::io::Error),

    /// File could not be read as UTF-8.
    #[error("read tui.lua")]
    Read(#[source] std::io::Error),

    /// Lua evaluation or setup failed; the new VM and effects are discarded.
    #[error("load tui.lua")]
    Load(#[source] crate::config::LoadError),

    /// Local callback or entity projection failed.
    #[error("execute tui.lua callback")]
    Script(#[source] mineral_script::Error),

    /// Local defaults could not be loaded.
    #[error("load TUI defaults")]
    Defaults(#[source] mineral_config::Error),

    /// Local overrides did not produce a valid TuiConfig.
    #[error("apply TUI configuration")]
    Config(#[source] ConfigWarning),

    /// Callback descriptors may only be replaced by loading callback definitions.
    #[error("TUI config override cannot replace callback descriptors: {path}")]
    CallbackConfig {
        /// Rejected callback descriptor path.
        path: String,
    },

    /// No local file has supplied a callback runtime.
    #[error("local copy template runtime is unavailable")]
    NoRuntime,
}

impl TuiStartup {
    /// Evaluate tui.lua once, before constructing resources that consume its settings.
    /// Invalid user settings leave local defaults active and retain the failure for the UI.
    pub(crate) fn load(path: PathBuf) -> mineral_config::Result<Self> {
        let defaults = Arc::new(TuiConfig::defaults()?);
        let mut script = TuiScript::watch(path);
        let (config, commands, error) = match script.prepare_reload(&defaults) {
            Ok(Some(prepared)) => {
                let (config, commands) = script.commit(prepared);
                (config, commands, None)
            }
            Ok(None) => (defaults, Vec::new(), None),
            Err(error) => (defaults, Vec::new(), Some(error)),
        };
        Ok(Self {
            config,
            script,
            commands,
            error,
        })
    }
}

impl TuiScript {
    /// Track a local entry; loading does not depend on a daemon connection.
    fn watch(path: PathBuf) -> Self {
        Self {
            entry: Some(TuiEntry {
                path,
                last_checked: Instant::now(),
                observed: None,
            }),
            ..Self::default()
        }
    }

    /// Observe one version and prepare a complete replacement without changing live effects.
    fn prepare_reload(&mut self, config: &TuiConfig) -> Result<Option<PreparedTui>, ReloadError> {
        let Some(entry) = self.entry.as_mut() else {
            return Ok(None);
        };
        let version = match std::fs::metadata(&entry.path) {
            Ok(metadata) => {
                FileVersion::Present(metadata.modified().map_err(ReloadError::Metadata)?)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => FileVersion::Missing,
            Err(error) => return Err(ReloadError::Metadata(error)),
        };
        if entry.observed == Some(version) {
            return Ok(None);
        }
        entry.observed = Some(version);
        let (base, file_config, runtime, commands) = match version {
            FileVersion::Missing => {
                let base = crate::config::default_tui_tree().map_err(ReloadError::Defaults)?;
                let config = crate::config::tui_from_tree(&base).map_err(ReloadError::Config)?;
                (base, config, None, Vec::new())
            }
            FileVersion::Present(_) => {
                let source = std::fs::read_to_string(&entry.path).map_err(ReloadError::Read)?;
                let load = crate::config::evaluate_tui(
                    &source,
                    &entry.path.to_string_lossy(),
                    WatchdogConfig::from(config.script()),
                )
                .map_err(ReloadError::Load)?;
                (load.tree, load.config, Some(load.runtime), load.commands)
            }
        };
        let overrides = merge_overrides(Vec::new(), &commands);
        let config = if overrides.is_empty() {
            file_config
        } else {
            compose_config(&base, &overrides)?
        };
        Ok(Some(PreparedTui {
            base,
            runtime,
            overrides,
            config,
            commands,
        }))
    }

    /// Install a validated configuration and its VM together, preserving the file watcher.
    fn commit(&mut self, prepared: PreparedTui) -> (Arc<TuiConfig>, Vec<TuiCommand>) {
        let PreparedTui {
            base,
            runtime,
            overrides,
            config,
            commands,
        } = prepared;
        self.base = Some(base);
        self.runtime = runtime;
        self.overrides = overrides;
        self.title = None;
        mineral_log::info!(
            target: "tui.script",
            path = ?self.entry.as_ref().map(|entry| &entry.path),
            removed = self.runtime.is_none(),
            overrides = self.overrides.len(),
            "tui.lua local settings and callbacks applied"
        );
        (Arc::new(config), commands)
    }

    /// Apply the current effective budget without resetting copy callback closures.
    pub(super) fn apply_config(&mut self, config: &crate::config::TuiScriptConfig) {
        if let Some(runtime) = self.runtime.as_mut() {
            runtime.set_watchdog(WatchdogConfig::from(config));
        }
    }
}

impl App {
    /// Finish startup with the configuration and VM already evaluated before resource creation.
    pub(crate) fn apply_tui_startup(&mut self, startup: TuiStartup) {
        self.tui_script = startup.script;
        self.apply_config(startup.config);
        self.commit_tui_ui_commands(startup.commands);
        let path = self
            .tui_script
            .entry
            .as_ref()
            .map(|entry| entry.path.clone());
        self.notify_startup_config(path.as_deref(), &[]);
        if let Some(error) = startup.error {
            self.report_tui_script_failure(&error);
        }
    }

    /// Enable file loading explicitly in App tests, without accessing a user's config directory.
    #[cfg(test)]
    pub(crate) fn start_tui_script(&mut self, path: PathBuf) {
        self.tui_script = TuiScript::watch(path);
        self.reload_tui_script();
    }

    /// Check local file changes on the UI thread; no Lua worker thread is needed.
    pub(crate) fn poll_tui_script(&mut self, now: Instant) {
        let Some(entry) = self.tui_script.entry.as_mut() else {
            return;
        };
        if now.duration_since(entry.last_checked) < POLL_INTERVAL {
            return;
        }
        entry.last_checked = now;
        self.reload_tui_script();
    }

    /// Commit successful reloads, keeping the prior runtime and effects on any failure.
    fn reload_tui_script(&mut self) {
        match self.tui_script.prepare_reload(&self.state.cfg) {
            Ok(Some(prepared)) => {
                let (config, commands) = self.tui_script.commit(prepared);
                self.apply_config(config);
                self.commit_tui_ui_commands(commands);
                self.notifications.dismiss_card_by_id(ERROR_CARD_ID);
            }
            Ok(None) => {}
            Err(error) => self.report_tui_script_failure(&error),
        }
    }

    /// Keep internal diagnostics in logs and generate the failure card in the frontend.
    fn report_tui_script_failure(&mut self, error: &ReloadError) {
        mineral_log::warn!(
            target: "tui.script",
            path = ?self.tui_script.entry.as_ref().map(|entry| &entry.path),
            error = mineral_log::chain(error),
            "tui.lua reload failed; keeping current local settings"
        );
        self.notifications.push_card(
            TextTint::Error,
            plain_line("Could not apply tui.lua"),
            plain_body(["Keeping current TUI settings; see logs for details".to_owned()]),
            Some(ERROR_CARD_ID.to_owned()),
            None,
        );
    }

    /// Render locally and validate all callback overrides before committing any effects.
    /// The caller may write the returned text to the clipboard only after success.
    pub(crate) fn render_local_copy_template(
        &mut self,
        index: usize,
        ctx: CopyTemplateCtx,
    ) -> Result<String, ReloadError> {
        let runtime = self
            .tui_script
            .runtime
            .as_ref()
            .ok_or(ReloadError::NoRuntime)?;
        runtime
            .seed_web_url_templates(self.source_web_urls())
            .map_err(ReloadError::Script)?;
        let (text, commands) = runtime
            .render_copy_template(index, ctx)
            .map_err(ReloadError::Script)?;
        let base = self
            .tui_script
            .base
            .as_ref()
            .ok_or(ReloadError::NoRuntime)?;
        let overrides = merge_overrides(self.tui_script.overrides.clone(), &commands);
        let effective = compose_config(base, &overrides)?;
        if overrides != self.tui_script.overrides {
            self.tui_script.overrides = overrides;
            self.apply_config(Arc::new(effective));
        }
        self.commit_tui_ui_commands(commands);
        mineral_log::info!(target: "tui.script", index, "local copy template rendered");
        Ok(text)
    }

    /// Translate known daemon capabilities into local Lua model projection URLs.
    fn source_web_urls(&self) -> Vec<mineral_script::SourceWebUrls> {
        self.state
            .models
            .caps
            .iter()
            .map(|(source, caps)| mineral_script::SourceWebUrls {
                source: source.name().to_owned(),
                song: caps.song_web_url().clone(),
                playlist: caps.playlist_web_url().clone(),
                album: caps.album_web_url().clone(),
                artist: caps.artist_web_url().clone(),
            })
            .collect()
    }

    /// Apply UI effects only after Lua execution and local config validation succeed.
    fn commit_tui_ui_commands(&mut self, commands: Vec<TuiCommand>) {
        for command in commands {
            match command {
                TuiCommand::Toast {
                    kind,
                    content,
                    id,
                    ttl_secs,
                } => apply_event(
                    &mut self.notifications,
                    mineral_protocol::Event::Toast {
                        kind,
                        content,
                        id,
                        ttl_secs,
                    },
                ),
                TuiCommand::Card {
                    kind,
                    title,
                    body,
                    id,
                    ttl_secs,
                } => {
                    self.notifications.push_card(
                        tint_of(kind),
                        title,
                        body,
                        id,
                        ttl_secs.map(Duration::from_secs),
                    );
                }
                TuiCommand::WindowTitle { text } => self.tui_script.title = text,
                TuiCommand::ConfigOverride { .. } => {}
            }
        }
    }
}

/// Merge commands into local session overrides; nil removes an earlier override.
fn merge_overrides(
    mut overrides: Vec<ConfigOverrideOp>,
    commands: &[TuiCommand],
) -> Vec<ConfigOverrideOp> {
    for command in commands {
        if let TuiCommand::ConfigOverride { ops } = command {
            for op in ops {
                if op.value.is_none() {
                    overrides.retain(|existing| existing.path != op.path);
                } else if let Some(existing) = overrides
                    .iter_mut()
                    .find(|existing| existing.path == op.path)
                {
                    *existing = op.clone();
                } else {
                    overrides.push(op.clone());
                }
            }
        }
    }
    overrides
}

/// Compose local file and local session overrides, then validate the TUI-only schema.
fn compose_config(
    base: &serde_json::Value,
    overrides: &[ConfigOverrideOp],
) -> Result<TuiConfig, ReloadError> {
    let mut effective = base.clone();
    for ConfigOverrideOp { path, value } in overrides {
        if let Some(value) = value {
            let patch = mineral_config::nest_path(path, value.clone().into_json());
            if patch.pointer("/copy/templates").is_some() {
                return Err(ReloadError::CallbackConfig {
                    path: "copy.templates".to_owned(),
                });
            }
            effective = mineral_config::merge_tree(effective, patch);
        }
    }
    crate::config::tui_from_tree(&effective).map_err(ReloadError::Config)
}

//! Explicit login using masked terminal input or a piped token.

use std::io::{self, IsTerminal};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Subcommand};
use indicatif::{ProgressBar, ProgressFinish};
use inquire::{InquireError, Password, PasswordDisplayMode};

use crate::api::Api;
use crate::credential::{StoredAppleMusicAuth, from_token, save_login};
use crate::{AppleMusicConfig, Error, Result};

/// The website in the input help uses an OSC 8 hyperlink.
const TOKEN_INPUT_HELP: &str = concat!(
    "Sign in at [",
    "\x1b]8;;https://music.apple.com\x1b\\",
    "music.apple.com",
    "\x1b]8;;\x1b\\",
    "], then copy the media-user-token cookie value from browser DevTools.",
);

/// Apple Music account commands.
#[derive(Debug, Args)]
pub struct AppleMusicCli {
    /// Source operation.
    #[command(subcommand)]
    pub command: AppleMusicCommand,
}

/// Account operations; tokens are never accepted as command-line arguments.
#[derive(Debug, Subcommand)]
pub enum AppleMusicCommand {
    /// Read a media-user-token from masked terminal input or piped stdin.
    Login,
}

/// Reports login failures without a diagnostic backtrace; cancellation exits with 130.
pub async fn run(cli: AppleMusicCli, config: &AppleMusicConfig) -> ExitCode {
    let result = match cli.command {
        AppleMusicCommand::Login => login(config).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(Error::Cancelled) => {
            eprintln!("Login cancelled.");
            ExitCode::from(130)
        }
        Err(Error::Network(source)) => {
            mineral_log::warn!(
                target: "apple_music",
                error = mineral_log::chain(&source),
                "login request failed"
            );
            if source.is_timeout() {
                eprintln!("Apple Music request timed out.");
            } else {
                eprintln!("Apple Music request failed.");
            }
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("Login failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn login(config: &AppleMusicConfig) -> Result<()> {
    let interactive = io::stdin().is_terminal();
    loop {
        let token = tokio::task::spawn_blocking(move || read_token(interactive)).await??;
        match validate_with_spinner(config, &token, interactive).await {
            Ok(auth) => {
                let path = tokio::select! {
                    result = save_login(auth) => result?,
                    signal = tokio::signal::ctrl_c() => {
                        signal?;
                        return Err(Error::Cancelled);
                    }
                };
                println!(
                    "Apple Music login saved to {}. Restart the daemon to load it.",
                    path.display()
                );
                return Ok(());
            }
            Err(error @ (Error::InvalidTokenValue | Error::LoginRejected)) if interactive => {
                eprintln!("{error}\n");
            }
            Err(error) => return Err(error),
        }
    }
}

/// The password prompt has released raw mode before Tokio handles cancellation here.
async fn validate_with_spinner(
    config: &AppleMusicConfig,
    token: &str,
    interactive: bool,
) -> Result<StoredAppleMusicAuth> {
    let spinner = if interactive {
        ProgressBar::new_spinner()
    } else {
        ProgressBar::hidden()
    }
    .with_message("Validating Apple Music account, it could take a while…")
    .with_finish(ProgressFinish::AndClear);
    if interactive {
        spinner.enable_steady_tick(Duration::from_millis(100));
    }
    let result = tokio::select! {
        result = validate_token(config, token) => result,
        signal = tokio::signal::ctrl_c() => {
            signal?;
            Err(Error::Cancelled)
        }
    };
    spinner.finish_and_clear();
    result
}

/// Returns credentials only after both local format checks and Apple account validation succeed.
async fn validate_token(config: &AppleMusicConfig, token: &str) -> Result<StoredAppleMusicAuth> {
    let auth = from_token(token)?;
    Api::new(config, auth.clone())?
        .session()
        .await
        .map_err(|error| match error {
            Error::AuthRequired => Error::LoginRejected,
            other => other,
        })?;
    Ok(auth)
}

fn read_token(interactive: bool) -> Result<String> {
    if interactive {
        Password::new("Apple Music media-user-token:")
            .with_display_mode(PasswordDisplayMode::Masked)
            .without_confirmation()
            .with_help_message(TOKEN_INPUT_HELP)
            .prompt()
            .map_err(|error| match error {
                InquireError::OperationCanceled | InquireError::OperationInterrupted => {
                    Error::Cancelled
                }
                other => Error::Prompt(other),
            })
    } else {
        let mut token = String::new();
        io::stdin().read_line(&mut token)?;
        Ok(token)
    }
}

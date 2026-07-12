//! daemon 启动所需的配置切片([`ServerConfig`]),从全局 `Config` 派生;
//! 以及 env > config 的音频后端 resolve([`resolve_audio_mode`])。

use std::num::{NonZeroU32, NonZeroUsize};

use mineral_audio::{AudioMode, EngineParams, EnvelopeParams, HighpassParams, ShelfParams};
use mineral_config::{BackendKind, DaemonConfig, DownloadConfig};
use mineral_model::BitRate;

/// daemon 启动配置切片。私有字段 + getter 读取;
#[non_exhaustive]
#[derive(Clone, Debug, typed_builder::TypedBuilder, derive_getters::Getters)]
pub struct ServerConfig {
    /// 音频引擎启动参数(初始音量 / tick / prefetch / tap 容量)。
    engine: EngineParams,

    /// 响度包络计算参数(配置 `audio.envelope`)。
    envelope: EnvelopeParams,

    /// 在线播放音质(独立于下载音质)。
    playback_quality: BitRate,

    /// 音频本体缓存容量上限(字节)。
    audio_cache_capacity: u64,

    /// 每 channel 任务 worker 数。
    channel_workers_per: usize,

    /// 下载段(音质 + 目录)。
    download: DownloadConfig,

    /// daemon 段(gapless 窗口 + 各间隔节拍)。
    daemon: DaemonConfig,

    /// 同步拦截 hook 软超时(毫秒,配置 `script.hook_timeout_ms`)。
    hook_timeout_ms: u64,

    /// `mineral.spawn` 并发上限(配置 `script.spawn_max_concurrent`;0 = 不限)。
    spawn_max_concurrent: usize,

    /// 聚合收藏后台补 meta:单次 `songs_detail` 批量数(配置 `sources.mineral.backfill.chunk_size`)。
    favorites_backfill_chunk_size: usize,

    /// 聚合收藏后台补 meta:并行拉取路数上限(配置 `sources.mineral.backfill.max_concurrent`)。
    favorites_backfill_max_concurrent: usize,

    /// shelf 扫描根列表(配置 `sources.shelf.roots`;空 = shelf 不激活、不扫)。
    shelf_roots: Vec<String>,

    /// shelf 遍历深度上限(配置 `sources.shelf.scan.max_depth`)。
    shelf_max_depth: usize,

    /// shelf 名称排除 regex 原文(配置 `sources.shelf.scan.exclude`)。
    shelf_exclude: Vec<String>,
}

impl ServerConfig {
    /// 从全局配置派生 daemon 切片(唯一生产构造入口)。
    ///
    /// # Params:
    ///   - `cfg`: 已加载的全局配置
    ///
    /// # Return:
    ///   daemon 启动切片。
    pub fn from_config(cfg: &mineral_config::Config) -> Self {
        let audio = cfg.audio();
        Self::builder()
            .engine(
                EngineParams::builder()
                    .initial_volume(*audio.volume())
                    .tick_ms(*audio.engine_tick_ms())
                    .prefetch_bytes(*audio.prefetch_bytes())
                    .tap_capacity(*audio.tap_capacity())
                    .build(),
            )
            .envelope(envelope_params_from(audio.envelope()))
            .playback_quality(*audio.playback_quality())
            .audio_cache_capacity(*cfg.cache().audio_capacity())
            .channel_workers_per(*cfg.daemon().channel_workers_per())
            .download(cfg.download().clone())
            .daemon(cfg.daemon().clone())
            .hook_timeout_ms(*cfg.script().hook_timeout_ms())
            .spawn_max_concurrent(*cfg.script().spawn_max_concurrent())
            .favorites_backfill_chunk_size(*cfg.sources().mineral().backfill().chunk_size())
            .favorites_backfill_max_concurrent(*cfg.sources().mineral().backfill().max_concurrent())
            .shelf_roots(cfg.sources().shelf().roots().clone())
            .shelf_max_depth(*cfg.sources().shelf().scan().max_depth())
            .shelf_exclude(cfg.sources().shelf().scan().exclude().clone())
            .build()
    }
}

/// `audio.envelope` 配置 → 包络计算参数。粒度字段的 0 值无意义,一律兜底为 1
/// (类型层 NonZero 强制,坏配置不该让 daemon 起不来)。
///
/// # Params:
///   - `cfg`: 包络配置段
///
/// # Return:
///   计算参数切片。
fn envelope_params_from(cfg: &mineral_config::EnvelopeConfig) -> EnvelopeParams {
    EnvelopeParams::builder()
        .point_count(NonZeroUsize::new(*cfg.points()).unwrap_or(NonZeroUsize::MIN))
        .block_ms(NonZeroU32::new(*cfg.block_ms()).unwrap_or(NonZeroU32::MIN))
        .window_ms(NonZeroU32::new(*cfg.window_ms()).unwrap_or(NonZeroU32::MIN))
        .shelf(
            ShelfParams::builder()
                .f0_hz(*cfg.shelf().f0_hz())
                .gain_db(*cfg.shelf().gain_db())
                .q(*cfg.shelf().q())
                .band_exponent(*cfg.shelf().band_exponent())
                .build(),
        )
        .highpass(
            HighpassParams::builder()
                .f0_hz(*cfg.highpass().f0_hz())
                .q(*cfg.highpass().q())
                .build(),
        )
        .build()
}

/// env > config 的音频后端 resolve:`MINERAL_AUDIO_NULL` 命中短路 config。
/// env 在 binary 边缘读好后以 bool 传入,本函数保持纯(可单测)。
///
/// # Params:
///   - `env_null`: `MINERAL_AUDIO_NULL` env 是否存在
///   - `backend`: config 的 `audio.backend`
///
/// # Return:
///   最终 [`AudioMode`]。
pub fn resolve_audio_mode(env_null: bool, backend: BackendKind) -> AudioMode {
    if env_null {
        return AudioMode::ForceNull;
    }
    match backend {
        BackendKind::Null => AudioMode::ForceNull,
        // BackendKind 是 #[non_exhaustive]:未来新增后端在接线前一律按 Auto 兜底。
        BackendKind::Auto | _ => AudioMode::Auto,
    }
}

#[cfg(test)]
mod tests {
    use mineral_audio::AudioMode;
    use mineral_config::BackendKind;

    use super::{ServerConfig, resolve_audio_mode};

    /// 不写配置:default.lua(唯一默认真相源)→ 切片的映射整体钉死。
    /// lua 默认变更或映射接错任一字段都会让本快照变红。
    #[test]
    fn server_config_defaults_snapshot() -> color_eyre::Result<()> {
        let cfg = mineral_config::Config::defaults()?;
        mineral_test::assert_snap_debug!(
            "ServerConfig(default.lua → daemon 切片映射,行为不变守卫)",
            ServerConfig::from_config(&cfg)
        );
        Ok(())
    }

    /// env > config 短路矩阵:env 命中恒 ForceNull;否则按 backend 落。
    #[test]
    fn resolve_audio_mode_matrix() {
        assert_eq!(
            resolve_audio_mode(/*env_null*/ true, BackendKind::Auto),
            AudioMode::ForceNull
        );
        assert_eq!(
            resolve_audio_mode(/*env_null*/ true, BackendKind::Null),
            AudioMode::ForceNull
        );
        assert_eq!(
            resolve_audio_mode(/*env_null*/ false, BackendKind::Null),
            AudioMode::ForceNull
        );
        assert_eq!(
            resolve_audio_mode(/*env_null*/ false, BackendKind::Auto),
            AudioMode::Auto
        );
    }
}

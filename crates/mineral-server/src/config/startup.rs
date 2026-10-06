//! daemon 启动所需的配置切片([`ServerConfig`]),从 daemon 私有配置派生;
//! 以及 env > config 的音频后端 resolve([`resolve_audio_mode`])。

use std::num::{NonZeroU32, NonZeroUsize};

use crate::config::{BackendKind, DownloadConfig};
use mineral_audio::{AudioMode, EngineParams, EnvelopeParams, HighpassParams, ShelfParams};
use mineral_model::BitRate;

/// daemon 启动配置切片。私有字段 + getter 读取;
#[non_exhaustive]
#[derive(Clone, Debug, derive_getters::Getters)]
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

    /// 播放器与服务任务的时序参数，由 daemon.lua 根字段派生。
    #[getter(skip)]
    timing: ServerTiming,

    /// 同步拦截 hook 软超时(毫秒,配置 `script.hook_timeout_ms`)。
    hook_timeout_ms: u64,

    /// 聚合收藏后台补 meta:单次 `songs_detail` 批量数(配置 `sources.mineral.backfill.chunk_size`)。
    favorites_backfill_chunk_size: usize,

    /// 聚合收藏后台补 meta:并行拉取路数上限(配置 `sources.mineral.backfill.max_concurrent`)。
    favorites_backfill_max_concurrent: usize,
}

impl ServerConfig {
    /// 从 daemon 私有配置派生启动切片。
    ///
    /// # Params:
    ///   - `cfg`: 已校验的 daemon 配置
    ///
    /// # Return:
    ///   daemon 启动切片。
    pub fn from_config(cfg: &crate::config::DaemonConfig) -> Self {
        let audio = cfg.audio();
        Self {
            engine: EngineParams::builder()
                .initial_volume(*audio.volume())
                .tick_ms(*audio.engine_tick_ms())
                .prefetch_bytes(*audio.prefetch_bytes())
                .tap_capacity(*audio.tap_capacity())
                .build(),
            envelope: envelope_params_from(audio.envelope()),
            playback_quality: *audio.playback_quality(),
            audio_cache_capacity: *cfg.cache().audio_capacity(),
            channel_workers_per: *cfg.channel_workers_per(),
            download: cfg.download().clone(),
            timing: ServerTiming {
                gapless_prefetch_ms: *cfg.gapless_prefetch_ms(),
                prev_restart_threshold_ms: *cfg.prev_restart_threshold_ms(),
                player_tick_ms: *cfg.player_tick_ms(),
                session_save_secs: *cfg.session_save_secs(),
                heartbeat_secs: *cfg.heartbeat_secs(),
                report_interval_ms: *cfg.report_interval_ms(),
                seek_threshold_ms: *cfg.seek_threshold_ms(),
                download_speed_tick_ms: *cfg.download_speed_tick_ms(),
            },
            hook_timeout_ms: *cfg.script().hook_timeout_ms(),
            favorites_backfill_chunk_size: *cfg.sources().mineral().backfill().chunk_size(),
            favorites_backfill_max_concurrent: *cfg.sources().mineral().backfill().max_concurrent(),
        }
    }

    /// 返回后台任务启动时使用的节拍，不随配置文件重载重建任务。
    pub(crate) fn timing(&self) -> &ServerTiming {
        &self.timing
    }
}

/// daemon 启动后使用的播放与后台任务节拍；不参与 Lua 配置落型。
#[derive(Clone, Debug, derive_getters::Getters)]
pub(crate) struct ServerTiming {
    /// 曲尾预取下一首的提前量，毫秒。
    gapless_prefetch_ms: u64,

    /// 上一首操作分界，毫秒；进度超过此值则回当前曲开头。
    prev_restart_threshold_ms: u64,

    /// 播放器轮询间隔，毫秒。
    player_tick_ms: u64,

    /// 播放进度保存间隔，秒。
    session_save_secs: u64,

    /// daemon 状态心跳日志间隔，秒。
    heartbeat_secs: u64,

    /// 系统媒体控件进度上报间隔，毫秒。
    report_interval_ms: u64,

    /// 跳转识别阈值，毫秒。
    seek_threshold_ms: u64,

    /// 下载测速刷新周期，毫秒。
    download_speed_tick_ms: u64,
}

/// `audio.envelope` 配置 → 包络计算参数。粒度字段的 0 值无意义,一律兜底为 1
/// (类型层 NonZero 强制,坏配置不该让 daemon 起不来)。
///
/// # Params:
///   - `cfg`: 包络配置段
///
/// # Return:
///   计算参数切片。
fn envelope_params_from(cfg: &crate::config::EnvelopeConfig) -> EnvelopeParams {
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
        BackendKind::Auto => AudioMode::Auto,
    }
}

#[cfg(test)]
mod tests {
    use crate::config::BackendKind;
    use mineral_audio::AudioMode;

    use super::{ServerConfig, resolve_audio_mode};

    /// daemon-default.lua 到启动切片的映射；覆盖引擎、存储与运行节拍参数。
    #[test]
    fn server_config_defaults_snapshot() -> color_eyre::Result<()> {
        let cfg = crate::config::DaemonConfig::defaults()?;
        mineral_test::assert_snap_debug!(
            "ServerConfig(daemon-default.lua → daemon 启动切片)",
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

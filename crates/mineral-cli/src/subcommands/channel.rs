//! `mineral channel ...` 子命令分发。

use std::time::Duration;

use clap::{Args as ClapArgs, Subcommand};
use mineral_channel_apple_music::cli::AppleMusicCli;
use mineral_channel_apple_music::{
    AlbumCacheConfig as AppleMusicAlbumCacheConfig, AppleMusicConfig,
};
use mineral_channel_bilibili::BilibiliConfig;
use mineral_channel_bilibili::cli::BilibiliCli;
use mineral_channel_netease::NeteaseConfig;
use mineral_channel_netease::cli::NeteaseCli;
use mineral_channel_netease::config::{AlbumCacheConfig, PlaylistFetchConfig, RequestsConfig};

/// Arguments for the `channel` subcommand.
#[derive(Debug, ClapArgs)]
pub struct ChannelArgs {
    /// choose a source
    #[command(subcommand)]
    pub channel: ChannelCommand,
}

/// Supported music sources.
#[derive(Debug, Subcommand)]
pub enum ChannelCommand {
    /// Apple Music
    Am(AppleMusicCli),

    /// NetEase Cloud Music
    Netease(NeteaseCli),

    /// Bilibili
    Bilibili(BilibiliCli),
}

/// 执行 `mineral channel ...` 下的命令。
///
/// # Params:
///   - `args`: 已解析的 channel namespace 参数。
///
/// # Return:
///   命令执行结果。
pub async fn run(args: ChannelArgs) -> crate::error::Result<()> {
    match args.channel {
        ChannelCommand::Am(cli) => {
            let (config, _warnings) = super::config::load_daemon().await?;
            let config = apple_music_config_from(config.sources().am());
            mineral_channel_apple_music::cli::run(cli, &config)
                .await
                .map_err(Into::into)
        }
        ChannelCommand::Netease(cli) => {
            // 加载 daemon 的网络参数，不执行 setup；代理与超时对扫码登录同样生效。
            let (config, _warnings) = super::config::load_daemon().await?;
            let nc = netease_config_from(config.sources().netease());
            mineral_channel_netease::cli::run(cli, &nc)
                .await
                .map_err(Into::into)
        }
        ChannelCommand::Bilibili(cli) => {
            let (config, _warnings) = super::config::load_daemon().await?;
            let bc = bilibili_config_from(config.sources().bilibili());
            mineral_channel_bilibili::cli::run(cli, &bc)
                .await
                .map_err(Into::into)
        }
    }
}

/// Supplies daemon settings without opening the optional playback device file.
pub fn apple_music_config_from(
    section: &mineral_server::config::AppleMusicSection,
) -> AppleMusicConfig {
    AppleMusicConfig::builder()
        .proxy(section.proxy().clone())
        .timeout_secs(*section.timeout_secs())
        .developer_token_cache_ttl(Duration::from_secs(
            u64::from(section.developer_token_cache_ttl_days().get()) * 24 * 60 * 60,
        ))
        .lyrics_translation_language(section.lyrics_translation_language().clone())
        .lyrics_romanization_script(section.lyrics_romanization_script().clone())
        .album_cache(
            AppleMusicAlbumCacheConfig::builder()
                .ttl_days(*section.album_cache().ttl_days())
                .ttl_jitter_days(*section.album_cache().ttl_jitter_days())
                .build(),
        )
        .device_file(section.device_file().clone())
        .media_concurrency(*section.media_concurrency())
        .startup_buffer(Duration::from_secs(u64::from(
            section.startup_buffer_secs().get(),
        )))
        .fragment_header_prefetch_bytes(*section.fragment_header_prefetch_bytes())
        .build()
}

/// 把配置的网易云段映射成构造参数(`NeteaseConfig` 是叶子类型,不依赖配置 crate,
/// 在消费侧做一次显式映射;`mineral` 启动链同样走本函数)。
///
/// # Params:
///   - `section`: 配置的 `sources.netease` 段
///
/// # Return:
///   网易云构造参数。
pub fn netease_config_from(section: &mineral_server::config::NeteaseSection) -> NeteaseConfig {
    NeteaseConfig::builder()
        .album_cache(
            AlbumCacheConfig::builder()
                .ttl_days(*section.album_cache().ttl_days())
                .ttl_jitter_days(*section.album_cache().ttl_jitter_days())
                .build(),
        )
        .playlist_fetch(
            PlaylistFetchConfig::builder()
                .batch_size(*section.playlist_fetch().batch_size())
                .max_concurrent(*section.playlist_fetch().max_concurrent())
                .build(),
        )
        .requests(
            RequestsConfig::builder()
                .album_detail_requests_per_second(
                    *section.requests().album_detail_requests_per_second(),
                )
                .retry_delays(
                    section
                        .requests()
                        .retry_delays_ms()
                        .iter()
                        .map(|delay| Duration::from_millis(delay.get()))
                        .collect(),
                )
                .build(),
        )
        .max_connections(*section.max_connections())
        .proxy(section.proxy().clone())
        .timeout_secs(*section.timeout_secs())
        .build()
}

/// 把配置的 B站段映射成构造参数(同 [`netease_config_from`],消费侧一次显式映射)。
///
/// # Params:
///   - `section`: 配置的 `sources.bilibili` 段
///
/// # Return:
///   B站构造参数。
pub fn bilibili_config_from(section: &mineral_server::config::BilibiliSection) -> BilibiliConfig {
    BilibiliConfig::builder()
        .max_connections(*section.max_connections())
        .proxy(section.proxy().clone())
        .timeout_secs(*section.timeout_secs())
        .build()
}

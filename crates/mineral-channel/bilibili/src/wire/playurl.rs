//! 播放地址端点(`x/player/wbi/playurl`)的 DTO。
//!
//! `fnval=4048` 请求 DASH 格式;音频流在 `data.dash.audio[]`(每档一个 `id` 音质码 + `baseUrl`),
//! 大会员无损在 `data.dash.flac.audio`。取流走 `baseUrl`——**必须带 `Referer`**,否则 403。

use serde::Deserialize;

/// playurl 响应的 `data` 主体。
#[derive(Debug, Clone, Deserialize)]
pub struct PlayUrlResult {
    /// DASH 分离流(音视频分轨);MVP 只取音频轨。
    pub dash: Option<Dash>,
}

/// DASH 分离流容器。
#[derive(Debug, Clone, Deserialize)]
pub struct Dash {
    /// 音频轨候选(不同音质档,`id` 越大质量越高)。
    pub audio: Option<Vec<DashAudio>>,

    /// 无损(FLAC)音频轨(需大会员;未开通为 `None`)。
    pub flac: Option<DashFlac>,
}

/// 无损音频轨容器。
#[derive(Debug, Clone, Deserialize)]
pub struct DashFlac {
    /// 无损音频轨(`display=false` 时可能为 `None`)。
    pub audio: Option<DashAudio>,
}

/// 一条音频轨。
#[derive(Debug, Clone, Deserialize)]
pub struct DashAudio {
    /// 音质码(`30216`=64k/`30232`=132k/`30280`=192k/`30250`=Dolby/`30251`=FLAC)。
    pub id: i64,

    /// 取流直链。B站 web playurl 同一项里 `baseUrl` 与 `base_url` **两个键并存**(值相同),
    /// 故**只认 `baseUrl`**——若给 `alias = "base_url"`,serde 会把两键当同一字段而报
    /// `duplicate field`(实测导致取流解析失败、播放卡在开头)。`base_url` 作未知字段忽略。
    #[serde(rename = "baseUrl")]
    pub base_url: String,

    /// 码率(bps);缺失为 `None`。
    pub bandwidth: Option<i64>,

    /// 编解码器串(如 `mp4a.40.2` = AAC;`fLaC` = 无损),用于判 format。
    pub codecs: Option<String>,
}

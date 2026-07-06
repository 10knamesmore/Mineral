//! 封面下载 URL 的图床服务端缩放改写(仅传输层)。
//!
//! 已知图床支持按 URL 参数出缩放图时,把目标尺寸拼进**下载那一刻**用的地址,
//! 让原图字节根本不进本机——省带宽、省解码 CPU,更消掉全尺寸解码的瞬时大缓冲。
//! 改写只存在于发请求的一瞬:缓存 key、模型、协议里流转的永远是原始 URL,
//! 任何展示/复制场景拿到的都是无缩放版本。

use url::Url;

/// 已知图床则给出带缩放参数的下载 URL,不适用返回 `None`(用原始 URL 下载)。
///
/// 语法(均对真实图床实测过):
///   - 网易云 `p*.music.126.net`:追加 `?param={d}y{d}`,精确缩放(封面为方图)。
///   - B站 `i*.hdslb.com/bfs/...`:追加 `@{d}w_{d}h`,bounding-box fit,
///     保纵横比不裁切,输出保持原格式。
///
/// URL 已带 query / fragment / 缩放后缀时一律不碰——盲目追加会拼出非法地址。
///
/// # Params:
///   - `url`: 原始封面 URL
///   - `max_dim`: 目标最大边(像素,来自配置 `cover.max_dim`)
///
/// # Return:
///   带缩放参数的下载 URL;图床未知或 URL 形态不适用时 `None`。
pub fn scaled_url(url: &Url, max_dim: u32) -> Option<String> {
    if url.query().is_some() || url.fragment().is_some() {
        return None;
    }
    let host = url.host_str()?;
    if host.ends_with(".music.126.net") {
        return Some(format!("{url}?param={max_dim}y{max_dim}"));
    }
    if host.ends_with(".hdslb.com") && url.path().starts_with("/bfs/") && !url.path().contains('@')
    {
        return Some(format!("{url}@{max_dim}w_{max_dim}h"));
    }
    None
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::scaled_url;

    /// 测试入参:目标最大边(= default.lua 的 `cover.max_dim`)。
    const MAX_DIM: u32 = 384;

    /// 网易云图床:追加 `?param={d}y{d}`。
    #[test]
    fn netease_appends_param() -> color_eyre::Result<()> {
        let url = Url::parse("http://p1.music.126.net/-4rO7P==/109951164323917099.jpg")?;
        assert_eq!(
            scaled_url(&url, MAX_DIM).as_deref(),
            Some("http://p1.music.126.net/-4rO7P==/109951164323917099.jpg?param=384y384"),
        );
        Ok(())
    }

    /// B站图床 bfs 路径:追加 `@{d}w_{d}h`。
    #[test]
    fn bilibili_appends_wh_suffix() -> color_eyre::Result<()> {
        let url = Url::parse("http://i0.hdslb.com/bfs/archive/13ade80d.jpg")?;
        assert_eq!(
            scaled_url(&url, MAX_DIM).as_deref(),
            Some("http://i0.hdslb.com/bfs/archive/13ade80d.jpg@384w_384h"),
        );
        Ok(())
    }

    /// 已带 query 的 URL 不追加(再拼 `?param=` 会出非法地址)。
    #[test]
    fn existing_query_passes_through() -> color_eyre::Result<()> {
        let url = Url::parse("http://p1.music.126.net/a/1.jpg?param=200y200")?;
        assert_eq!(scaled_url(&url, MAX_DIM), None);
        Ok(())
    }

    /// 已带缩放后缀的 B站 URL 不重复追加。
    #[test]
    fn existing_scale_suffix_passes_through() -> color_eyre::Result<()> {
        let url = Url::parse("http://i0.hdslb.com/bfs/archive/a.jpg@672w_378h")?;
        assert_eq!(scaled_url(&url, MAX_DIM), None);
        Ok(())
    }

    /// 未知图床(含测试用 mock server 地址)原样放行。
    #[test]
    fn unknown_host_passes_through() -> color_eyre::Result<()> {
        let url = Url::parse("http://127.0.0.1:8080/cover.jpg")?;
        assert_eq!(scaled_url(&url, MAX_DIM), None);
        Ok(())
    }

    /// hdslb 非 bfs 路径不确定支持缩放,不碰。
    #[test]
    fn hdslb_non_bfs_passes_through() -> color_eyre::Result<()> {
        let url = Url::parse("http://i0.hdslb.com/other/a.jpg")?;
        assert_eq!(scaled_url(&url, MAX_DIM), None);
        Ok(())
    }
}

//! 收藏夹端点(`x/v3/fav/*`,明文 GET;「我的」/私密夹需登录 cookie)。

use std::future::Future;

use crate::transport::Transport;
use crate::wire::de::from_value;
use crate::wire::fav::{FavFolder, FavFolderList, FavInfo, FavMedia, FavResourceList};

/// 我创建的收藏夹列表端点(分页版,每项带 cover;`list-all` 全量但无 cover,不用)。
const FOLDER_CREATED_LIST_URL: &str = "https://api.bilibili.com/x/v3/fav/folder/created/list";

/// 收藏夹内容端点。
const RESOURCE_LIST_URL: &str = "https://api.bilibili.com/x/v3/fav/resource/list";

/// 翻页安全上限(条数 = 本值 × 每页 20)。`has_more` 是主终止条件,本上限只防
/// `has_more` 恒真时死循环,触达会 warn(不静默截断)。
const MAX_FAV_PAGES: u32 = 200;

/// 拉某用户创建的收藏夹列表一页(`up_mid` + 页码;分页 `created/list`,每项带 cover)。
///
/// # Params:
///   - `transport`: HTTP 传输层
///   - `mid`: 目标用户 mid
///   - `page`: 页码(从 1 起)
///
/// # Return:
///   该页收藏夹列表 DTO(含 `has_more`)。
async fn created_folders_page(
    transport: &Transport,
    mid: &str,
    page: u32,
) -> color_eyre::Result<FavFolderList> {
    let data = transport
        .get_data(&format!(
            "{FOLDER_CREATED_LIST_URL}?up_mid={mid}&pn={page}&ps=40"
        ))
        .await?;
    from_value(data)
}

/// 翻页拉全某用户创建的收藏夹列表(`up_mid`;拉「我的」需登录 cookie)。
///
/// # Params:
///   - `transport`: HTTP 传输层
///   - `mid`: 目标用户 mid
///
/// # Return:
///   合并所有页的收藏夹列表 DTO(每项带 cover / intro)。
pub async fn created_folders(
    transport: &Transport,
    mid: &str,
) -> color_eyre::Result<FavFolderList> {
    collect_folder_pages(|page| created_folders_page(transport, mid, page)).await
}

/// 翻页拉全收藏夹列表:反复调 `fetch(page)`(pn 从 1 起)直到 `has_more == false` / 空页 /
/// 触达 [`MAX_FAV_PAGES`],把各页 `list` 拼接。fetcher 注入,便于离线测试。
///
/// # Params:
///   - `fetch`: 按页码取一页收藏夹列表的异步取数器
///
/// # Return:
///   合并后的列表(`list` 为全部收藏夹,`has_more` 归 `false`)。
async fn collect_folder_pages<F, Fut>(mut fetch: F) -> color_eyre::Result<FavFolderList>
where
    F: FnMut(u32) -> Fut,
    Fut: Future<Output = color_eyre::Result<FavFolderList>>,
{
    let mut all = Vec::<FavFolder>::new();
    let mut page = 1;
    loop {
        let resp = fetch(page).await?;
        let has_more = resp.has_more;
        let batch = resp.list.unwrap_or_default();
        let batch_empty = batch.is_empty();
        all.extend(batch);
        if !has_more || batch_empty {
            break;
        }
        if page >= MAX_FAV_PAGES {
            mineral_log::warn!(
                target: "channel",
                pages = page,
                items = all.len(),
                "收藏夹列表翻页触达安全上限,可能未取全",
            );
            break;
        }
        page += 1;
    }
    Ok(FavFolderList {
        list: Some(all),
        has_more: false,
    })
}

/// 拉某收藏夹的内容(分页,每页 20)。
///
/// # Params:
///   - `transport`: HTTP 传输层
///   - `fid`: 收藏夹 id(media_id)
///   - `page`: 页码(从 1 起)
///
/// # Return:
///   收藏夹内容 DTO(元信息 + 条目)。
pub async fn resource_list(
    transport: &Transport,
    fid: &str,
    page: u32,
) -> color_eyre::Result<FavResourceList> {
    let data = transport
        .get_data(&format!(
            "{RESOURCE_LIST_URL}?media_id={fid}&pn={page}&ps=20"
        ))
        .await?;
    from_value(data)
}

/// 翻页拉全某收藏夹内容:反复调 `fetch(page)`(pn 从 1 起)直到 `has_more == false` / 空页 /
/// 触达 [`MAX_FAV_PAGES`],把各页 `medias` 拼接;`info` 取自第 1 页。fetcher 注入,便于离线测试。
///
/// # Params:
///   - `fetch`: 按页码取一页内容的异步取数器
///
/// # Return:
///   合并后的内容(`medias` 为全部条目,`has_more` 归 `false`)。
async fn collect_pages<F, Fut>(mut fetch: F) -> color_eyre::Result<FavResourceList>
where
    F: FnMut(u32) -> Fut,
    Fut: Future<Output = color_eyre::Result<FavResourceList>>,
{
    let mut info: Option<FavInfo> = None;
    let mut all = Vec::<FavMedia>::new();
    let mut page = 1;
    loop {
        let resp = fetch(page).await?;
        if info.is_none() {
            info = resp.info;
        }
        let has_more = resp.has_more;
        let batch = resp.medias.unwrap_or_default();
        let batch_empty = batch.is_empty();
        all.extend(batch);
        if !has_more || batch_empty {
            break;
        }
        if page >= MAX_FAV_PAGES {
            mineral_log::warn!(
                target: "channel",
                pages = page,
                items = all.len(),
                "收藏夹翻页触达安全上限,可能未取全",
            );
            break;
        }
        page += 1;
    }
    Ok(FavResourceList {
        info,
        medias: Some(all),
        has_more: false,
    })
}

/// 拉全某收藏夹内容(翻页拼接);channel 层用这个,不关心分页细节。
///
/// # Params:
///   - `transport`: HTTP 传输层
///   - `fid`: 收藏夹 id(media_id)
///
/// # Return:
///   合并所有页的收藏夹内容 DTO。
pub async fn all_resources(
    transport: &Transport,
    fid: &str,
) -> color_eyre::Result<FavResourceList> {
    collect_pages(|page| resource_list(transport, fid, page)).await
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::collect_pages;
    use crate::wire::fav::FavResourceList;

    /// 缺 `has_more` 字段时默认 false(单页收藏夹的常见形态)。
    #[test]
    fn resource_list_defaults_has_more_false() -> color_eyre::Result<()> {
        let list: FavResourceList = serde_json::from_value(json!({ "medias": [] }))?;
        assert!(!list.has_more, "缺字段应默认 false");
        Ok(())
    }

    /// collect_pages 反复翻页直到 `has_more == false`,拼齐所有页的 medias、info 取自第 1 页。
    /// 回归:曾只取第 1 页(ps=20),>20 条的收藏夹静默截断。
    #[tokio::test]
    async fn collect_pages_accumulates_until_has_more_false() -> color_eyre::Result<()> {
        let pages = [
            page(&["BV1", "BV2"], /*has_more*/ true)?,
            page(&["BV3", "BV4"], /*has_more*/ true)?,
            page(&["BV5"], /*has_more*/ false)?,
        ];
        let merged = collect_pages(|p| {
            let idx = usize::try_from(p).unwrap_or(0).saturating_sub(1);
            let resp = pages.get(idx).cloned();
            async move { resp.ok_or_else(|| color_eyre::eyre::eyre!("越界请求页 {p}")) }
        })
        .await?;
        let medias = merged.medias.unwrap_or_default();
        assert_eq!(medias.len(), 5, "三页共 5 条应全部拼齐");
        assert!(!merged.has_more, "合并结果 has_more 归 false");
        let info = merged
            .info
            .ok_or_else(|| color_eyre::eyre::eyre!("info 应取自第 1 页"))?;
        assert_eq!(info.title, "测试夹");
        Ok(())
    }

    /// 停在第 1 页:第 1 页就 `has_more == false`,不应再请求第 2 页(越界会报错)。
    #[tokio::test]
    async fn collect_pages_stops_when_first_page_is_complete() -> color_eyre::Result<()> {
        let only = [page(&["BV1"], /*has_more*/ false)?];
        let merged = collect_pages(|p| {
            let idx = usize::try_from(p).unwrap_or(0).saturating_sub(1);
            let resp = only.get(idx).cloned();
            async move { resp.ok_or_else(|| color_eyre::eyre::eyre!("不应请求页 {p}")) }
        })
        .await?;
        assert_eq!(merged.medias.unwrap_or_default().len(), 1);
        Ok(())
    }

    /// 构造一页收藏夹响应(每条只填 bvid + upper 必填项)。
    fn page(bvids: &[&str], has_more: bool) -> color_eyre::Result<FavResourceList> {
        let medias = bvids
            .iter()
            .map(|bv| json!({ "bvid": bv, "upper": { "mid": 1, "name": "up" } }))
            .collect::<Vec<_>>();
        Ok(serde_json::from_value(json!({
            "info": { "title": "测试夹", "media_count": 5 },
            "medias": medias,
            "has_more": has_more,
        }))?)
    }

    /// collect_folder_pages 反复翻页拼齐所有页的收藏夹项,`has_more == false` 收尾。
    /// 回归:分页 `created/list` 端点 >一页(ps=40)的收藏夹曾只取首页静默截断。
    #[tokio::test]
    async fn collect_folder_pages_accumulates_until_has_more_false() -> color_eyre::Result<()> {
        use super::collect_folder_pages;

        let pages = [
            folder_page(&[(1, "夹1"), (2, "夹2")], /*has_more*/ true)?,
            folder_page(&[(3, "夹3")], /*has_more*/ false)?,
        ];
        let merged = collect_folder_pages(|p| {
            let idx = usize::try_from(p).unwrap_or(0).saturating_sub(1);
            let resp = pages.get(idx).cloned();
            async move { resp.ok_or_else(|| color_eyre::eyre::eyre!("越界请求页 {p}")) }
        })
        .await?;
        let list = merged.list.unwrap_or_default();
        assert_eq!(list.len(), 3, "两页共 3 个收藏夹应全部拼齐");
        assert!(!merged.has_more, "合并结果 has_more 归 false");
        Ok(())
    }

    /// 构造一页「我创建的收藏夹」响应(每项填 id + title + cover)。
    fn folder_page(
        folders: &[(i64, &str)],
        has_more: bool,
    ) -> color_eyre::Result<crate::wire::fav::FavFolderList> {
        let list = folders
            .iter()
            .map(|(id, title)| json!({ "id": id, "title": title, "cover": "//i0.hdslb.com/c.jpg" }))
            .collect::<Vec<_>>();
        Ok(serde_json::from_value(json!({
            "list": list,
            "has_more": has_more,
        }))?)
    }
}

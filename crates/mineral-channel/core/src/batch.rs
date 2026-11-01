//! 按需列表批次；来源保留分页进度，调用者只负责继续读取或丢弃流。

use std::future::Future;

use async_stream::stream;
use futures_util::stream::BoxStream;

use crate::{Error, Page, PageResult, Result};

/// 一批列表条目，完成标记由来源根据远端响应判断。
#[derive(Debug, Clone)]
pub struct Batch<T> {
    /// 本批条目，保持来源顺序。
    pub items: Vec<T>,

    /// 后续没有更多结果；空批次也可以尚未完成。
    pub complete: bool,
}

/// 每次消费读取一批；失败保留当前进度，再次消费才重试，丢弃流取消读取。
pub type BatchStream<'a, T> = BoxStream<'a, Result<Batch<T>>>;

/// 将 offset 型来源的单页请求封装成流；成功才推进位置，失败不自动重试。
pub fn page_batches<'a, T, F, Fut>(mut fetch: F) -> BatchStream<'a, T>
where
    T: Send + 'a,
    F: FnMut(Page) -> Fut + Send + 'a,
    Fut: Future<Output = Result<PageResult<T>>> + Send + 'a,
{
    Box::pin(stream! {
        let mut page = Page::default();
        loop {
            let result = fetch(page).await.and_then(|result| {
                let limit = usize::try_from(page.limit).map_err(|source| Error::Parse {
                    source: Box::new(source),
                })?;
                Ok(Batch {
                    complete: !result.has_more.unwrap_or(result.items.len() >= limit),
                    items: result.items,
                })
            });
            match result {
                Ok(batch) => {
                    let complete = batch.complete;
                    yield Ok(batch);
                    if complete {
                        return;
                    }
                    let Some(offset) = page.offset.checked_add(page.limit) else {
                        yield Err(Error::InvalidData { field: "page offset overflow" });
                        return;
                    };
                    page.offset = offset;
                }
                Err(error) => yield Err(error),
            }
        }
    })
}

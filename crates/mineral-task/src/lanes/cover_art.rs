//! CoverArt lane:N 个 worker 共享单一队列,跑「裸 HTTP fetch + 图片解码」。
//!
//! 跟 ChannelFetch 不同,本 lane 不持 `Arc<dyn MusicChannel>`、不分优先级
//! ——封面拉取语义就是「用户在看哪个就是 User」,worker 抢同一个队列处理。

use std::sync::Arc;
use std::time::Duration;

use isahc::config::Configurable;
use isahc::AsyncReadResponseExt;
use isahc::HttpClient;
use mineral_model::MediaUrl;
use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::event::TaskEvent;
use crate::id::TaskId;
use crate::ongoing::Ongoing;
use crate::outcome::TaskOutcome;

/// 单一 worker 池的并发度。封面图都是几十 KB,4 路够覆盖快速翻 selection。
const WORKERS: usize = 4;

/// HTTP 客户端 timeout。封面比 audio 流小得多,30s 足够慢网兜底。
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// 解码后 resize 到此最大边(像素),保持比例。
///
/// 终端 cell 典型 8×16 px,cover 面板大概 30 cols × 15 rows ≈ 240×240 px;
/// 384 远超显示需求,留点余量给高 DPI / 大字号。原图常常 1024×1024(网易裸 URL),
/// resize 到 384 内存降 ~7x,RGBA 256KB/张 vs 1.5MB+。视觉上完全无损。
const COVER_MAX_DIM: u32 = 384;

/// 投递给 worker 的一次任务。
pub(crate) struct Job {
    /// 任务 id,完成后 ongoing.remove 用。
    pub id: TaskId,

    /// 待 fetch 的封面 URL。
    pub url: MediaUrl,

    /// 取消 token,用户切到别的歌单时可以批量 cancel。
    pub cancel: CancellationToken,

    /// 完成回执通道。
    pub done_tx: oneshot::Sender<TaskOutcome>,
}

/// CoverArt lane:对外只暴露 [`CoverArtLane::dispatch`]。
pub(crate) struct CoverArtLane {
    sender: mpsc::UnboundedSender<Job>,
}

impl CoverArtLane {
    /// 启动 lane:spawn `WORKERS` 个 worker,共享一个队列。
    /// HTTP client 建失败时不 spawn,后续 dispatch 进去的 job 静默丢
    /// (打到队列没人消费),用户只看到程序化封面 fallback,不崩。
    pub fn spawn(ongoing: &Arc<Ongoing>, event_tx: &Arc<Mutex<Vec<TaskEvent>>>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel::<Job>();
        let client = match HttpClient::builder().timeout(HTTP_TIMEOUT).build() {
            Ok(c) => c,
            Err(e) => {
                mineral_log::warn!(
                    target: "cover_art",
                    "isahc client init failed: {e}; covers disabled"
                );
                return Self { sender: tx };
            }
        };
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        for _ in 0..WORKERS {
            let rx = Arc::clone(&rx);
            let ongoing = Arc::clone(ongoing);
            let event_tx = Arc::clone(event_tx);
            let client = client.clone();
            tokio::spawn(async move {
                worker_loop(rx, ongoing, event_tx, client).await;
            });
        }
        Self { sender: tx }
    }

    /// 把一个 [`Job`] 投递到队列。worker 抢占式拉。
    pub fn dispatch(&self, job: Job) {
        let _ = self.sender.send(job);
    }
}

async fn worker_loop(
    rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Job>>>,
    ongoing: Arc<Ongoing>,
    event_tx: Arc<Mutex<Vec<TaskEvent>>>,
    client: HttpClient,
) {
    loop {
        let job = {
            let mut g = rx.lock().await;
            match g.recv().await {
                Some(j) => j,
                None => return, // 队列关了
            }
        };
        let id = job.id;
        run_job(job, &event_tx, &client).await;
        ongoing.remove(id);
    }
}

async fn run_job(job: Job, event_tx: &Arc<Mutex<Vec<TaskEvent>>>, client: &HttpClient) {
    let Job {
        id: _,
        url,
        cancel,
        done_tx,
    } = job;
    if cancel.is_cancelled() {
        let _ = done_tx.send(TaskOutcome::Cancelled);
        return;
    }
    let outcome = tokio::select! {
        biased;
        () = cancel.cancelled() => TaskOutcome::Cancelled,
        out = fetch_and_decode(&url, client, event_tx) => out,
    };
    let _ = done_tx.send(outcome);
}

async fn fetch_and_decode(
    url: &MediaUrl,
    client: &HttpClient,
    event_tx: &Arc<Mutex<Vec<TaskEvent>>>,
) -> TaskOutcome {
    let bytes = match read_bytes(url, client).await {
        Ok(b) => b,
        Err(e) => {
            mineral_log::warn!(target: "cover_art", url = %url, "fetch: {e}");
            return TaskOutcome::Failed;
        }
    };
    let img = match image::load_from_memory(&bytes) {
        Ok(i) => i,
        Err(e) => {
            mineral_log::warn!(target: "cover_art", url = %url, "decode: {e}");
            return TaskOutcome::Failed;
        }
    };
    // resize 到 COVER_MAX_DIM 之内 —— 保持纵横比,Triangle 滤镜质量比 Nearest 好、
    // 比 Lanczos3 快一档,对缩略图够用。原图小于这个就直接用(no-op)。
    let img = if img.width() > COVER_MAX_DIM || img.height() > COVER_MAX_DIM {
        img.resize(
            COVER_MAX_DIM,
            COVER_MAX_DIM,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    event_tx.lock().push(TaskEvent::CoverReady {
        url: url.clone(),
        image: Arc::new(img),
    });
    TaskOutcome::Ok
}

async fn read_bytes(url: &MediaUrl, client: &HttpClient) -> color_eyre::Result<Vec<u8>> {
    match url {
        MediaUrl::Remote(u) => {
            let mut resp = client
                .get_async(u.as_str())
                .await
                .map_err(|e| color_eyre::eyre::eyre!("http: {e}"))?;
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| color_eyre::eyre::eyre!("read body: {e}"))?;
            Ok(bytes)
        }
        MediaUrl::Local(p) => tokio::fs::read(p)
            .await
            .map_err(|e| color_eyre::eyre::eyre!("read file: {e}")),
    }
}

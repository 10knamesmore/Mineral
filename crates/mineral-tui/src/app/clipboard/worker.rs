//! 串行执行系统剪贴板写入；关闭队列后排空任务并释放独占的句柄。

use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, Builder, JoinHandle};

use arboard::{Clipboard, Error as ClipboardError};
use tokio::sync::oneshot;

/// 一次在剪贴板线程执行的写入；捕获的数据必须由任务拥有。
type WriteClipboard = Box<dyn FnOnce(&mut Clipboard) -> Result<(), ClipboardError> + Send>;

struct ClipboardRequest {
    write: WriteClipboard,

    reply: oneshot::Sender<Result<(), ClipboardWriteError>>,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum ClipboardWriteError {
    #[error("start clipboard worker")]
    StartWorker(#[source] io::Error),

    #[error("initialize clipboard")]
    Initialize(#[source] ClipboardError),

    #[error("write clipboard")]
    Write(#[source] ClipboardError),

    #[error("clipboard worker unavailable")]
    Unavailable,
}

/// 后台写入入口；`finish` 关闭队列并等待排队写入及句柄析构。
pub(crate) struct ClipboardWorker {
    requests: Sender<ClipboardRequest>,

    thread: JoinHandle<()>,
}

impl ClipboardWorker {
    pub(super) fn spawn() -> Result<Self, ClipboardWriteError> {
        let (requests, receiver) = mpsc::channel();
        let thread = Builder::new()
            .name("mineral-clipboard".to_owned())
            .spawn(move || run(receiver))
            .map_err(ClipboardWriteError::StartWorker)?;
        Ok(Self { requests, thread })
    }

    pub(super) fn finish(self) -> thread::Result<()> {
        drop(self.requests);
        self.thread.join()
    }

    pub(super) fn write(
        &self,
        write: impl FnOnce(&mut Clipboard) -> Result<(), ClipboardError> + Send + 'static,
    ) -> oneshot::Receiver<Result<(), ClipboardWriteError>> {
        let (reply, result) = oneshot::channel();
        let _ = self.requests.send(ClipboardRequest {
            write: Box::new(write),
            reply,
        });
        result
    }
}

fn run(requests: Receiver<ClipboardRequest>) {
    let mut clipboard = None;
    for ClipboardRequest { write, reply } in requests {
        let result = write_to_clipboard(&mut clipboard, write);
        let _ = reply.send(result);
    }
}

fn write_to_clipboard(
    clipboard: &mut Option<Clipboard>,
    write: WriteClipboard,
) -> Result<(), ClipboardWriteError> {
    let clipboard = match clipboard {
        Some(clipboard) => clipboard,
        empty @ None => empty.insert(Clipboard::new().map_err(ClipboardWriteError::Initialize)?),
    };
    write(clipboard).map_err(ClipboardWriteError::Write)
}

//! 前端复制动作与后台加载、写入结果提示；新复制操作取代旧请求的反馈。

use arboard::{Clipboard, Error as ClipboardError, ImageData};
use image::RgbaImage;
use mineral_model::{MediaUrl, SourceKind};
use tokio::sync::oneshot::Receiver;
use tokio::sync::oneshot::error::TryRecvError;

use crate::app::App;
use crate::components::toast::notifications::{TextTint, tinted_text_item};
use crate::image::fetch::OriginalImageError;

use super::worker::{ClipboardWorker, ClipboardWriteError};

/// 复制成功提示的文本长度上限，单位为字符。
const COPY_TOAST_MAX_CHARS: usize = 48;

/// 后台写入入口与最近一次复制的结果；析构时等待已提交写入完成。
#[derive(Default)]
pub(crate) struct ClipboardState {
    /// 首次写入时启动；系统剪贴板句柄由 worker 独占。
    pub(crate) worker: Option<ClipboardWorker>,

    /// 尚未提交写入的图片；新复制操作丢弃旧接收端，避免迟到图片覆盖新内容。
    pending_image: Option<Receiver<Result<RgbaImage, OriginalImageError>>>,

    /// 只接收最近一次提交的反馈；已入队的写入仍按顺序执行。
    pending_write: Option<PendingWrite>,
}

impl Drop for ClipboardState {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take()
            && worker.finish().is_err()
        {
            mineral_log::warn!(target: "tui", "剪贴板线程异常结束");
        }
    }
}

struct PendingWrite {
    result: Receiver<Result<(), ClipboardWriteError>>,

    success_message: String,
}

impl App {
    /// 取消等待中的图片复制，向后台提交文本写入。
    pub(crate) fn copy_to_clipboard(&mut self, text: &str) {
        self.clipboard.pending_image = None;
        let shown = if text.chars().count() > COPY_TOAST_MAX_CHARS {
            let head = text.chars().take(COPY_TOAST_MAX_CHARS).collect::<String>();
            format!("Copied: {head}…")
        } else {
            format!("Copied: {text}")
        };
        let text = text.to_owned();
        self.with_clipboard(move |clipboard| clipboard.set_text(text), shown);
    }

    /// 后台加载源图，准备完成后提交后台剪贴板写入。
    pub(crate) fn copy_image_to_clipboard(&mut self, source: SourceKind, url: MediaUrl) {
        self.clipboard.pending_write = None;
        self.clipboard.pending_image = Some(self.state.resources.images.load_original(source, url));
    }

    /// 主循环只检查结果，图片解码、PNG 编码和系统剪贴板操作均在后台执行。
    pub(crate) fn poll_clipboard(&mut self) {
        self.poll_image_copy();
        self.poll_clipboard_write();
    }

    fn poll_image_copy(&mut self) {
        let result = match self
            .clipboard
            .pending_image
            .as_mut()
            .map(Receiver::try_recv)
        {
            Some(Ok(result)) => result,
            Some(Err(TryRecvError::Closed)) => Err(OriginalImageError::Unavailable),
            None | Some(Err(TryRecvError::Empty)) => return,
        };
        self.clipboard.pending_image = None;
        match result {
            Ok(image) => {
                let (_, width, height) = image.sample_layout().extents();
                self.with_clipboard(
                    move |clipboard| {
                        clipboard.set_image(ImageData {
                            width,
                            height,
                            bytes: image.into_raw().into(),
                        })
                    },
                    "Copied image".to_owned(),
                );
            }
            Err(error) => {
                mineral_log::warn!(target: "tui", error = mineral_log::chain(&error), "读取复制图片失败");
                self.notifications.flash(tinted_text_item(
                    "Could not load image".to_owned(),
                    TextTint::Error,
                ));
            }
        }
    }

    fn with_clipboard(
        &mut self,
        write: impl FnOnce(&mut Clipboard) -> Result<(), ClipboardError> + Send + 'static,
        success_message: String,
    ) {
        self.clipboard.pending_write = None;
        let worker = match &mut self.clipboard.worker {
            Some(worker) => worker,
            empty @ None => match ClipboardWorker::spawn() {
                Ok(worker) => empty.insert(worker),
                Err(error) => {
                    self.notify_clipboard_error(&error);
                    return;
                }
            },
        };
        self.clipboard.pending_write = Some(PendingWrite {
            result: worker.write(write),
            success_message,
        });
    }

    fn poll_clipboard_write(&mut self) {
        let Some(pending) = self.clipboard.pending_write.as_mut() else {
            return;
        };
        match pending.result.try_recv() {
            Ok(Ok(())) => {
                self.notifications.flash(tinted_text_item(
                    pending.success_message.clone(),
                    TextTint::Normal,
                ));
            }
            Ok(Err(error)) => self.notify_clipboard_error(&error),
            Err(TryRecvError::Closed) => {
                self.notify_clipboard_error(&ClipboardWriteError::Unavailable);
            }
            Err(TryRecvError::Empty) => return,
        }
        self.clipboard.pending_write = None;
    }

    fn notify_clipboard_error(&mut self, error: &ClipboardWriteError) {
        mineral_log::warn!(target: "tui", error = mineral_log::chain(error), "剪贴板操作失败");
        let message = match error {
            ClipboardWriteError::Write(_) => "copy failed",
            ClipboardWriteError::StartWorker(_)
            | ClipboardWriteError::Initialize(_)
            | ClipboardWriteError::Unavailable => "clipboard unavailable",
        };
        self.notifications
            .flash(tinted_text_item(message.to_owned(), TextTint::Error));
    }
}

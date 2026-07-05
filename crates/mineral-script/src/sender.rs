//! daemon 侧持有的脚本投递句柄([`ScriptSender`]):fire-and-forget 投递 +
//! 带回执/超时的往返入口(拦截、查询、curate、复制模板)。
//!
//! 消息类型本体在 [`crate::message`];本模块只管「怎么发、怎么等」。

use crate::message::{
    ActionOutcome, CurateOutcome, PlaylistBrief, QueryId, ResolveValue, ScriptEvent, ScriptMsg,
};

/// daemon 侧持有的脚本投递句柄(fire-and-forget),带**热重载间接层**:
/// 内层是当前脚本线程的消息入口,[`Self::attach`] 在重载时原子换新,
/// 持有者(Notifier / 泵)无感。未挂线程(无脚本 / 重载窗口)时投递静默丢、
/// 查询立即回"未启用"。
///
/// 内层是 `std::sync::mpsc`(无界,send 永不阻塞):消费端在脚本线程,
/// 需要 `recv_timeout` 驱动 timer 心跳,tokio 通道给不了。
#[derive(Clone, Debug)]
pub struct ScriptSender {
    /// 当前脚本线程的消息入口;`None` = 未挂(无脚本 / 重载换 VM 的窗口)。
    inner: std::sync::Arc<parking_lot::RwLock<Option<std::sync::mpsc::Sender<ScriptMsg>>>>,
}

impl ScriptSender {
    /// 建一个未挂线程的句柄(daemon 装配期先建,脚本线程起来后 [`Self::attach`])。
    #[must_use]
    pub fn detached() -> Self {
        Self {
            inner: std::sync::Arc::new(parking_lot::RwLock::new(None)),
        }
    }

    /// 把(新)脚本线程的消息入口挂进来(启动 / 热重载换 VM 后调)。
    pub(crate) fn attach(&self, tx: std::sync::mpsc::Sender<ScriptMsg>) {
        *self.inner.write() = Some(tx);
    }

    /// 摘掉当前线程入口(重载 eval 失败弃新 VM 时**不**调——保留旧线程)。
    pub fn detach(&self) {
        *self.inner.write() = None;
    }

    /// 是否挂着活线程(daemon 报"脚本未启用"的判断点)。
    #[must_use]
    pub fn is_attached(&self) -> bool {
        self.inner.read().is_some()
    }

    /// 向当前线程投一条消息;未挂 / 线程已退出时把消息原样还给调用方
    /// (Box 压扁 Err 体积,消息只在失败路径装箱)。
    fn try_send(&self, msg: ScriptMsg) -> Result<(), Box<ScriptMsg>> {
        let guard = self.inner.read();
        match guard.as_ref() {
            Some(tx) => tx.send(msg).map_err(|failed| Box::new(failed.0)),
            None => Err(Box::new(msg)),
        }
    }

    /// 投递一个事件给脚本线程。
    ///
    /// # Params:
    ///   - `event`: 要投递的事件
    pub fn send(&self, event: ScriptEvent) {
        // 未挂 / 线程退出:丢弃即可,不是错误(脚本是旁路增强)。
        let _ = self.try_send(ScriptMsg::Event(event));
    }

    /// 回投一次异步查询的结果(daemon 泵完成 [`ScriptCmd`] 查询后调)。
    ///
    /// # Params:
    ///   - `query`: 查询句柄(随查询命令带出的那个)
    ///   - `value`: 查询结果
    pub fn resolve(&self, query: QueryId, value: ResolveValue) {
        let _ = self.try_send(ScriptMsg::Resolve { query, value });
    }

    /// 拉取 `mineral.bind` 的键绑定表。
    ///
    /// 未挂 / 线程已退出时,回执立即就绪为空表(client 合并空表 = 无 bind)。
    ///
    /// # Return:
    ///   oneshot 接收端;`await` 得到注册顺序的 bind 表。
    #[must_use]
    pub fn script_binds(
        &self,
    ) -> tokio::sync::oneshot::Receiver<Vec<mineral_protocol::ScriptBind>> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        if let Err(failed) = self.try_send(ScriptMsg::GetBinds { reply })
            && let ScriptMsg::GetBinds { reply } = *failed
        {
            let _ = reply.send(Vec::new());
        }
        rx
    }

    /// 同步拦截:把入参快照投给脚本线程并等裁决,墙钟超时放行。
    ///
    /// 一切异常路径(未挂线程 / 线程退出 / 超时)都返回
    /// [`HookDecision::Continue`](crate::HookDecision::Continue) —— 拦截失败
    /// 不致命,播放 / 下载照常推进,超时与线程退出各记一条 warn。
    ///
    /// # Params:
    ///   - `ctx`: 入参快照
    ///   - `timeout`: 软超时(配置 `script.hook_timeout_ms` / 预取窗口)
    ///
    /// # Return:
    ///   裁决结果。
    pub async fn intercept_stream(
        &self,
        ctx: crate::hooks::BeforeStreamCtx,
        timeout: std::time::Duration,
    ) -> crate::hooks::HookDecision {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.await_intercept(
            ScriptMsg::InterceptStream { ctx, reply },
            rx,
            timeout,
            crate::hooks::HookKind::BeforeStream,
        )
        .await
    }

    /// 同步拦截 `before_download`(语义同 [`Self::intercept_stream`],入参快照不同)。
    ///
    /// # Params:
    ///   - `ctx`: 入参快照
    ///   - `timeout`: 软超时(配置 `script.hook_timeout_ms`)
    ///
    /// # Return:
    ///   裁决结果。
    pub async fn intercept_download(
        &self,
        ctx: crate::hooks::BeforeDownloadCtx,
        timeout: std::time::Duration,
    ) -> crate::hooks::HookDecision {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.await_intercept(
            ScriptMsg::InterceptDownload { ctx, reply },
            rx,
            timeout,
            crate::hooks::HookKind::BeforeDownload,
        )
        .await
    }

    /// 发送一条拦截消息并带墙钟超时等回执(各拦截入口共用的往返骨架;
    /// 异常路径的放行语义见调用方文档)。
    async fn await_intercept(
        &self,
        msg: ScriptMsg,
        rx: tokio::sync::oneshot::Receiver<crate::hooks::HookDecision>,
        timeout: std::time::Duration,
        kind: crate::hooks::HookKind,
    ) -> crate::hooks::HookDecision {
        use crate::hooks::HookDecision;
        if self.try_send(msg).is_err() {
            // 无脚本线程:拦截天然不存在,静默放行(不是异常)。
            return HookDecision::Continue;
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(decision)) => decision,
            Ok(Err(_dropped)) => {
                mineral_log::warn!(
                    target: "script",
                    hook = kind.as_str(),
                    "脚本线程退出,拦截放行"
                );
                HookDecision::Continue
            }
            Err(_elapsed) => {
                mineral_log::warn!(
                    target: "script",
                    hook = kind.as_str(),
                    timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
                    "拦截 hook 超时,放行"
                );
                HookDecision::Continue
            }
        }
    }

    /// 跑一级 curate transform 并等采纳结果,墙钟超时透传。
    ///
    /// 一切异常路径(未挂线程 / 线程退出 / 超时)都返回
    /// [`CurateOutcome::Identity`] —— transform 失败不致命,原列表照常展示,
    /// 超时与线程退出各记一条 warn。
    ///
    /// # Params:
    ///   - `source`: `Some` = per-source 函数;`None` = 跨源函数(合并列表)
    ///   - `briefs`: 待 transform 的歌单投影
    ///   - `timeout`: 软超时(配置 `script.hook_timeout_ms`,与拦截 hook 同刻度)
    ///
    /// # Return:
    ///   采纳结果。
    pub async fn curate_playlists(
        &self,
        source: Option<mineral_model::SourceKind>,
        briefs: Vec<PlaylistBrief>,
        timeout: std::time::Duration,
    ) -> CurateOutcome {
        // SourceKind 是 Copy;name() 给日志用。
        let label = source.map_or("<merged>", |s| s.name());
        let (reply, rx) = tokio::sync::oneshot::channel();
        if self
            .try_send(ScriptMsg::CuratePlaylists {
                source,
                briefs,
                reply,
            })
            .is_err()
        {
            // 无脚本线程:transform 天然不存在,静默透传(不是异常)。
            return CurateOutcome::Identity;
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_dropped)) => {
                mineral_log::warn!(
                    target: "script",
                    source = label,
                    "脚本线程退出,curate 透传"
                );
                CurateOutcome::Identity
            }
            Err(_elapsed) => {
                mineral_log::warn!(
                    target: "script",
                    source = label,
                    timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
                    "curate transform 超时,透传"
                );
                CurateOutcome::Identity
            }
        }
    }

    /// 拉取 per-source curate 函数的源名键集(daemon 启动时对无对应 channel
    /// 的键打 warn 用)。
    ///
    /// 未挂 / 线程已退出时,回执立即就绪为空集。
    ///
    /// # Return:
    ///   oneshot 接收端;`await` 得到源名键集(不含跨源函数)。
    #[must_use]
    pub fn curate_source_keys(&self) -> tokio::sync::oneshot::Receiver<Vec<String>> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        if let Err(failed) = self.try_send(ScriptMsg::GetCurateKeys { reply })
            && let ScriptMsg::GetCurateKeys { reply } = *failed
        {
            let _ = reply.send(Vec::new());
        }
        rx
    }

    /// 调用一个具名动作,返回结果回执的接收端。
    ///
    /// 未挂 / 线程已退出时,回执立即就绪为 [`ActionOutcome::Failed`]。
    ///
    /// # Params:
    ///   - `name`: 动作注册名
    ///   - `ctx`: 按键瞬间的 client 上下文(无界面触发面传 `None`)
    ///   - `args`: 调用位置实参(CLI 采集;TUI 键位 / 无参触发传空 `Vec`)
    ///
    /// # Return:
    ///   oneshot 接收端;`await` 得到调用结果。
    #[must_use]
    pub fn invoke_action(
        &self,
        name: String,
        ctx: Option<mineral_protocol::KeyContext>,
        args: Vec<String>,
    ) -> tokio::sync::oneshot::Receiver<ActionOutcome> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        if let Err(failed) = self.try_send(ScriptMsg::Action {
            name,
            ctx,
            args,
            reply,
        }) && let ScriptMsg::Action { reply, .. } = *failed
        {
            let _ = reply.send(ActionOutcome::Failed("脚本未启用或线程已退出".to_owned()));
        }
        rx
    }

    /// 投递一次复制模板渲染(`copy.templates[index]` 的函数,脚本线程执行)。
    ///
    /// 未挂 / 线程已退出时,回执立即就绪为 `Err`。
    ///
    /// # Params:
    ///   - `index`: 模板下标(0-based,对位 config 数组序)
    ///   - `ctx`: 模板作用的实体
    ///
    /// # Return:
    ///   oneshot 接收端;`await` 得到剪贴板文本或人读错误。
    #[must_use]
    pub fn render_copy_template(
        &self,
        index: usize,
        ctx: mineral_protocol::CopyTemplateCtx,
    ) -> tokio::sync::oneshot::Receiver<Result<String, String>> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        if let Err(failed) = self.try_send(ScriptMsg::RenderCopyTemplate { index, ctx, reply })
            && let ScriptMsg::RenderCopyTemplate { reply, .. } = *failed
        {
            let _ = reply.send(Err("脚本未启用或线程已退出".to_owned()));
        }
        rx
    }
}

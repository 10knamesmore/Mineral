//! 连接建立后的握手帧:版本守门 + 订阅集声明。
//!
//! 没有"协议版本协商"——两端同仓同发版,**包版本相等才互通**
//! ([`PkgVersion::current`],workspace 统一版本,升版自动跟随、无手动 bump)。
//! 不匹配时 server 回 `accepted == false`,client 提示重启 daemon 后干净退出。

use color_eyre::eyre::eyre;
use serde::{Deserialize, Serialize};

/// 结构化包版本(workspace 统一版本)。两端相等才互通——同仓同发版前提下,
/// 二进制版本就是协议版本。比较走 `Eq`,人读展示走 `Display`(`0.4.2` 形)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PkgVersion {
    /// 主版本号。
    pub major: u16,

    /// 次版本号。
    pub minor: u16,

    /// 修订号。
    pub patch: u16,
}

impl PkgVersion {
    /// 本端包版本(编译期注入的 `CARGO_PKG_VERSION_{MAJOR,MINOR,PATCH}`)。
    #[must_use]
    pub fn current() -> Self {
        Self {
            major: parse_component(env!("CARGO_PKG_VERSION_MAJOR")),
            minor: parse_component(env!("CARGO_PKG_VERSION_MINOR")),
            patch: parse_component(env!("CARGO_PKG_VERSION_PATCH")),
        }
    }

    /// 与对端版本是否互通。**1.0 前不守 SemVer**——必须完全相等;
    /// **1.0 起按 SemVer**——主版本相同即互通(同 major 内协议向后兼容)。
    #[must_use]
    pub fn compatible_with(self, other: Self) -> bool {
        if self.major == 0 || other.major == 0 {
            self == other
        } else {
            self.major == other.major
        }
    }
}

impl std::fmt::Display for PkgVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// 解析单段版本号。cargo 保证 `CARGO_PKG_VERSION_*` 是十进制数,失败分支
/// 理论不可达,回 0 兜底(守卫测试钉住 `current() != 0.0.0`)。
fn parse_component(s: &str) -> u16 {
    s.parse().unwrap_or(0)
}

/// client → server 握手首帧(连接建立后第一帧,先于任何请求)。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientInfo {
    /// client 的包版本(构造经 [`ClientInfo::new`] 自动填 [`PkgVersion::current`])。
    pub version: PkgVersion,

    /// 期望接收的推送类别;不订阅的类别 server 不下发。一次性 CLI 命令订空集。
    pub subscriptions: Vec<Subscription>,
}

impl ClientInfo {
    /// 以本端包版本构造握手帧。
    #[must_use]
    pub fn new(subscriptions: Vec<Subscription>) -> Self {
        Self {
            version: PkgVersion::current(),
            subscriptions,
        }
    }

    /// 对端报的版本与本端是否互通(server 侧守门判定,规则见
    /// [`PkgVersion::compatible_with`])。
    #[must_use]
    pub fn version_matches(&self) -> bool {
        self.version.compatible_with(PkgVersion::current())
    }
}

/// server → client 握手应答。`accepted == false` 时连接随即被 server 关闭,
/// client 据 [`reason`](Self::reason) 与 [`version`](Self::version) 出人话提示。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerHello {
    /// 是否接受本连接。
    pub accepted: bool,

    /// 拒绝原因;`accepted == true` 时恒 `None`。
    pub reason: Option<RejectReason>,

    /// server 的包版本(版本错配时 client 用它拼提示)。
    pub version: PkgVersion,
}

impl ServerHello {
    /// 接受连接的应答。
    #[must_use]
    pub fn accept() -> Self {
        Self {
            accepted: true,
            reason: None,
            version: PkgVersion::current(),
        }
    }

    /// 被接受则 `Ok(())`,被拒则给出人话错误(client 侧握手校验的单一入口,
    /// oneshot 与长连接 client 共用)。
    ///
    /// # Errors
    /// 握手被拒(busy / 版本不匹配)。
    pub fn ensure_accepted(&self) -> color_eyre::Result<()> {
        if self.accepted {
            return Ok(());
        }
        match self.reason {
            Some(RejectReason::Busy) => Err(eyre!("daemon busy:已有另一个 client 连接")),
            Some(RejectReason::VersionMismatch) => Err(eyre!(
                "daemon 版本 {} 与 client 版本 {} 不一致,请重启 daemon",
                self.version,
                PkgVersion::current()
            )),
            None => Err(eyre!("daemon 拒绝了连接(未给出原因)")),
        }
    }

    /// 拒绝连接的应答。
    #[must_use]
    pub fn reject(reason: RejectReason) -> Self {
        Self {
            accepted: false,
            reason: Some(reason),
            version: PkgVersion::current(),
        }
    }
}

/// 握手被拒的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectReason {
    /// 已有 client 占用(单 client 限制)。
    Busy,

    /// 两端包版本不一致(多见于升级后旧 daemon 仍在跑,重启 daemon 即解)。
    VersionMismatch,
}

/// client 侧握手单入口:发 [`Frame::Handshake`](crate::Frame::Handshake)、等
/// [`Frame::Hello`](crate::Frame::Hello)、校验 accepted。oneshot 与长连接 client 共用。
///
/// **发送失败也先读一帧再定罪**:server 的 busy 拒绝不等握手帧就回 Hello 并关连接,
/// 此时本端的握手帧可能撞上已关 socket(EPIPE)——真正的原因在缓冲里的 Hello,
/// 优先把它捞出来报人话,而不是报一条没信息量的 broken pipe。
///
/// # Params:
///   - `conn`: 已建立的 framed 连接
///   - `info`: 握手信息(版本 + 订阅集)
///
/// # Errors
/// 握手被拒(busy / 版本不匹配)/ 对端没回 Hello / 连接被关。
pub async fn client_handshake<S>(
    conn: &mut crate::Framed<S>,
    info: ClientInfo,
) -> color_eyre::Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use color_eyre::eyre::{WrapErr, bail};

    use crate::codec::{recv, send};
    use crate::frame::Frame;

    let sent = send(conn, &Frame::Handshake(info)).await;
    match recv::<Frame, _>(conn).await {
        Ok(Some(Frame::Hello(hello))) => hello.ensure_accepted(),
        Ok(Some(other)) => bail!("握手应答应是 Hello,实际收到 {other:?}"),
        Ok(None) => {
            sent.wrap_err("发送握手帧失败")?;
            bail!("daemon 在握手期间关闭了连接(版本过旧的 daemon 不认识握手帧,重启 daemon 试试)")
        }
        Err(recv_err) => {
            sent.wrap_err("发送握手帧失败")?;
            Err(recv_err).wrap_err("等待握手应答")
        }
    }
}

/// 订阅类别(client 想收哪类 [`Event`](crate::Event),按类别整组订阅)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Subscription {
    /// 属性变更([`Event::PropertyChanged`](crate::Event::PropertyChanged))。
    Property,

    /// 瞬时提示([`Event::Toast`](crate::Event::Toast))。
    Toast,

    /// 生命周期事件(曲终 / 下载完成)。
    Lifecycle,

    /// 自定义事件总线([`Event::BusMessage`](crate::Event::BusMessage),
    /// 脚本 / 外部 client 自定义消息;内置 TUI 不订阅)。
    Bus,
}

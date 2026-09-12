//! 为 TUI、CLI 和其他客户端提供统一的 daemon 访问入口。
//!
//! 每个 [`Client`] 拥有一条会话和一份本地状态镜像。daemon 执行业务操作并推送状态；
//! 客户端负责选择订阅、消费结果和展示数据，请求配对、更新组装与重新同步由会话处理。
//!
//! # 接入顺序
//!
//! 1. **连接**：由调用方建立传输连接，再将 [`mineral_protocol::Wire`] 交给
//!    [`Client::from_wire`] 完成握手并启动会话。[`connection`] 提供容量配置、
//!    连接错误、启动数据和会话指标。会话需要 Tokio runtime；传输连接的建立与
//!    daemon 的启动由调用方安排。
//! 2. **订阅**：用 [`Client::subscribe`] 选择需要的主题，再从 [`Client::mirror`]
//!    读取已收到的状态。连接成功不代表订阅首帧已到达；按需调用
//!    [`Client::wait_subscriptions_ready`] 等待，它也会因超时或断连返回。随后用订阅 ID
//!    调用 [`state::Mirror::subscription_seen`] 检查就绪状态，避免把初始值当作 daemon 状态。
//!    同一主题按引用计数订阅，每次订阅应有对应的 [`Client::unsubscribe`]。
//! 3. **操作**：用 [`Client::fire`] 提交不等待结果的 [`mineral_protocol::Request`]；
//!    需要结论时使用 [`Client::submit`] 或队列、脚本等类型化方法，按 [`operation`]
//!    处理提交与执行结果。收到成功结论不代表本地镜像已更新，状态仍由订阅推送确认。
//! 4. **读取与展示**：[`state`] 提供本地镜像及播放、队列、下载等视图。
//!    读取状态不发送请求；事件和 PCM 使用消费式读取，应在一个消费点取走后分发。
//! 5. **结束会话**：用 [`Client::connected`] 检查连接状态，用 [`Client::close`]
//!    主动关闭。断连后会话不自动重连，也不重发执行结果不明的操作。
//!
//! # 提交与执行结果
//!
//! 返回 `Result<Pending<T>, SubmitError>` 的方法先报告本地是否接受请求，再由
//! [`operation::Pending::outcome`] 等待 daemon 结论；异步便捷方法直接返回
//! [`operation::Outcome`]。[`Client::fire`] 提交后立即返回，不向调用方交付执行结论
//! 或查询载荷；本地提交失败或 daemon 明确拒绝操作时记日志。
//! [`Client::submit`] 的自定义译码器依次接收 daemon 结论和自动派生的请求名，
//! 可将名称用于应答类型或载荷不符时的诊断。
//!
//! # 按需能力
//!
//! - **启动数据**：需要来源能力或脚本键绑定时，调用 [`Client::bootstrap`]。
//! - **播放位置**：[`state::PlaybackMirror`] 根据最近的播放锚点和本地时钟推进展示位置。
//! - **音频可视化**：订阅 PCM 后，用 [`state::Mirror::drain_pcm`] 取走近期样本，
//!   同时消费 [`state::Mirror::take_pcm_discontinuity`]。出现断续时，展示端应重置
//!   依赖连续样本的 FFT 或波形状态；窗口有容量上限，不能用于无损录音。
//! - **连接诊断**：[`Client::metrics`] 返回发送批次、消息、请求和丢弃更新的计数。
//!
//! # 最小接入示例
//!
//! ```no_run
//! use std::path::Path;
//! use std::time::Duration;
//!
//! use mineral_client::Client;
//! use mineral_client::connection::{ClientConfig, ConnectError};
//! use mineral_protocol::{SocketWire, SubscriptionTopic};
//!
//! # async fn show_queue(socket_path: &Path) -> Result<(), ConnectError> {
//! let wire = SocketWire::connect(socket_path).await?;
//! let client = Client::from_wire(Box::new(wire), "queue_view", ClientConfig::default()).await?;
//! let subscription = client.subscribe(SubscriptionTopic::Player);
//! client.wait_subscriptions_ready(Duration::from_secs(/*secs*/ 2)).await;
//! if client.mirror().subscription_seen(subscription) {
//!     client.mirror().read_player(|player| {
//!         println!("队列共 {} 首", player.queue().len());
//!     });
//! }
//! client.unsubscribe(SubscriptionTopic::Player);
//! client.close();
//! # Ok(())
//! # }
//! ```

mod client;
mod session;

pub mod connection;
pub mod operation;
pub mod state;

pub use client::Client;

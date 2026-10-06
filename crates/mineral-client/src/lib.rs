//! abstraction for client of mineral
//!
//! example:
//!
//! ```
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

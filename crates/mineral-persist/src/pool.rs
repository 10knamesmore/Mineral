//! 共享的 sqlite 连接逻辑:[`ServerStore`](crate::ServerStore) 与
//! [`ClientStore`](crate::ClientStore) 各自打开自己的库文件都走这里,单一真相源。

use std::path::Path;

use color_eyre::eyre::WrapErr;
use sqlx::SqlitePool;
use sqlx::sqlite::SqlitePoolOptions;

/// 连接(或创建)一个 sqlite 库文件,返回单连接池。
///
/// `mode=rwc` 不存在则建文件(但**不建父目录**,调用方需先确保目录存在)。
///
/// # Params:
///   - `db_path`: sqlite 文件路径
///
/// # Return:
///   就绪连接池;连接失败返回 `Err`。
pub(crate) async fn connect(db_path: &Path) -> color_eyre::Result<SqlitePool> {
    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .wrap_err_with(|| format!("连接 sqlite 失败 path={}", db_path.display()))
}

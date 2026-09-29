//! SQLite 文件数据库的单连接初始化。

use std::path::Path;

use sea_orm::{ConnectOptions, Database, DatabaseConnection};

/// 连接(或创建)一个 sqlite 库文件,返回单连接池。
///
/// `mode=rwc` 不存在则建文件(但**不建父目录**,调用方需先确保目录存在)。
///
/// # Params:
///   - `db_path`: sqlite 文件路径
///
/// # Return:
///   就绪连接池;连接失败返回 `Err`。
pub async fn connect(db_path: &Path) -> crate::Result<DatabaseConnection> {
    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    let mut options = ConnectOptions::new(url);
    options.max_connections(/*value*/ 1);
    Database::connect(options)
        .await
        .map_err(|source| crate::Error::Connect {
            path: db_path.to_path_buf(),
            source,
        })
}

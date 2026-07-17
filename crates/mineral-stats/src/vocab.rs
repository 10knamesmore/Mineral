//! 落库用的受控词汇枚举。
//!
//! plays 事实行与事件行里那些「取值有限、进 SQL CHECK 约束」的列,在 Rust 侧用强
//! 类型枚举表示。全部派生 `sqlx::Type`(TEXT 存储、snake_case),写时直接 bind、读时经
//! `sqlx::FromRow` / `query_as` 按类型 decode——领域枚举与 TEXT 列双向由 `sqlx::Type`
//! 强类型转换,无手工字符串映射。

/// 一次播放的结束原因(plays.finish_reason)。
///
/// 点播新歌顶掉在播曲时,在播曲结算记 [`FinishReason::Skip`]。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type, serde::Serialize)]
#[sqlx(rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// 自然播完。
    Eof,

    /// 用户跳过(next / prev 切歌,或点播顶掉)。
    Skip,

    /// 用户显式停止。
    Stop,

    /// 解码 / 取链失败导致中断。
    Error,
}

/// 播放当时音频本体的来源位置(plays.playback_origin)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum PlaybackOrigin {
    /// 下载导出库(永久,文件系统即真相)。
    Download,

    /// 音频本体缓存(LRU,可被淘汰)。
    Cache,

    /// 远端流(可能边播边收割入缓存)。
    Remote,
}

/// 行为的发起方(跨切面列 actor)。
///
/// 脚本命令与用户请求共用同一播放核心,不带此标注则分不清 seek / love 是人按的
/// 还是脚本干的。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum Actor {
    /// 用户在 TUI 交互发起。
    User,

    /// Lua 脚本发起。
    Script,

    /// daemon 自治链路发起(自动接续 / 会话恢复等)。
    System,

    /// CLI 子命令发起。
    Cli,
}

/// 播放模式(plays.play_mode / mode_changes.from_mode / to_mode)。
///
/// 与 client 侧播放模式同构但独立定义——stats 保持 client 形态中立、不依赖 protocol,
/// 边界转换在 server 侧做;落库串与既有 `script_name` 词汇一致,历史数据不漂移。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum PlayMode {
    /// 顺序播放(到底停止)。
    Sequential,

    /// 随机播放。
    Shuffle,

    /// 整列循环。
    RepeatAll,

    /// 单曲循环。
    RepeatOne,
}

/// 一行播放的发起方式(plays.origin_kind)。
///
/// 与队列上下文([`crate::context::QueueContext`])分层:本枚举答「这一行怎么起
/// 播的」,上下文答「队列来自哪」。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum PlayOrigin {
    /// 用户在某视图显式点播。
    Explicit,

    /// 播完自动接续 / next / prev 推进。
    AutoAdvance,

    /// daemon 重启后的会话恢复起播。
    Resume,

    /// Lua 脚本发起。
    Script,

    /// 未标注(旧 client 缺省,向后兼容)。
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::{Actor, FinishReason, PlayOrigin, PlaybackOrigin};
    use sqlx::sqlite::SqlitePoolOptions;
    use sqlx::{Decode, Encode, Sqlite, SqlitePool, Type};

    /// 建一个只有单列 `v TEXT` 的内存库,验证枚举 encode/decode 往返。
    async fn mem_pool() -> color_eyre::Result<SqlitePool> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?;
        sqlx::query("CREATE TABLE t (v TEXT NOT NULL)")
            .execute(&pool)
            .await?;
        Ok(pool)
    }

    /// 对一组 `(变体, 期望落库串)`:插入后既断言裸串(pin 存储词汇),又断言类型化
    /// 读回等于原变体(验证派生的 Encode/Decode)。
    async fn assert_round_trip<T>(cases: &[(T, &str)]) -> color_eyre::Result<()>
    where
        T: Copy
            + PartialEq
            + std::fmt::Debug
            + Type<Sqlite>
            + for<'a> Encode<'a, Sqlite>
            + for<'a> Decode<'a, Sqlite>
            + Send
            + Unpin
            + 'static,
    {
        let pool = mem_pool().await?;
        for (variant, text) in cases {
            sqlx::query("INSERT INTO t(v) VALUES(?)")
                .bind(*variant)
                .execute(&pool)
                .await?;
            let raw =
                sqlx::query_scalar::<_, String>("SELECT v FROM t ORDER BY rowid DESC LIMIT 1")
                    .fetch_one(&pool)
                    .await?;
            assert_eq!(&raw, text, "落库串 for {variant:?}");
            let got = sqlx::query_scalar::<_, T>("SELECT v FROM t ORDER BY rowid DESC LIMIT 1")
                .fetch_one(&pool)
                .await?;
            assert_eq!(got, *variant);
        }
        Ok(())
    }

    #[tokio::test]
    async fn finish_reason_round_trips() -> color_eyre::Result<()> {
        assert_round_trip(&[
            (FinishReason::Eof, "eof"),
            (FinishReason::Skip, "skip"),
            (FinishReason::Stop, "stop"),
            (FinishReason::Error, "error"),
        ])
        .await
    }

    #[tokio::test]
    async fn playback_origin_round_trips() -> color_eyre::Result<()> {
        assert_round_trip(&[
            (PlaybackOrigin::Download, "download"),
            (PlaybackOrigin::Cache, "cache"),
            (PlaybackOrigin::Remote, "remote"),
        ])
        .await
    }

    #[tokio::test]
    async fn actor_round_trips() -> color_eyre::Result<()> {
        assert_round_trip(&[
            (Actor::User, "user"),
            (Actor::Script, "script"),
            (Actor::System, "system"),
            (Actor::Cli, "cli"),
        ])
        .await
    }

    #[tokio::test]
    async fn play_origin_round_trips() -> color_eyre::Result<()> {
        assert_round_trip(&[
            (PlayOrigin::Explicit, "explicit"),
            (PlayOrigin::AutoAdvance, "auto_advance"),
            (PlayOrigin::Resume, "resume"),
            (PlayOrigin::Script, "script"),
            (PlayOrigin::Unknown, "unknown"),
        ])
        .await
    }

    #[tokio::test]
    async fn play_mode_round_trips() -> color_eyre::Result<()> {
        use super::PlayMode;
        assert_round_trip(&[
            (PlayMode::Sequential, "sequential"),
            (PlayMode::Shuffle, "shuffle"),
            (PlayMode::RepeatAll, "repeat_all"),
            (PlayMode::RepeatOne, "repeat_one"),
        ])
        .await
    }
}

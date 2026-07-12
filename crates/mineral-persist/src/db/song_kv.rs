//! per-song 持久 KV:开放命名空间键落 `song_kv` 表,一等字段 `rating` 落
//! `song_stats.rating` 列。挂在 [`NamespaceStore`] 上的扩展方法。
//!
//! 开放 key 与一等字段的路由(`rating` 等保留名改走专用方法)由上层(Lua API)
//! 负责;本层对保留键写入直接拒绝,防止旁路。

use color_eyre::eyre::{WrapErr, bail};
use mineral_log::trace;
use mineral_model::SongId;
use mineral_protocol::StoreValue;

use crate::db::namespace::NamespaceStore;

/// 保留键(一等字段名):`kv_set` 拒写,读写应走专用方法 / `song_stats` 映射。
pub const RESERVED_KEYS: [&str; 3] = ["local_play_count", "rating", "last_played"];

/// rating 合法上限(0..=5)。
const RATING_MAX: u8 = 5;

/// `song_kv` 一行的原始列:(vtype, int_val, real_val, text_val)。
type KvRow = (String, Option<i64>, Option<f64>, Option<String>);

impl NamespaceStore {
    /// 读一条开放 KV;降级 / 未命中返回 `Ok(StoreValue::Nil)`。
    ///
    /// # Params:
    ///   - `id`: 歌曲 id(裸值入库,namespace 由本 store 隐含)
    ///   - `key`: 开放键(保留键不在本表,查询恒 `Nil`)
    ///
    /// # Return:
    ///   命中返回标量值,未命中返回 `StoreValue::Nil`。
    pub async fn kv_get(&self, id: &SongId, key: &str) -> color_eyre::Result<StoreValue> {
        let Some(pool) = self.pool() else {
            return Ok(StoreValue::Nil);
        };
        let row: Option<KvRow> = sqlx::query_as(
            "SELECT vtype,int_val,real_val,text_val FROM song_kv \
             WHERE namespace=? AND song_value=? AND key=?",
        )
        .bind(self.namespace())
        .bind(id.value())
        .bind(key)
        .fetch_optional(pool)
        .await
        .wrap_err_with(|| format!("读 song_kv 失败 song={} key={key}", id.value()))?;
        let Some((vtype, int_val, real_val, text_val)) = row else {
            return Ok(StoreValue::Nil);
        };
        decode_value(&vtype, int_val, real_val, text_val)
            .wrap_err_with(|| format!("song_kv 值重建失败 song={} key={key}", id.value()))
    }

    /// 写一条开放 KV(upsert;`Nil` 删除该 key)。降级 no-op。
    ///
    /// 保留键([`RESERVED_KEYS`])返回 `Err`——一等字段走 [`Self::set_rating`]
    /// 等专用方法,不允许从开放表旁路。
    ///
    /// # Params:
    ///   - `id`: 歌曲 id
    ///   - `key`: 开放键
    ///   - `value`: 标量值(`Nil` = 删除)
    pub async fn kv_set(
        &self,
        id: &SongId,
        key: &str,
        value: &StoreValue,
    ) -> color_eyre::Result<()> {
        if RESERVED_KEYS.contains(&key) {
            bail!("key {key:?} 是保留的一等字段,不能写入开放 KV(走专用方法)");
        }
        let Some(pool) = self.pool() else {
            return Ok(());
        };
        trace!(target: "persist", song = %id.value(), key, "kv_set");
        if matches!(value, StoreValue::Nil) {
            sqlx::query("DELETE FROM song_kv WHERE namespace=? AND song_value=? AND key=?")
                .bind(self.namespace())
                .bind(id.value())
                .bind(key)
                .execute(pool)
                .await
                .wrap_err_with(|| format!("删 song_kv 失败 song={} key={key}", id.value()))?;
            return Ok(());
        }
        let (vtype, int_val, real_val, text_val) = encode_value(value);
        sqlx::query(
            "INSERT INTO song_kv(namespace,song_value,key,vtype,int_val,real_val,text_val) \
             VALUES(?,?,?,?,?,?,?) \
             ON CONFLICT(namespace,song_value,key) DO UPDATE SET \
             vtype=excluded.vtype,int_val=excluded.int_val,\
             real_val=excluded.real_val,text_val=excluded.text_val",
        )
        .bind(self.namespace())
        .bind(id.value())
        .bind(key)
        .bind(vtype)
        .bind(int_val)
        .bind(real_val)
        .bind(text_val)
        .execute(pool)
        .await
        .wrap_err_with(|| format!("写 song_kv 失败 song={} key={key}", id.value()))?;
        Ok(())
    }

    /// 数值自增:读-改-写单语句完成(upsert + `int_val + delta`),返回自增后的值。
    ///
    /// 仅对 `Int` 类型值有意义;key 不存在视作 0 起步。现有值非 `Int`(`vtype != 'int'`)
    /// 返回 `Err`。降级返回 `Ok(StoreValue::Nil)`。
    ///
    /// # Params:
    ///   - `id`: 歌曲 id
    ///   - `key`: 开放键(保留键同 [`Self::kv_set`] 拒绝)
    ///   - `delta`: 增量(可负)
    ///
    /// # Return:
    ///   自增后的 `StoreValue::Int`。
    pub async fn kv_inc(
        &self,
        id: &SongId,
        key: &str,
        delta: i64,
    ) -> color_eyre::Result<StoreValue> {
        if RESERVED_KEYS.contains(&key) {
            bail!("key {key:?} 是保留的一等字段,不能写入开放 KV(走专用方法)");
        }
        let Some(pool) = self.pool() else {
            return Ok(StoreValue::Nil);
        };
        trace!(target: "persist", song = %id.value(), key, delta, "kv_inc");
        // 单语句 upsert-自增:不存在则以 delta 起步;存在但类型不是 int 时
        // WHERE 子句让 UPDATE 不命中,RETURNING 无行,下面按错误处理。
        let row: Option<(i64,)> = sqlx::query_as(
            "INSERT INTO song_kv(namespace,song_value,key,vtype,int_val) \
             VALUES(?,?,?,'int',?) \
             ON CONFLICT(namespace,song_value,key) DO UPDATE SET \
             int_val=song_kv.int_val+excluded.int_val \
             WHERE song_kv.vtype='int' \
             RETURNING int_val",
        )
        .bind(self.namespace())
        .bind(id.value())
        .bind(key)
        .bind(delta)
        .fetch_optional(pool)
        .await
        .wrap_err_with(|| format!("自增 song_kv 失败 song={} key={key}", id.value()))?;
        let Some((value,)) = row else {
            bail!("key {key:?} 现有值不是整数,不能自增");
        };
        Ok(StoreValue::Int(value))
    }

    /// 设/清 rating(一等字段,落 `song_stats.rating`;`None` 清空)。降级 no-op。
    ///
    /// # Params:
    ///   - `id`: 歌曲 id
    ///   - `rating`: 0..=5;`None` 清空;>5 返回 `Err`
    pub async fn set_rating(&self, id: &SongId, rating: Option<u8>) -> color_eyre::Result<()> {
        if let Some(r) = rating
            && r > RATING_MAX
        {
            bail!("rating {r} 越界(合法 0..={RATING_MAX})");
        }
        let Some(pool) = self.pool() else {
            return Ok(());
        };
        trace!(target: "persist", song = %id.value(), ?rating, "set_rating");
        let value = rating.map(i64::from);
        sqlx::query(
            "INSERT INTO song_stats(namespace,song_value,rating) VALUES(?,?,?) \
             ON CONFLICT(namespace,song_value) DO UPDATE SET rating=excluded.rating",
        )
        .bind(self.namespace())
        .bind(id.value())
        .bind(value)
        .execute(pool)
        .await
        .wrap_err_with(|| format!("写 rating 失败 song={}", id.value()))?;
        Ok(())
    }

    /// 读 rating。降级 / 无记录 / 未评分返回 `Ok(None)`。
    ///
    /// # Params:
    ///   - `id`: 歌曲 id
    ///
    /// # Return:
    ///   已评分返回 `Some(0..=5)`。
    pub async fn query_rating(&self, id: &SongId) -> color_eyre::Result<Option<u8>> {
        let Some(pool) = self.pool() else {
            return Ok(None);
        };
        let row: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT rating FROM song_stats WHERE namespace=? AND song_value=?")
                .bind(self.namespace())
                .bind(id.value())
                .fetch_optional(pool)
                .await
                .wrap_err_with(|| format!("查 rating 失败 song={}", id.value()))?;
        let Some((Some(raw),)) = row else {
            return Ok(None);
        };
        Ok(Some(u8::try_from(raw)?))
    }
}

/// `StoreValue` → 行编码(vtype 标签 + 三候选列,bool 落 int_val)。
fn encode_value(value: &StoreValue) -> (&'static str, Option<i64>, Option<f64>, Option<String>) {
    match value {
        StoreValue::Int(n) => ("int", Some(*n), None, None),
        StoreValue::Real(f) => ("real", None, Some(*f), None),
        StoreValue::Text(s) => ("text", None, None, Some(s.clone())),
        StoreValue::Bool(b) => ("bool", Some(i64::from(*b)), None, None),
        // 调用方已在 kv_set 前置分支处理 Nil(删除),不会走到这里。
        StoreValue::Nil => ("nil", None, None, None),
    }
}

/// 行编码 → `StoreValue`(按 vtype 标签重建;列与标签不符返回 `Err`)。
fn decode_value(
    vtype: &str,
    int_val: Option<i64>,
    real_val: Option<f64>,
    text_val: Option<String>,
) -> color_eyre::Result<StoreValue> {
    match (vtype, int_val, real_val, text_val) {
        ("int", Some(n), _, _) => Ok(StoreValue::Int(n)),
        ("real", _, Some(f), _) => Ok(StoreValue::Real(f)),
        ("text", _, _, Some(s)) => Ok(StoreValue::Text(s)),
        ("bool", Some(b), _, _) => Ok(StoreValue::Bool(b != 0)),
        (vtype, ..) => bail!("song_kv 行损坏:vtype={vtype:?} 与值列不符"),
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::{SongId, SourceKind};
    use mineral_protocol::StoreValue;

    /// 开一个 tempdir sqlite,返回 (守住生命周期的 dir, NETEASE scope)。
    async fn open_scope()
    -> color_eyre::Result<(tempfile::TempDir, crate::db::namespace::NamespaceStore)> {
        let dir = tempfile::tempdir()?;
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
        let s = p.scope(SourceKind::NETEASE);
        Ok((dir, s))
    }

    #[tokio::test]
    async fn song_kv_roundtrips_all_variants() -> color_eyre::Result<()> {
        let (_dir, s) = open_scope().await?;
        let id = SongId::new(SourceKind::NETEASE, "1");
        let cases = [
            ("plugin.int", StoreValue::Int(-42)),
            ("plugin.real", StoreValue::Real(2.5)),
            ("plugin.text", StoreValue::Text("漫游".to_owned())),
            ("plugin.bool", StoreValue::Bool(true)),
        ];
        for (key, value) in &cases {
            s.kv_set(&id, key, value).await?;
            assert_eq!(s.kv_get(&id, key).await?, *value, "key={key}");
        }
        // 未命中 → Nil;写 Nil → 删除回到 Nil
        assert_eq!(s.kv_get(&id, "plugin.miss").await?, StoreValue::Nil);
        s.kv_set(&id, "plugin.int", &StoreValue::Nil).await?;
        assert_eq!(s.kv_get(&id, "plugin.int").await?, StoreValue::Nil);
        Ok(())
    }

    #[tokio::test]
    async fn song_kv_isolated_by_source() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
        let netease = p.scope(SourceKind::NETEASE);
        let local = p.scope(SourceKind::SHELF);
        let id_a = SongId::new(SourceKind::NETEASE, "7");
        let id_b = SongId::new(SourceKind::SHELF, "7");
        netease
            .kv_set(&id_a, "plugin.x", &StoreValue::Int(1))
            .await?;
        local.kv_set(&id_b, "plugin.x", &StoreValue::Int(2)).await?;
        assert_eq!(netease.kv_get(&id_a, "plugin.x").await?, StoreValue::Int(1));
        assert_eq!(local.kv_get(&id_b, "plugin.x").await?, StoreValue::Int(2));
        Ok(())
    }

    #[tokio::test]
    async fn kv_inc_starts_from_zero_and_accumulates() -> color_eyre::Result<()> {
        let (_dir, s) = open_scope().await?;
        let id = SongId::new(SourceKind::NETEASE, "9");
        assert_eq!(
            s.kv_inc(&id, "plugin.n", /*delta*/ 3).await?,
            StoreValue::Int(3),
            "不存在的 key 以 delta 起步"
        );
        assert_eq!(
            s.kv_inc(&id, "plugin.n", /*delta*/ -1).await?,
            StoreValue::Int(2)
        );
        // 非整数值拒绝自增
        s.kv_set(&id, "plugin.s", &StoreValue::Text("x".to_owned()))
            .await?;
        assert!(s.kv_inc(&id, "plugin.s", /*delta*/ 1).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn rating_set_query_clear_and_bounds() -> color_eyre::Result<()> {
        let (_dir, s) = open_scope().await?;
        let id = SongId::new(SourceKind::NETEASE, "5");
        assert_eq!(s.query_rating(&id).await?, None, "未评分为 None");
        s.set_rating(&id, Some(4)).await?;
        assert_eq!(s.query_rating(&id).await?, Some(4));
        s.set_rating(&id, /*rating*/ None).await?;
        assert_eq!(s.query_rating(&id).await?, None, "None 清空");
        assert!(s.set_rating(&id, Some(6)).await.is_err(), "越界拒绝");
        Ok(())
    }

    #[tokio::test]
    async fn reserved_keys_rejected_in_open_kv() -> color_eyre::Result<()> {
        let (_dir, s) = open_scope().await?;
        let id = SongId::new(SourceKind::NETEASE, "3");
        for key in super::RESERVED_KEYS {
            assert!(
                s.kv_set(&id, key, &StoreValue::Int(1)).await.is_err(),
                "保留键 {key} 必须拒写"
            );
            assert!(
                s.kv_inc(&id, key, /*delta*/ 1).await.is_err(),
                "保留键 {key} 必须拒自增"
            );
        }
        Ok(())
    }
}

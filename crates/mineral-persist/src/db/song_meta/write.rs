//! 歌曲元数据与艺人集合的事务批量写入。

use color_eyre::eyre::WrapErr;
use mineral_model::{MediaUrl, Song};
use rustc_hash::FxHashMap;
use sea_orm::sea_query::{self, Expr, Func, Iden, OnConflict};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, Set, TransactionTrait};

use crate::db::namespace::NamespaceStore;
use crate::entity::{song_artists, song_meta};

/// 每条 metadata 绑定八个字段，分批控制语句大小与参数数量。
const BATCH_ROWS: usize = 100;

impl NamespaceStore {
    /// upsert 一首歌的元数据(song_meta + 按需重写 song_artists 保序)。
    ///
    /// 降级 ServerStore 下静默 no-op。
    ///
    /// song_meta 是**富化程度不一的投影**的落点(同一首歌可能先由列表投影写入、后被 detail
    /// 投影刷新,反之亦然),统一一条合并规则:**「本次输入缺失该字段 = 无新信息 → 保留旧值」**。
    /// 标量富化字段(alias / album / duration / cover)用 SQL NULL 表达「缺失」,走
    /// `COALESCE(excluded.x, song_meta.x)`;艺人列表用**空 Vec** 表达「缺失」,空则跳过重写、
    /// 保留已存行,非空才按 [`Song::artists`] 顺序整体重写(先删后插,`position` 即下标)。
    /// name 是 NOT NULL 列、每个投影必带,恒以新值为准。
    ///
    /// 空艺人列表判「缺失」而非「权威零艺人」是**领域不变量**:本仓任何 channel 产出的 Song
    /// 都必带艺人(netease `ar` / album_artist_refs 有 primary 兜底;bilibili UP 主映射),
    /// 零艺人只会是投影没带、不会是歌真没有。故与 `put_playlist_cache`「空曲目列表清空歌单」
    /// 相反并不矛盾:歌单曲目是权威可编辑的,song 的艺人不是。
    ///
    /// # Params:
    ///   - `song`: 待写入的歌曲元数据
    ///
    /// # Return:
    ///   成功返回 `Ok(())`;降级时同样 `Ok(())`。
    pub async fn upsert_meta(&self, song: &Song) -> color_eyre::Result<()> {
        self.upsert_meta_batch(&[song]).await
    }

    /// 在一个事务内按输入顺序合并多首歌的元数据；失败回滚整批。
    ///
    /// 合并规则与单首写入一致：空字段保留旧值，非空艺人集合整体替换。
    /// 同一歌曲重复出现时，歌名取最后一次，艺人取最后一个非空集合。
    ///
    /// # Params:
    ///   - `songs`: 本 namespace 内的歌曲投影，借用以免批量写入前复制歌曲
    ///
    /// # Return:
    ///   成功或禁用存储时返回 `Ok(())`；写入失败返回带来源上下文的错误。
    pub async fn upsert_meta_batch(&self, songs: &[&Song]) -> color_eyre::Result<()> {
        let Some(pool) = self.pool().filter(|_| !songs.is_empty()) else {
            return Ok(());
        };
        let started = std::time::Instant::now();
        let ns = self.namespace();
        let tx = pool.begin().await.wrap_err("开启批量元数据事务失败")?;
        for batch in songs.chunks(BATCH_ROWS) {
            write_metadata(&tx, ns, batch).await?;
            replace_artists(&tx, ns, batch).await?;
        }
        tx.commit()
            .await
            .wrap_err_with(|| format!("提交批量元数据事务失败 source={ns}"))?;
        mineral_log::debug!(target: "persist", source = ns, songs = songs.len(),
            elapsed_ms = started.elapsed().as_millis(), "stored song metadata batch");
        Ok(())
    }
}

/// 冲突更新中本次待写入的记录。
#[derive(Iden)]
struct Excluded;

/// 一条语句合并一组元数据；同一身份按输入顺序应用富化规则。
async fn write_metadata(
    connection: &impl ConnectionTrait,
    ns: &str,
    songs: &[&Song],
) -> color_eyre::Result<()> {
    let models = songs
        .iter()
        .map(|song| {
            Ok(song_meta::ActiveModel {
                namespace: Set(ns.to_owned()),
                song_value: Set(song.id.value().to_owned()),
                name: Set(song.name.clone()),
                alias: Set(song.alias.clone()),
                album_id: Set(song.album.as_ref().map(|album| album.id.value().to_owned())),
                album_name: Set(song.album.as_ref().map(|album| album.name.clone())),
                duration_ms: Set(song
                    .duration_ms
                    .map(i64::try_from)
                    .transpose()
                    .wrap_err_with(|| {
                        format!("歌曲时长不能存入 SQLite song={}", song.id.qualified())
                    })?),
                cover_url: Set(song.cover_url.as_ref().map(MediaUrl::to_string)),
            })
        })
        .collect::<color_eyre::Result<Vec<_>>>()?;
    let mut merge =
        OnConflict::columns([song_meta::Column::Namespace, song_meta::Column::SongValue]);
    merge.update_column(song_meta::Column::Name);
    for column in [
        song_meta::Column::Alias,
        song_meta::Column::AlbumId,
        song_meta::Column::AlbumName,
        song_meta::Column::DurationMs,
        song_meta::Column::CoverUrl,
    ] {
        merge.value(
            column,
            Func::coalesce([
                Expr::col((Excluded, column)),
                Expr::col((song_meta::Entity, column)),
            ]),
        );
    }
    song_meta::Entity::insert_many(models)
        .on_conflict(merge)
        .exec_without_returning(connection)
        .await
        .wrap_err_with(|| format!("批量合并 song_meta 失败 source={ns}"))?;
    Ok(())
}

/// 同批内同一歌曲只替换一次艺人集合，保留最后一个非空投影。
async fn replace_artists(
    connection: &impl ConnectionTrait,
    ns: &str,
    songs: &[&Song],
) -> color_eyre::Result<()> {
    let mut replacements = FxHashMap::default();
    for &song in songs {
        if !song.artists.is_empty() {
            replacements.insert(song.id.value(), song);
        }
    }
    if replacements.is_empty() {
        return Ok(());
    }
    song_artists::Entity::delete_many()
        .filter(song_artists::Column::Namespace.eq(ns))
        .filter(song_artists::Column::SongValue.is_in(replacements.keys().copied()))
        .exec(connection)
        .await
        .wrap_err_with(|| format!("批量清空 song_artists 失败 source={ns}"))?;
    let mut artists = Vec::new();
    for (id, song) in replacements {
        for (index, artist) in song.artists.iter().enumerate() {
            artists.push(song_artists::ActiveModel {
                namespace: Set(ns.to_owned()),
                song_value: Set(id.to_owned()),
                position: Set(i64::try_from(index)?),
                artist_id: Set(artist.id.value().to_owned()),
                artist_name: Set(artist.name.clone()),
            });
        }
    }
    let batch_count = artists.len().div_ceil(BATCH_ROWS);
    let mut artists = artists.into_iter();
    for _ in 0..batch_count {
        song_artists::Entity::insert_many(artists.by_ref().take(BATCH_ROWS))
            .exec_without_returning(connection)
            .await
            .wrap_err_with(|| format!("批量写入 song_artists 失败 source={ns}"))?;
    }
    Ok(())
}

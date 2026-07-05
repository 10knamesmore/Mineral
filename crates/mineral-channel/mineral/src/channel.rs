//! 聚合 channel 实现:数据全部来自 persist,无网络后端。

use async_trait::async_trait;
use mineral_channel_core::{ChannelCaps, Error, MusicChannel, Page, Result, SearchHits};
use mineral_model::{BitRate, PlayUrl, Playlist, PlaylistId, Song, SongId, SourceKind};
use mineral_persist::ServerStore;

/// `mineral:favorites` 歌单 id——本 channel 唯一一张 synthetic 歌单。
///
/// # Return:
///   聚合收藏歌单的 [`PlaylistId`]。
pub fn favorites_playlist_id() -> PlaylistId {
    PlaylistId::new(SourceKind::MINERAL, "favorites")
}

/// 跨源聚合 channel:source 为 [`SourceKind::MINERAL`],把 persist 的全源收藏
/// 投影成一张 `Favorites` 歌单。
///
/// 搜索 / 详情 / 取流一律 `NotSupported`——歌单里每首歌的 id 保留**原源** namespace,
/// 播放与详情由调度层按 id 路由回真实 channel,本 channel 不会收到这些调用。
/// 收藏的写入也不经它(favorites 编排直写 persist),它是纯只读投影。
pub struct MineralChannel {
    /// persist 句柄(loved + meta 的事实来源)。
    store: ServerStore,
}

impl MineralChannel {
    /// 新建聚合 channel。
    ///
    /// # Params:
    ///   - `store`: server 拥有的 persist 句柄
    ///
    /// # Return:
    ///   聚合 channel 实例。
    pub fn new(store: ServerStore) -> Self {
        Self { store }
    }

    /// 从 persist 重建聚合收藏歌单(name `Favorites`,曲目按收藏时间降序)。
    ///
    /// `track_count` 与曲目**同口径**:都只计 join 到 meta 的收藏(缺 meta 的行
    /// persist 层已跳过),sidebar 计数不会和实际曲目数打架。
    ///
    /// count-only 路径走 `loved_count`(一条 COUNT),不为拿个数字重建整个 `Vec<Song>`;
    /// 只有 `with_songs` 时才拉全曲目。
    ///
    /// # Params:
    ///   - `with_songs`: `false` 只出计数(歌单列表用,省 IPC 载荷 + 省 DB 重建),`true` 带全曲目
    ///
    /// # Return:
    ///   聚合收藏歌单。
    async fn build_favorites(&self, with_songs: bool) -> Result<Playlist> {
        let (track_count, songs) = if with_songs {
            let songs = self.store.loved_songs().await.map_err(Error::Other)?;
            let count = u64::try_from(songs.len())
                .map_err(|e| Error::Other(color_eyre::Report::new(e)))?;
            (count, songs)
        } else {
            let count = self.store.loved_count().await.map_err(Error::Other)?;
            (count, Vec::new())
        };
        Ok(Playlist::builder()
            .id(favorites_playlist_id())
            .name("Favorites".to_owned())
            .track_count(track_count)
            .songs(songs)
            .build())
    }
}

#[async_trait]
impl MusicChannel for MineralChannel {
    fn source(&self) -> SourceKind {
        SourceKind::MINERAL
    }

    fn caps(&self) -> ChannelCaps {
        ChannelCaps::builder()
            .searchable(Vec::new())
            .playlist_edit(false)
            .build()
    }

    async fn search_songs(&self, _query: &str, _page: Page) -> Result<SearchHits<Song>> {
        Err(Error::NotSupported)
    }

    async fn songs_detail(&self, _ids: &[SongId]) -> Result<Vec<Song>> {
        Err(Error::NotSupported)
    }

    async fn song_urls(&self, _ids: &[SongId], _quality: BitRate) -> Result<Vec<PlayUrl>> {
        Err(Error::NotSupported)
    }

    async fn my_playlists(&self) -> Result<Vec<Playlist>> {
        Ok(vec![self.build_favorites(/*with_songs*/ false).await?])
    }

    async fn playlist_detail(&self, id: &PlaylistId) -> Result<Playlist> {
        if *id != favorites_playlist_id() {
            return Err(Error::NotSupported);
        }
        self.build_favorites(/*with_songs*/ true).await
    }
}

#[cfg(test)]
mod tests {
    use mineral_channel_core::{Error, MusicChannel};
    use mineral_model::{SongId, SourceKind};
    use mineral_persist::ServerStore;
    use mineral_test::{song, with_name};

    use super::{MineralChannel, favorites_playlist_id};

    /// 造一个含两源收藏的 store:netease「Palisade」+ bilibili「夜間飛行」,
    /// 外加一条 loved 但无 meta 的幽灵行(应被跳过)。TempDir 须由调用方持有到测试尾。
    async fn store_with_favorites() -> color_eyre::Result<(tempfile::TempDir, ServerStore)> {
        let dir = tempfile::tempdir()?;
        let store = ServerStore::open(&dir.path().join("t.db")).await?;
        let netease = store.scope(SourceKind::NETEASE);
        let n1 = with_name(song("n1"), "Palisade");
        netease.upsert_meta(&n1).await?;
        netease.set_loved(&n1.id, true).await?;
        let bilibili = store.scope(SourceKind::BILIBILI);
        let mut b1 = with_name(song("b1"), "夜間飛行");
        b1.id = SongId::new(SourceKind::BILIBILI, "b1");
        bilibili.upsert_meta(&b1).await?;
        bilibili.set_loved(&b1.id, true).await?;
        netease
            .set_loved(&SongId::new(SourceKind::NETEASE, "ghost"), true)
            .await?;
        Ok((dir, store))
    }

    /// my_playlists:恰好一张 synthetic 歌单,id/name 固定,track_count 只计
    /// join 到 meta 的收藏(幽灵行不计),songs 空(列表面不带载荷)。
    #[tokio::test]
    async fn my_playlists_is_single_synthetic() -> color_eyre::Result<()> {
        let (_dir, store) = store_with_favorites().await?;
        let ch = MineralChannel::new(store);
        let lists = ch.my_playlists().await?;
        assert_eq!(lists.len(), 1);
        let p = lists
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有一张歌单"))?;
        assert_eq!(p.id, favorites_playlist_id());
        assert_eq!(p.name, "Favorites");
        assert_eq!(p.track_count, 2, "幽灵行(无 meta)不计入");
        assert!(p.songs.is_empty(), "列表面不带曲目载荷");
        Ok(())
    }

    /// playlist_detail:favorites id 出全曲目(收藏时间降序,源 namespace 保留);
    /// 其他 id 一律 NotSupported。
    #[tokio::test]
    async fn playlist_detail_aggregates_and_rejects_unknown() -> color_eyre::Result<()> {
        let (_dir, store) = store_with_favorites().await?;
        let ch = MineralChannel::new(store);
        let p = ch.playlist_detail(&favorites_playlist_id()).await?;
        assert_eq!(p.track_count, 2);
        assert_eq!(p.songs.len(), 2, "detail 带全曲目,与 track_count 同口径");
        let sources = p
            .songs
            .iter()
            .map(mineral_model::Song::source)
            .collect::<Vec<SourceKind>>();
        assert!(
            sources.contains(&SourceKind::NETEASE) && sources.contains(&SourceKind::BILIBILI),
            "曲目保留原源 namespace(播放据此路由回真实 channel)"
        );

        let other = mineral_model::PlaylistId::new(SourceKind::MINERAL, "nope");
        assert!(
            matches!(ch.playlist_detail(&other).await, Err(Error::NotSupported)),
            "未知 id 不臆造歌单"
        );
        Ok(())
    }

    /// 降级 store(无 pool):歌单仍在,只是空——聚合视图不因 persist 降级而消失。
    #[tokio::test]
    async fn disabled_store_yields_empty_favorites() -> color_eyre::Result<()> {
        let ch = MineralChannel::new(ServerStore::disabled());
        let p = ch.playlist_detail(&favorites_playlist_id()).await?;
        assert_eq!(p.track_count, 0);
        assert!(p.songs.is_empty());
        Ok(())
    }

    /// source 报 MINERAL(id namespace / 徽标 / 任务路由都靠它)。
    #[test]
    fn source_is_mineral() {
        let ch = MineralChannel::new(ServerStore::disabled());
        assert_eq!(ch.source(), SourceKind::MINERAL);
    }
}

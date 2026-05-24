//! proptest 生成器(strategy):造随机但合法的 model 值,供属性测试复用。
//!
//! 策略写在这里、不给生产类型 `#[derive(Arbitrary)]`,以免污染 `mineral-model`。

use mineral_model::{AlbumId, AlbumRef, ArtistId, ArtistRef, Song, SongId, SourceKind};
use proptest::collection::vec;
use proptest::option;
use proptest::prelude::{Just, Strategy, any, prop_oneof};

/// 随机 `Song` 生成器:覆盖各来源、空/非空艺人、有无专辑、任意时长。
///
/// 不生成 `cover_url` / `source_url`(对 codec / 序列化往返不增信息;需要时另写策略)。
///
/// # Return:
///   产出合法 `Song` 的 proptest [`Strategy`]。
pub fn arb_song() -> impl Strategy<Value = Song> {
    (
        prop_oneof![Just(SourceKind::Netease), Just(SourceKind::Local)],
        any::<String>(),
        any::<String>(),
        vec(arb_artist(), 0..3),
        option::of(any::<String>()),
        any::<u64>(),
    )
        .prop_map(|(source, id, name, artists, album, duration_ms)| Song {
            source,
            id: SongId::from(id.as_str()),
            name,
            artists,
            album: album.map(|n| AlbumRef {
                id: AlbumId::from(n.as_str()),
                name: n,
            }),
            duration_ms,
            cover_url: None,
            source_url: None,
        })
}

/// 随机 `ArtistRef` 生成器(`id` 由名字派生)。
///
/// # Return:
///   产出 `ArtistRef` 的 proptest [`Strategy`]。
fn arb_artist() -> impl Strategy<Value = ArtistRef> {
    any::<String>().prop_map(|name| ArtistRef {
        id: ArtistId::from(name.as_str()),
        name,
    })
}

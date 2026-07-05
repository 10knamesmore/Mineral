//! proptest 生成器(strategy):造随机但合法的 model 值,供属性测试复用。
//!
//! 策略写在这里、不给生产类型 `#[derive(Arbitrary)]`,以免污染 `mineral-model`。

use mineral_model::{AlbumId, AlbumRef, ArtistId, ArtistRef, Song, SongId, SourceKind};
use proptest::collection::vec;
use proptest::option;
use proptest::prelude::{Just, Strategy, any, prop_oneof};

/// 随机 `Song` 生成器:覆盖各来源、空/非空艺人、有无专辑、任意时长(含未知)。
///
/// 同一首歌的 song / artist / album id 共用所选 `source` 作 namespace,
/// 故 `song.source() == song.id.namespace()` 恒成立(可被属性测试当不变量守护)。
/// 不生成 `cover_url` / `source_url`(对 codec / 序列化往返不增信息;需要时另写策略)。
///
/// # Return:
///   产出合法 `Song` 的 proptest [`Strategy`]。
pub fn arb_song() -> impl Strategy<Value = Song> {
    (
        prop_oneof![Just(SourceKind::NETEASE), Just(SourceKind::LOCAL)],
        any::<String>(),
        any::<String>(),
        vec(any::<String>(), 0..3),
        option::of(any::<String>()),
        option::of(any::<u64>()),
    )
        .prop_map(|(source, id, name, artist_names, album, duration_ms)| {
            Song::builder()
                .id(SongId::new(source, id))
                .name(name)
                .artists(
                    artist_names
                        .into_iter()
                        .map(|n| ArtistRef {
                            id: ArtistId::new(source, n.as_str()),
                            name: n,
                        })
                        .collect(),
                )
                .album(album.map(|n| AlbumRef {
                    id: AlbumId::new(source, n.as_str()),
                    name: n,
                }))
                .duration_ms(duration_ms)
                .build()
        })
}

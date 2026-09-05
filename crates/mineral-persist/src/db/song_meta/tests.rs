//! 元数据合并与读取的业务契约。

use mineral_model::{ArtistId, ArtistRef, Song, SongId, SourceKind};

/// 批量查询只返回请求集合，跨批次、重复 ID、缺失行与来源隔离保持正确。
#[tokio::test]
async fn batch_reads_requested_metadata_with_ordered_artists() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = crate::ServerStore::open(&dir.path().join("batch.db")).await?;
    let scope = store.scope(SourceKind::NETEASE);
    let songs = (0..450)
        .map(|index| {
            mineral_test::with_artists(mineral_test::song(&index.to_string()), &["主唱", "客串"])
        })
        .collect::<Vec<_>>();
    scope
        .upsert_meta_batch(&songs.iter().collect::<Vec<_>>())
        .await?;
    scope
        .upsert_meta(&mineral_test::song("unrequested"))
        .await?;
    store
        .scope(SourceKind::BILIBILI)
        .upsert_meta(&mineral_test::with_name(
            mineral_test::with_source(mineral_test::song("0"), SourceKind::BILIBILI),
            "另一个来源",
        ))
        .await?;
    let mut ids = songs.iter().map(|song| song.id.clone()).collect::<Vec<_>>();
    ids.push(SongId::new(SourceKind::NETEASE, "0"));
    ids.push(SongId::new(SourceKind::NETEASE, "missing"));
    let got = scope.get_meta_batch(&ids).await?;
    assert_eq!(got.len(), songs.len());
    for song in &songs {
        assert_eq!(got.get(&song.id), Some(song), "艺人及其顺序和来源都应保持");
    }
    assert!(scope.get_meta_batch(&[]).await?.is_empty());
    assert!(
        crate::ServerStore::disabled()
            .scope(SourceKind::NETEASE)
            .get_meta_batch(&ids)
            .await?
            .is_empty()
    );
    Ok(())
}

/// 同批重复投影按原顺序富化，空艺人保留最近一次非空集合。
#[tokio::test]
async fn batch_duplicate_songs_keep_latest_nonempty_fields() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = crate::ServerStore::open(&dir.path().join("batch.db")).await?;
    let scope = store.scope(SourceKind::NETEASE);
    let rich = mineral_test::with_artists(
        mineral_test::with_alias(mineral_test::song("1"), "译名"),
        &["旧艺人"],
    );
    let revised = mineral_test::with_artists(mineral_test::song("1"), &["主唱", "客串"]);
    let poor = mineral_test::with_name(mineral_test::song("1"), "新歌名");
    scope.upsert_meta_batch(&[&rich, &revised, &poor]).await?;
    let expected = mineral_test::with_name(mineral_test::with_alias(revised, "译名"), "新歌名");
    assert_eq!(scope.get_meta(&rich.id).await?, Some(expected));
    Ok(())
}

/// 后续分块失败必须回滚前面的写入，不发布半份 metadata 批次。
#[tokio::test]
async fn failed_metadata_batch_rolls_back_earlier_rows() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = crate::ServerStore::open(&dir.path().join("batch.db")).await?;
    let scope = store.scope(SourceKind::NETEASE);
    let songs = (0..120)
        .map(|index| mineral_test::song(&index.to_string()))
        .collect::<Vec<_>>();
    let invalid = mineral_test::with_duration(mineral_test::song("invalid"), u64::MAX);
    let mut batch = songs.iter().collect::<Vec<_>>();
    batch.push(&invalid);
    assert!(scope.upsert_meta_batch(&batch).await.is_err());
    assert!(scope.list_meta().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn upsert_meta_then_get_roundtrips() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
    let s = p.scope(SourceKind::NETEASE);
    let song = Song::builder()
        .id(SongId::new(SourceKind::NETEASE, "123"))
        .name("迷跡波".to_owned())
        .artists(vec![ArtistRef {
            id: ArtistId::new(SourceKind::NETEASE, "a1"),
            name: "演者".to_owned(),
        }])
        .duration_ms(Some(200_000))
        .build();
    s.upsert_meta(&song).await?;
    let got = s.get_meta(&song.id).await?;
    assert!(got.is_some());
    if let Some(g) = got {
        assert_eq!(g.name, "迷跡波");
        assert_eq!(g.artists.len(), song.artists.len());
        assert_eq!(g.duration_ms, Some(200_000));
    }
    Ok(())
}

/// list_meta:枚举全量并按 song_artists 保序重建艺人;不同 namespace 互不漏;降级为空。
#[tokio::test]
async fn list_meta_returns_all_with_ordered_artists() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
    let s = p.scope(SourceKind::NETEASE);
    let mk = |v: &str, name: &str, artists: Vec<ArtistRef>| {
        Song::builder()
            .id(SongId::new(SourceKind::NETEASE, v))
            .name(name.to_owned())
            .artists(artists)
            .build()
    };
    let a = |id: &str, name: &str| ArtistRef {
        id: ArtistId::new(SourceKind::NETEASE, id),
        name: name.to_owned(),
    };
    s.upsert_meta(&mk("1", "晴天", vec![a("j", "周杰伦"), a("x", "第二艺人")]))
        .await?;
    s.upsert_meta(&mk("2", "无艺人歌", Vec::new())).await?;
    // 另一 namespace 的歌不应混入。
    p.scope(SourceKind::BILIBILI)
        .upsert_meta(
            &Song::builder()
                .id(SongId::new(SourceKind::BILIBILI, "BV1"))
                .name("B站歌".to_owned())
                .build(),
        )
        .await?;

    let all = s.list_meta().await?;
    assert_eq!(all.len(), 2, "本 namespace 应只有两首: {all:?}");
    let Some(one) = all.iter().find(|x| x.id.value() == "1") else {
        return Err(color_eyre::eyre::eyre!("应含 id=1"));
    };
    let names = one
        .artists
        .iter()
        .map(|x| x.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["周杰伦", "第二艺人"], "艺人应保序");
    Ok(())
}

/// 可空富化字段「非空进步、NULL 不回退」:贫投影(alias/duration/cover/album 全缺、
/// 艺人空)后到,不得抹掉先前富投影写入的值;name 非空列恒以新值为准。
#[tokio::test]
async fn upsert_meta_null_fields_do_not_regress() -> color_eyre::Result<()> {
    use mineral_model::{AlbumId, AlbumRef, MediaUrl};

    let dir = tempfile::tempdir()?;
    let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
    let s = p.scope(SourceKind::NETEASE);
    let id = SongId::new(SourceKind::NETEASE, "42");

    let rich = Song::builder()
        .id(id.clone())
        .name("ButterFly".to_owned())
        .alias(Some("黄油飞".to_owned()))
        .artists(vec![ArtistRef {
            id: ArtistId::new(SourceKind::NETEASE, "a1"),
            name: "和田光司".to_owned(),
        }])
        .album(Some(AlbumRef {
            id: AlbumId::new(SourceKind::NETEASE, "al1"),
            name: "数码宝贝".to_owned(),
        }))
        .duration_ms(Some(259_000))
        .cover_url(Some(MediaUrl::remote("https://p1.example/c.jpg")?))
        .build();
    s.upsert_meta(&rich).await?;

    // 贫投影:除 name 外全缺。
    let poor = Song::builder()
        .id(id.clone())
        .name("Butter-Fly".to_owned())
        .build();
    s.upsert_meta(&poor).await?;

    let got = s
        .get_meta(&id)
        .await?
        .ok_or_else(|| color_eyre::eyre::eyre!("应命中 meta"))?;
    assert_eq!(got.name, "Butter-Fly", "name 恒以新值为准");
    assert_eq!(
        got.alias.as_deref(),
        Some("黄油飞"),
        "alias 不得被 NULL 回退"
    );
    assert_eq!(got.duration_ms, Some(259_000), "duration 不得被 NULL 回退");
    assert!(got.cover_url.is_some(), "cover 不得被 NULL 回退");
    assert!(got.album.is_some(), "album 不得被 NULL 回退");
    assert_eq!(got.artists.len(), 1, "空艺人列表应保留已存行");

    // 后续富投影仍能正常更新非空值(进步方向不受影响)。
    let newer = Song::builder()
        .id(id.clone())
        .name("Butter-Fly".to_owned())
        .alias(Some("黄油飞(数码宝贝OP)".to_owned()))
        .duration_ms(Some(260_000))
        .build();
    s.upsert_meta(&newer).await?;
    let got2 = s
        .get_meta(&id)
        .await?
        .ok_or_else(|| color_eyre::eyre::eyre!("应命中 meta"))?;
    assert_eq!(got2.alias.as_deref(), Some("黄油飞(数码宝贝OP)"));
    assert_eq!(got2.duration_ms, Some(260_000));
    Ok(())
}

#[tokio::test]
async fn upsert_meta_preserves_artist_order_and_clears_stale() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
    let s = p.scope(SourceKind::NETEASE);
    let id = SongId::new(SourceKind::NETEASE, "777");

    // 首次 upsert:3 个艺人,验证保序 + 内容
    let song = Song::builder()
        .id(id.clone())
        .name("多人合作".to_owned())
        .artists(vec![
            ArtistRef {
                id: ArtistId::new(SourceKind::NETEASE, "a1"),
                name: "甲".to_owned(),
            },
            ArtistRef {
                id: ArtistId::new(SourceKind::NETEASE, "a2"),
                name: "乙".to_owned(),
            },
            ArtistRef {
                id: ArtistId::new(SourceKind::NETEASE, "a3"),
                name: "丙".to_owned(),
            },
        ])
        .duration_ms(Some(123_000))
        .build();
    s.upsert_meta(&song).await?;

    let got = s.get_meta(&id).await?;
    assert!(got.is_some());
    if let Some(g) = got {
        let names = g
            .artists
            .iter()
            .map(|a| a.name.clone())
            .collect::<Vec<String>>();
        assert_eq!(
            names,
            vec!["甲".to_owned(), "乙".to_owned(), "丙".to_owned()]
        );
    }

    // 再次 upsert 同 id:换成 1 个不同艺人,验证旧 3 行被清
    let updated = Song::builder()
        .id(id.clone())
        .name("改为单人".to_owned())
        .artists(vec![ArtistRef {
            id: ArtistId::new(SourceKind::NETEASE, "b1"),
            name: "丁".to_owned(),
        }])
        .duration_ms(Some(99_000))
        .build();
    s.upsert_meta(&updated).await?;

    let got2 = s.get_meta(&id).await?;
    assert!(got2.is_some());
    if let Some(g) = got2 {
        assert_eq!(g.name, "改为单人");
        let names = g
            .artists
            .iter()
            .map(|a| a.name.clone())
            .collect::<Vec<String>>();
        assert_eq!(names, vec!["丁".to_owned()]);
    }
    Ok(())
}

#[tokio::test]
async fn get_meta_miss_returns_none() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
    let s = p.scope(SourceKind::NETEASE);
    assert!(
        s.get_meta(&SongId::new(SourceKind::NETEASE, "nope"))
            .await?
            .is_none()
    );
    Ok(())
}

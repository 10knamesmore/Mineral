//! before_stream / prefetch hook 改写 · skip · 无版权跨源补救。

use super::*;
use pretty_assertions::assert_eq;

/// before_stream 改写:hook 返回 {url, quality} → 起播用改写值、play_url 回填改写值。
#[tokio::test]
async fn before_stream_rewrite_replaces_url_and_quality() -> color_eyre::Result<()> {
    let (core, runtime) = core_with_script(
        r#"
            mineral.hook("before_stream", function(ctx)
                return { url = "https://fallback.example/b.flac", quality = "standard" }
            end)
            "#,
    )?;
    core.with_state(|st| st.current_song = Some(song("a")));
    crate::hook_bridge::before_stream(&core, &song("a"), test_play_url("a")?);
    let rewritten = wait_until(|| {
        core.with_state(|st| {
            st.play_url
                .as_ref()
                .is_some_and(|pu| pu.url.to_string() == "https://fallback.example/b.flac")
        })
    })
    .await;
    assert!(rewritten, "play_url 应回填改写后的 URL");
    core.with_state(|st| {
        assert_eq!(
            st.play_url.as_ref().map(|pu| pu.quality),
            Some(BitRate::Standard),
            "音质应一并改写"
        );
        assert_eq!(
            st.play_url.as_ref().map(|pu| pu.substituted),
            Some(true),
            "顶换过 URL 的流必须带 substituted 标记(歌词降级依据)"
        );
    });
    drop(runtime);
    Ok(())
}

/// before_stream 改写带 headers:hook 返回 {url, headers} → play_url 回填的 stream_headers 含该头。
/// 改写顶替进来的 B站 url 必须带 `Referer`,否则 403(header 穿透 rewrite→play)。
#[tokio::test]
async fn before_stream_rewrite_carries_stream_headers() -> color_eyre::Result<()> {
    let (core, runtime) = core_with_script(
        r#"
            mineral.hook("before_stream", function(ctx)
                return {
                    url = "https://fallback.example/b.flac",
                    headers = { {"Referer", "https://www.bilibili.com"} },
                }
            end)
            "#,
    )?;
    core.with_state(|st| st.current_song = Some(song("a")));
    crate::hook_bridge::before_stream(&core, &song("a"), test_play_url("a")?);
    let carried = wait_until(|| {
        core.with_state(|st| {
            st.play_url.as_ref().is_some_and(|pu| {
                pu.stream_headers
                    == vec![("Referer".to_owned(), "https://www.bilibili.com".to_owned())]
            })
        })
    })
    .await;
    assert!(
        carried,
        "play_url.stream_headers 应带上 hook 返回的 Referer"
    );
    drop(runtime);
    Ok(())
}

/// before_stream 改写 URL 但未指定 layout → effective.layout 默认 `Chunked`(改写目标常是分片流,
/// 流式打开避免起播 stall)。回归:曾直接继承原曲 layout(网易云 `Contiguous`),改写成 B站
/// fMP4 后被 seekable 全扫、起播卡数秒。
#[tokio::test]
async fn before_stream_rewrite_url_defaults_chunked_layout() -> color_eyre::Result<()> {
    let (core, runtime) = core_with_script(
        r#"
            mineral.hook("before_stream", function(ctx)
                return { url = "https://fallback.example/audio.m4s" }
            end)
            "#,
    )?;
    core.with_state(|st| st.current_song = Some(song("a")));
    crate::hook_bridge::before_stream(&core, &song("a"), test_play_url("a")?);
    let chunked = wait_until(|| {
        core.with_state(|st| {
            st.play_url
                .as_ref()
                .is_some_and(|pu| pu.layout == mineral_model::StreamLayout::Chunked)
        })
    })
    .await;
    assert!(chunked, "改写 URL 后 layout 应默认 Chunked");
    drop(runtime);
    Ok(())
}

/// before_stream 改写时脚本显式 `layout = "contiguous"` → effective.layout 用该值(压过默认 Chunked),
/// 给「改写成直链源」的脚本一个恢复 seekable 的出口。
#[tokio::test]
async fn before_stream_rewrite_explicit_layout_overrides_default() -> color_eyre::Result<()> {
    let (core, runtime) = core_with_script(
        r#"
            mineral.hook("before_stream", function(ctx)
                return { url = "https://fallback.example/direct.mp3", layout = "contiguous" }
            end)
            "#,
    )?;
    core.with_state(|st| st.current_song = Some(song("a")));
    crate::hook_bridge::before_stream(&core, &song("a"), test_play_url("a")?);
    let contiguous = wait_until(|| {
        core.with_state(|st| {
            st.play_url
                .as_ref()
                .is_some_and(|pu| pu.layout == mineral_model::StreamLayout::Contiguous)
        })
    })
    .await;
    assert!(contiguous, "显式 layout=contiguous 应压过默认 Chunked");
    drop(runtime);
    Ok(())
}

/// before_stream 跳过:hook 返回 false → 不起播本曲,推进到下一首。
#[tokio::test]
async fn before_stream_skip_advances_to_next() -> color_eyre::Result<()> {
    let (core, runtime) = core_with_script(
        r#"
            local skipped = false
            mineral.hook("before_stream", function(ctx)
                -- 只跳第一次(下一首放行,避免连锁)
                if not skipped then
                    skipped = true
                    return false
                end
            end)
            "#,
    )?;
    core.with_state(|st| {
        st.queue = vec![song("a"), song("b")];
        st.queue_sel = 0;
        st.current_song = Some(song("a"));
    });
    crate::hook_bridge::before_stream(&core, &song("a"), test_play_url("a")?);
    let advanced = wait_until(|| {
        core.with_state(|st| {
            st.current_song
                .as_ref()
                .is_some_and(|s| s.id.as_str() == "b")
        })
    })
    .await;
    assert!(advanced, "skip 后应推进到下一首");
    drop(runtime);
    Ok(())
}

/// 预取提交点放行:裁决回来后按原 URL 武装(queued 登记原值)。
#[tokio::test]
async fn prefetch_hook_continue_arms_original() -> color_eyre::Result<()> {
    let (core, runtime) = core_with_script(
        r#"
            mineral.hook("before_stream", function(ctx) return nil end)
            "#,
    )?;
    let next = song("b");
    core.with_state(|st| {
        st.queue = vec![song("a"), song("b")];
        st.queue_sel = 0;
        st.current_song = Some(song("a"));
        st.prefetch_fired_for = Some(song("b").id);
    });
    let original = test_play_url("b")?;
    let want = original.url.to_string();
    crate::hook_bridge::on_prefetch_ready(&core, &next.id, original);
    let armed = wait_until(|| {
        core.with_state(|st| {
            st.queued
                .as_ref()
                .and_then(|q| q.play_url.as_ref())
                .is_some_and(|pu| pu.url.to_string() == want)
        })
    })
    .await;
    assert!(armed, "放行后应按原 URL 登记预排");
    drop(runtime);
    Ok(())
}

/// 预取提交点改写:武装改写流(URL + 取流头),且**不 capture**。
#[tokio::test]
async fn prefetch_hook_rewrite_arms_effective() -> color_eyre::Result<()> {
    let (core, runtime) = core_with_script(
        r#"
            mineral.hook("before_stream", function(ctx)
                if ctx.mode == "prefetch" then
                    return { url = "https://fallback.example/b.m4s",
                             headers = {{"Referer", "https://www.bilibili.com"}} }
                end
            end)
            "#,
    )?;
    let next = song("b");
    core.with_state(|st| {
        st.queue = vec![song("a"), song("b")];
        st.queue_sel = 0;
        st.current_song = Some(song("a"));
        st.prefetch_fired_for = Some(song("b").id);
    });
    crate::hook_bridge::on_prefetch_ready(&core, &next.id, test_play_url("b")?);
    let armed = wait_until(|| {
        core.with_state(|st| {
            st.queued
                .as_ref()
                .and_then(|q| q.play_url.as_ref())
                .is_some_and(|pu| pu.url.to_string() == "https://fallback.example/b.m4s")
        })
    })
    .await;
    assert!(armed, "改写后应按 effective 登记预排");
    core.with_state(|st| {
        let queued = st.queued.as_ref();
        assert!(
            queued.is_some_and(|q| q.capturing.is_none()),
            "改写流不 capture 入缓存"
        );
        assert!(
            queued.and_then(|q| q.play_url.as_ref()).is_some_and(|pu| pu
                .stream_headers
                .contains(&("Referer".to_owned(), "https://www.bilibili.com".to_owned()))),
            "改写流应携带脚本给的取流头"
        );
    });
    drop(runtime);
    Ok(())
}

/// 预取提交点 Skip = 否决:下标进否决集、队列不动、预拉标记复位待重排,不武装。
#[tokio::test]
async fn prefetch_hook_skip_vetoes_next() -> color_eyre::Result<()> {
    let (core, runtime) = core_with_script(
        r#"
            mineral.hook("before_stream", function(ctx)
                return { skip = "灰歌无替身" }
            end)
            "#,
    )?;
    let next = song("b");
    core.with_state(|st| {
        st.queue = vec![song("a"), song("b"), song("c")];
        st.queue_sel = 0;
        st.current_song = Some(song("a"));
        st.prefetch_fired_for = Some(song("b").id);
    });
    crate::hook_bridge::on_prefetch_ready(&core, &next.id, test_play_url("b")?);
    let vetoed = wait_until(|| core.with_state(|st| st.prefetch_vetoed == vec![1])).await;
    assert!(vetoed, "b 的下标应进否决集");
    core.with_state(|st| {
        assert!(st.queued.is_none(), "否决不武装");
        assert_eq!(st.prefetch_fired_for, None, "预拉标记应复位待重排");
        assert_eq!(st.queue.len(), 3, "队列纹丝不动");
        // 否决生效后,下一首预测应越过 b 落到 c。
        assert_eq!(crate::queue::next_index(st), Some(2));
    });
    drop(runtime);
    Ok(())
}

/// unplayable(取链失败)+ 改写 = 补救:脚本据 ctx.unplayable 顶入可播流,起播改写值。
#[tokio::test]
async fn unplayable_rewrite_plays_fallback() -> color_eyre::Result<()> {
    let (core, runtime) = core_with_script(
        r#"
            mineral.hook("before_stream", function(ctx)
                if ctx.unplayable then
                    return { url = "https://rescue.example/a.m4s",
                             headers = {{"Referer", "https://www.bilibili.com"}} }
                end
            end)
            "#,
    )?;
    core.with_state(|st| st.current_song = Some(song("a")));
    crate::hook_bridge::on_unplayable_current(&core, &song("a"));
    let rescued = wait_until(|| {
        core.with_state(|st| {
            st.play_url
                .as_ref()
                .is_some_and(|pu| pu.url.to_string() == "https://rescue.example/a.m4s")
        })
    })
    .await;
    assert!(rescued, "补救改写应回填 play_url");
    drop(runtime);
    Ok(())
}

/// unplayable + 放行(脚本帮不上):不起播、不回填 play_url(维持原失败语义,
/// 失败信号经 track_finished("error") 通知)。
#[tokio::test]
async fn unplayable_continue_keeps_failure() -> color_eyre::Result<()> {
    let (core, runtime) = core_with_script(
        r#"
            mineral.hook("before_stream", function(ctx) return nil end)
            "#,
    )?;
    core.with_state(|st| st.current_song = Some(song("a")));
    crate::hook_bridge::on_unplayable_current(&core, &song("a"));
    // 裁决是异步的:留出窗口再断言「什么都没被顶入」。
    tokio::time::sleep(Duration::from_millis(300)).await;
    core.with_state(|st| {
        assert!(st.play_url.is_none(), "放行 = 维持失败,不该凭空出 play_url");
        assert!(
            st.current_song
                .as_ref()
                .is_some_and(|s| s.id.as_str() == "a"),
            "放行不推进队列"
        );
    });
    drop(runtime);
    Ok(())
}

/// before_stream 放行(无 hook 命中):走原 capture 起播路径,play_url 回填原值。
#[tokio::test]
async fn before_stream_continue_keeps_original() -> color_eyre::Result<()> {
    let (core, runtime) = core_with_script("-- 无 hook")?;
    core.with_state(|st| st.current_song = Some(song("a")));
    let original = test_play_url("a")?;
    let want = original.url.to_string();
    crate::hook_bridge::before_stream(&core, &song("a"), original);
    let kept = wait_until(|| {
        core.with_state(|st| {
            st.play_url
                .as_ref()
                .is_some_and(|pu| pu.url.to_string() == want)
        })
    })
    .await;
    assert!(kept, "放行应回填原 URL");
    drop(runtime);
    Ok(())
}

/// before_stream hook 触发即记一条 hook_fires(即便裁决 Continue);真 recorder 写库,
/// 证 hook_bridge 接线产数据。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn before_stream_hook_records_hook_fire() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = mineral_stats::StatsStore::open(&dir.path().join("stats.db")).await?;
    let params = crate::params_from_config(mineral_config::Config::defaults()?.stats());
    let (recorder, _actor) = crate::StatsRecorder::spawn(store.clone(), params);
    let (core, runtime) = core_with_script_stats(
        r#"mineral.hook("before_stream", function(ctx) end)"#, // 无返回 = Continue
        recorder,
    )?;
    core.with_state(|st| st.current_song = Some(song("a")));
    crate::hook_bridge::before_stream(&core, &song("a"), test_play_url("a")?);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while store.status().await?.events < 1 {
        if std::time::Instant::now() > deadline {
            color_eyre::eyre::bail!("超时:hook_fires 未落库");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(runtime);
    Ok(())
}

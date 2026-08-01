-- Favorite membership 是独立 preference relation，不再平铺进播放统计。
CREATE TABLE song_favorites (
    namespace TEXT NOT NULL,
    song_value TEXT NOT NULL,
    entered_at INTEGER NOT NULL,
    PRIMARY KEY (namespace, song_value));

CREATE INDEX idx_song_favorites_order
    ON song_favorites (entered_at DESC, namespace, song_value);

-- 保留老库现有收藏时间；没有 metadata 的 favorite 同样是合法 membership。
INSERT INTO song_favorites (namespace, song_value, entered_at)
SELECT namespace, song_value, loved_at
FROM song_stats
WHERE loved_at IS NOT NULL;

-- SQLite 不支持 DROP COLUMN 的所有目标版本，rebuild 后只保留播放统计事实。
CREATE TABLE song_stats_new (
    namespace TEXT NOT NULL,
    song_value TEXT NOT NULL,
    play_count INTEGER NOT NULL DEFAULT 0,
    skip_count INTEGER NOT NULL DEFAULT 0,
    total_listen_ms INTEGER NOT NULL DEFAULT 0,
    last_played_at INTEGER,
    rating INTEGER,
    PRIMARY KEY (namespace, song_value));

INSERT INTO song_stats_new (
    namespace,
    song_value,
    play_count,
    skip_count,
    total_listen_ms,
    last_played_at,
    rating
)
SELECT
    namespace,
    song_value,
    play_count,
    skip_count,
    total_listen_ms,
    last_played_at,
    rating
FROM song_stats;

DROP TABLE song_stats;
ALTER TABLE song_stats_new RENAME TO song_stats;

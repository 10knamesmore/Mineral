-- 歌曲艺人维表:成员聚合的数据源(「你听了谁的歌」,区别于 plays.context_ref 的
-- 「你从谁的详情页起播」)。多值关系,故不平铺进 songs(单值列才能平铺,见
-- 0002_song_dim.sql 的 album_id/album_name);position 保住「主艺人在前」的顺序。
--
-- 写入语义与 songs 不同:songs 单值列走 COALESCE(非空进步、NULL 不回退),本表多值
-- 只能先删后插——但贫投影(Song.artists 为空)必须整个跳过删插,否则会把已知艺人
-- 抹光(guard 在 upsert_song 里做,不在 schema 层)。
CREATE TABLE song_artists (
    ns           TEXT NOT NULL,
    song_value   TEXT NOT NULL,
    position     INTEGER NOT NULL,
    artist_value TEXT NOT NULL,
    artist_name  TEXT NOT NULL,
    PRIMARY KEY (ns, song_value, position)
);

CREATE INDEX idx_song_artists_artist ON song_artists (ns, artist_value);

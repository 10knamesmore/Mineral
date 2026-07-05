-- baseline:server 库(mineral.db)全量 schema(规范化、无 JSON 列)。
-- 刻意用裸 CREATE TABLE:建于迁移机制之前的老库(表已存在、无迁移记账)在这里
-- 响亮撞错,由 ensure_schema 的错误指引用户 `mineral cache reset` 重建,
-- 而不是 IF NOT EXISTS 静默收编一个结构过时的库、把错误推迟到运行时。

CREATE TABLE song_meta (
    namespace TEXT NOT NULL,
    song_value TEXT NOT NULL,
    name TEXT NOT NULL,
    album_id TEXT,
    album_name TEXT,
    -- 时长毫秒;NULL = 未知(与「真的 0 ms」区分,别用 0 冒充)。
    duration_ms INTEGER,
    cover_url TEXT,
    PRIMARY KEY (namespace, song_value));

-- 艺人行随 song_meta 同事务「先删后插」维护;FK 级联守住孤儿行。
CREATE TABLE song_artists (
    namespace TEXT NOT NULL,
    song_value TEXT NOT NULL,
    position INTEGER NOT NULL,
    artist_id TEXT NOT NULL,
    artist_name TEXT NOT NULL,
    PRIMARY KEY (namespace, song_value, position),
    FOREIGN KEY (namespace, song_value)
        REFERENCES song_meta(namespace, song_value) ON DELETE CASCADE);

-- 统计 / 历史 / 会话队列刻意**不**外键到 song_meta:meta 是可后补的缓存
-- (打点先于 meta 回填是常态),这些表的行必须能先于 meta 存在。
CREATE TABLE song_stats (
    namespace TEXT NOT NULL,
    song_value TEXT NOT NULL,
    play_count INTEGER NOT NULL DEFAULT 0,
    skip_count INTEGER NOT NULL DEFAULT 0,
    total_listen_ms INTEGER NOT NULL DEFAULT 0,
    last_played_at INTEGER, loved_at INTEGER,
    rating INTEGER,
    PRIMARY KEY (namespace, song_value));

CREATE TABLE play_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    namespace TEXT NOT NULL,
    song_value TEXT NOT NULL,
    played_at INTEGER NOT NULL,
    completed INTEGER NOT NULL,
    listen_ms INTEGER NOT NULL);

-- 最近播放查询(WHERE namespace ORDER BY played_at DESC)与按时间清理都走它。
CREATE INDEX idx_play_history_ns_time ON play_history(namespace, played_at);

CREATE TABLE playlist_cache (
    namespace TEXT NOT NULL,
    playlist_id TEXT NOT NULL,
    name TEXT,
    fetched_at INTEGER NOT NULL,
    track_update_time INTEGER,
    PRIMARY KEY (namespace, playlist_id));

-- 曲目行随 playlist_cache 同事务维护(头行先 upsert 再刷曲目);FK 级联守住孤儿行。
-- song_value 不外键到 song_meta(meta 可后补,同上)。
CREATE TABLE playlist_tracks (
    namespace TEXT NOT NULL,
    playlist_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    song_value TEXT NOT NULL,
    PRIMARY KEY (namespace, playlist_id, position),
    FOREIGN KEY (namespace, playlist_id)
        REFERENCES playlist_cache(namespace, playlist_id) ON DELETE CASCADE);

CREATE TABLE session_state (
    id INTEGER PRIMARY KEY CHECK (id = 0),
    -- 当前曲成对可空:要么都有(有当前曲)要么都空(无),半空行是坏数据。
    cur_namespace TEXT,
    cur_song_value TEXT,
    position_ms INTEGER NOT NULL DEFAULT 0,
    play_mode TEXT NOT NULL,
    volume REAL NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK ((cur_namespace IS NULL) = (cur_song_value IS NULL)));

CREATE TABLE session_queue (
    position INTEGER PRIMARY KEY,
    namespace TEXT NOT NULL,
    song_value TEXT NOT NULL
    );

CREATE TABLE song_kv (
    namespace TEXT NOT NULL,
    song_value TEXT NOT NULL,
    key TEXT NOT NULL,
    -- 标量值:类型标签 + 候选列只填其一(bool 落 int_val),读出按 vtype 重建;
    -- CHECK 让「标签与值列相符」由库而非代码纪律保证。
    vtype TEXT NOT NULL,
    int_val INTEGER,
    real_val REAL,
    text_val TEXT,
    PRIMARY KEY (namespace, song_value, key),
    CHECK (
        (vtype IN ('int', 'bool')
             AND int_val IS NOT NULL AND real_val IS NULL AND text_val IS NULL)
        OR (vtype = 'real'
             AND real_val IS NOT NULL AND int_val IS NULL AND text_val IS NULL)
        OR (vtype = 'text'
             AND text_val IS NOT NULL AND int_val IS NULL AND real_val IS NULL)));

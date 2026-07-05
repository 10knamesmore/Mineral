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

CREATE TABLE song_artists (
    namespace TEXT NOT NULL,
    song_value TEXT NOT NULL,
    position INTEGER NOT NULL,
    artist_id TEXT NOT NULL,
    artist_name TEXT NOT NULL,
    PRIMARY KEY (namespace, song_value, position));

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

CREATE TABLE playlist_cache (
    namespace TEXT NOT NULL,
    playlist_id TEXT NOT NULL,
    name TEXT,
    fetched_at INTEGER NOT NULL,
    track_update_time INTEGER,
    PRIMARY KEY (namespace, playlist_id));

CREATE TABLE playlist_tracks (
    namespace TEXT NOT NULL,
    playlist_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    song_value TEXT NOT NULL,
    PRIMARY KEY (namespace, playlist_id, position));

CREATE TABLE session_state (
    id INTEGER PRIMARY KEY CHECK (id = 0),
    cur_namespace TEXT,
    cur_song_value TEXT,
    position_ms INTEGER NOT NULL DEFAULT 0,
    play_mode TEXT NOT NULL,
    volume REAL NOT NULL,
    updated_at INTEGER NOT NULL);

CREATE TABLE session_queue (
    position INTEGER PRIMARY KEY,
    namespace TEXT NOT NULL,
    song_value TEXT NOT NULL
    );

CREATE TABLE song_kv (
    namespace TEXT NOT NULL,
    song_value TEXT NOT NULL,
    key TEXT NOT NULL,
    -- 标量值:类型标签 + 候选列只填其一(bool 落 int_val),读出按 vtype 重建。
    vtype TEXT NOT NULL,
    int_val INTEGER,
    real_val REAL,
    text_val TEXT,
    PRIMARY KEY (namespace, song_value, key));

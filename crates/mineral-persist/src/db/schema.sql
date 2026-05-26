-- Mineral 结构态 schema(规范化、无 JSON 列)。由 schema.rs 经 include_str! 引入并整体执行。

CREATE TABLE IF NOT EXISTS song_meta (
    namespace TEXT NOT NULL,
    song_value TEXT NOT NULL,
    name TEXT NOT NULL,
    album_id TEXT,
    album_name TEXT,
    duration_ms INTEGER NOT NULL,
    cover_url TEXT,
    PRIMARY KEY (namespace, song_value));

CREATE TABLE IF NOT EXISTS song_artists (
    namespace TEXT NOT NULL,
    song_value TEXT NOT NULL,
    position INTEGER NOT NULL,
    artist_id TEXT NOT NULL,
    artist_name TEXT NOT NULL,
    PRIMARY KEY (namespace, song_value, position));

CREATE TABLE IF NOT EXISTS song_stats (
    namespace TEXT NOT NULL,
    song_value TEXT NOT NULL,
    play_count INTEGER NOT NULL DEFAULT 0,
    skip_count INTEGER NOT NULL DEFAULT 0,
    total_listen_ms INTEGER NOT NULL DEFAULT 0,
    last_played_at INTEGER, loved_at INTEGER,
    PRIMARY KEY (namespace, song_value));

CREATE TABLE IF NOT EXISTS play_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    namespace TEXT NOT NULL,
    song_value TEXT NOT NULL,
    played_at INTEGER NOT NULL,
    completed INTEGER NOT NULL,
    listen_ms INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS playlist_cache (
    namespace TEXT NOT NULL,
    playlist_id TEXT NOT NULL,
    name TEXT,
    fetched_at INTEGER NOT NULL,
    track_update_time INTEGER,
    PRIMARY KEY (namespace, playlist_id));

CREATE TABLE IF NOT EXISTS playlist_tracks (
    namespace TEXT NOT NULL,
    playlist_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    song_value TEXT NOT NULL,
    PRIMARY KEY (namespace, playlist_id, position));

CREATE TABLE IF NOT EXISTS session_state (
    id INTEGER PRIMARY KEY CHECK (id = 0),
    cur_namespace TEXT,
    cur_song_value TEXT,
    position_ms INTEGER NOT NULL DEFAULT 0,
    play_mode TEXT NOT NULL,
    volume REAL NOT NULL,
    updated_at INTEGER NOT NULL);

CREATE TABLE IF NOT EXISTS session_queue (
    position INTEGER PRIMARY KEY,
    namespace TEXT NOT NULL,
    song_value TEXT NOT NULL
    );

-- stats.db baseline。
--
-- 行为埋点的只追加原始事实流水:plays 播放事实 + sessions 收听会话 + 每类交互一张
-- 强 schema 专表(库内无 JSON)。规矩(与 mineral-persist 迁移同源):
--   * 裸 CREATE TABLE(不用 IF NOT EXISTS)——迁移前的老库撞「表已存在」由上层错误
--     信息指引重建,不静默收编过时结构;
--   * 时间戳一律 INTEGER 存 Unix epoch 毫秒;
--   * NULL 表「未知 / 不适用」,绝不用 0 / '' 等哨兵;
--   * 布尔存 INTEGER(0/1);
--   * 只做 additive 演进:改口径改查询,不改历史行。派生指标查询期现算,无物化聚合表。
-- baseline 发布后永不改写;结构变更一律新增 NNNN_*.sql。

-- ============================== 核心档 ==============================

-- 收听会话:播放活动间隔超过 gap 阈值切分新会话。曲目数 / 时长由 plays.session_id
-- 聚合,本表不冗余;ended_at 随活动推进 UPDATE(唯一做行内 UPDATE 的表)。
CREATE TABLE sessions (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    started_at INTEGER NOT NULL,
    ended_at   INTEGER NOT NULL
);

-- 播放事实:每次播放一行,结束(切歌 / 播完 / 停止 / 顶掉 / 错误)时一次写齐,不做
-- 行内 UPDATE。格式 / 音质 / 来源位置各列取自起播时的 PlayUrl 快照。
CREATE TABLE plays (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    ns                   TEXT NOT NULL,
    song_value           TEXT NOT NULL,
    started_at           INTEGER NOT NULL,
    ended_at             INTEGER NOT NULL,
    listen_ms            INTEGER NOT NULL,
    duration_ms_snapshot INTEGER,
    finish_reason        TEXT NOT NULL CHECK (finish_reason IN ('eof', 'skip', 'stop', 'error')),
    skip_at_ms           INTEGER,
    play_mode            TEXT NOT NULL CHECK (play_mode IN ('sequential', 'shuffle', 'repeat_all', 'repeat_one')),
    session_id           INTEGER NOT NULL REFERENCES sessions(id),
    origin_kind          TEXT NOT NULL CHECK (origin_kind IN ('explicit', 'auto_advance', 'resume', 'script', 'unknown')),
    actor                TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    context_kind         TEXT NOT NULL CHECK (context_kind IN ('search', 'playlist', 'album', 'artist', 'manual', 'unknown')),
    context_ref          TEXT,
    audio_format         TEXT,
    is_lossless          INTEGER,
    bitrate_bps          INTEGER,
    quality              TEXT CHECK (quality IN ('standard', 'higher', 'exhigh', 'lossless', 'hires')),
    bit_depth            INTEGER,
    playback_origin      TEXT NOT NULL CHECK (playback_origin IN ('download', 'cache', 'remote')),
    substituted          INTEGER NOT NULL
);
CREATE INDEX idx_plays_started ON plays (started_at);
CREATE INDEX idx_plays_song ON plays (ns, song_value, started_at);
CREATE INDEX idx_plays_session ON plays (session_id);
CREATE INDEX idx_plays_context ON plays (context_kind, context_ref);

-- ============================== 行为域(有人 / 脚本发起,统一带 actor) ==============================

CREATE TABLE searches (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    ts           INTEGER NOT NULL,
    session_id   INTEGER REFERENCES sessions(id),
    actor        TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    query        TEXT,
    query_hash   TEXT NOT NULL,
    kind         TEXT NOT NULL CHECK (kind IN ('song', 'album', 'artist', 'playlist')),
    source       TEXT NOT NULL,
    page         INTEGER NOT NULL,
    result_count INTEGER,
    outcome      TEXT NOT NULL CHECK (outcome IN ('ok', 'failed', 'cancelled'))
);
CREATE INDEX idx_searches_ts ON searches (ts);

CREATE TABLE seeks (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    actor      TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    ns         TEXT NOT NULL,
    song_value TEXT NOT NULL,
    from_ms    INTEGER NOT NULL,
    to_ms      INTEGER NOT NULL
);
CREATE INDEX idx_seeks_ts ON seeks (ts);

CREATE TABLE pauses (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    actor      TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    ns         TEXT NOT NULL,
    song_value TEXT NOT NULL,
    at_ms      INTEGER NOT NULL,
    action     TEXT NOT NULL CHECK (action IN ('pause', 'resume'))
);
CREATE INDEX idx_pauses_ts ON pauses (ts);

CREATE TABLE volume_changes (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    actor      TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    from_pct   INTEGER NOT NULL,
    to_pct     INTEGER NOT NULL
);
CREATE INDEX idx_volume_changes_ts ON volume_changes (ts);

CREATE TABLE mode_changes (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    actor      TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    from_mode  TEXT NOT NULL CHECK (from_mode IN ('sequential', 'shuffle', 'repeat_all', 'repeat_one')),
    to_mode    TEXT NOT NULL CHECK (to_mode IN ('sequential', 'shuffle', 'repeat_all', 'repeat_one'))
);
CREATE INDEX idx_mode_changes_ts ON mode_changes (ts);

CREATE TABLE love_changes (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    ts            INTEGER NOT NULL,
    session_id    INTEGER REFERENCES sessions(id),
    actor         TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    ns            TEXT NOT NULL,
    song_value    TEXT NOT NULL,
    loved         INTEGER NOT NULL,
    origin        TEXT NOT NULL CHECK (origin IN ('user', 'import')),
    remote_mirror TEXT CHECK (remote_mirror IN ('ok', 'not_supported', 'failed'))
);
CREATE INDEX idx_love_changes_ts ON love_changes (ts);

CREATE TABLE queue_ops (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    actor      TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    op         TEXT NOT NULL CHECK (op IN ('set', 'insert_next', 'append', 'clear', 'remove')),
    ns         TEXT,
    song_value TEXT,
    count      INTEGER NOT NULL
);
CREATE INDEX idx_queue_ops_ts ON queue_ops (ts);

CREATE TABLE playlist_ops (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    ts           INTEGER NOT NULL,
    session_id   INTEGER REFERENCES sessions(id),
    actor        TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    op           TEXT NOT NULL CHECK (op IN ('create', 'delete', 'add', 'remove', 'rename', 'set_description')),
    playlist_ref TEXT NOT NULL,
    ns           TEXT,
    song_value   TEXT,
    song_count   INTEGER NOT NULL,
    outcome      TEXT NOT NULL CHECK (outcome IN ('ok', 'failed')),
    error_kind   TEXT CHECK (error_kind IN ('auth_required', 'rate_limited', 'not_supported', 'api', 'other'))
);
CREATE INDEX idx_playlist_ops_ts ON playlist_ops (ts);

CREATE TABLE fetches (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    actor      TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    fetch_kind TEXT NOT NULL CHECK (fetch_kind IN ('my_playlists', 'playlist_detail', 'song_url', 'lyrics', 'remote_play_count', 'search', 'artist_detail', 'artist_albums', 'album_detail')),
    source     TEXT NOT NULL,
    target_ref TEXT,
    trigger    TEXT NOT NULL CHECK (trigger IN ('user', 'system')),
    outcome    TEXT NOT NULL CHECK (outcome IN ('ok', 'failed', 'cancelled')),
    latency_ms INTEGER NOT NULL
);
CREATE INDEX idx_fetches_ts ON fetches (ts);

CREATE TABLE downloads (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    actor      TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    ns         TEXT NOT NULL,
    song_value TEXT NOT NULL,
    quality    TEXT NOT NULL,
    format     TEXT,
    outcome    TEXT NOT NULL CHECK (outcome IN ('downloaded', 'skipped', 'failed')),
    hooked     TEXT NOT NULL CHECK (hooked IN ('none', 'rewrite', 'skip')),
    path       TEXT
);
CREATE INDEX idx_downloads_ts ON downloads (ts);

CREATE TABLE task_cancels (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ts          INTEGER NOT NULL,
    session_id  INTEGER REFERENCES sessions(id),
    actor       TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    filter_tags TEXT NOT NULL
);
CREATE INDEX idx_task_cancels_ts ON task_cancels (ts);

CREATE TABLE copy_renders (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    ts             INTEGER NOT NULL,
    session_id     INTEGER REFERENCES sessions(id),
    actor          TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    template_index INTEGER NOT NULL,
    ctx_kind       TEXT NOT NULL CHECK (ctx_kind IN ('song', 'playlist')),
    target_ref     TEXT,
    outcome        TEXT NOT NULL CHECK (outcome IN ('ok', 'failed'))
);
CREATE INDEX idx_copy_renders_ts ON copy_renders (ts);

CREATE TABLE action_invocations (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    actor      TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    name       TEXT NOT NULL,
    trigger    TEXT NOT NULL CHECK (trigger IN ('tui', 'cli')),
    outcome    TEXT NOT NULL CHECK (outcome IN ('ok', 'failed'))
);
CREATE INDEX idx_action_invocations_ts ON action_invocations (ts);

CREATE TABLE config_overrides (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    actor      TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    path       TEXT NOT NULL
);
CREATE INDEX idx_config_overrides_ts ON config_overrides (ts);

CREATE TABLE store_writes (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    actor      TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    ns         TEXT NOT NULL,
    song_value TEXT NOT NULL,
    key        TEXT NOT NULL,
    op         TEXT NOT NULL CHECK (op IN ('set', 'inc'))
);
CREATE INDEX idx_store_writes_ts ON store_writes (ts);

CREATE TABLE spawns (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    actor      TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    program    TEXT NOT NULL,
    outcome    TEXT NOT NULL CHECK (outcome IN ('exited', 'killed', 'spawn_failed')),
    exit_code  INTEGER
);
CREATE INDEX idx_spawns_ts ON spawns (ts);

CREATE TABLE bus_messages (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    actor      TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    name       TEXT NOT NULL
);
CREATE INDEX idx_bus_messages_ts ON bus_messages (ts);

CREATE TABLE fullscreen_changes (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    actor      TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    fullscreen INTEGER NOT NULL
);
CREATE INDEX idx_fullscreen_changes_ts ON fullscreen_changes (ts);

CREATE TABLE connection_rejects (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    actor      TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    reason     TEXT NOT NULL CHECK (reason IN ('busy', 'version_mismatch'))
);
CREATE INDEX idx_connection_rejects_ts ON connection_rejects (ts);

CREATE TABLE app_lifecycle (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    ts               INTEGER NOT NULL,
    session_id       INTEGER REFERENCES sessions(id),
    actor            TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    who              TEXT NOT NULL CHECK (who IN ('daemon', 'client')),
    phase            TEXT NOT NULL CHECK (phase IN ('start', 'stop')),
    audio_backend    TEXT CHECK (audio_backend IN ('device', 'null')),
    session_restored INTEGER,
    client_version   TEXT
);
CREATE INDEX idx_app_lifecycle_ts ON app_lifecycle (ts);

-- ============================== 系统域(daemon 自治链路,无 actor) ==============================

CREATE TABLE url_resolutions (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    ts                INTEGER NOT NULL,
    session_id        INTEGER REFERENCES sessions(id),
    ns                TEXT NOT NULL,
    song_value        TEXT NOT NULL,
    quality_requested TEXT NOT NULL,
    outcome           TEXT NOT NULL CHECK (outcome IN ('ok', 'empty', 'error')),
    for_prefetch      INTEGER NOT NULL
);
CREATE INDEX idx_url_resolutions_ts ON url_resolutions (ts);

CREATE TABLE hook_fires (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    ns         TEXT,
    song_value TEXT,
    hook       TEXT NOT NULL CHECK (hook IN ('before_stream', 'before_download')),
    stage      TEXT NOT NULL CHECK (stage IN ('immediate', 'prefetch')),
    decision   TEXT NOT NULL CHECK (decision IN ('continue', 'rewrite', 'skip')),
    fail_open  TEXT CHECK (fail_open IN ('timeout', 'thread_dead', 'error'))
);
CREATE INDEX idx_hook_fires_ts ON hook_fires (ts);

CREATE TABLE gapless_boundaries (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    ns         TEXT NOT NULL,
    song_value TEXT NOT NULL,
    result     TEXT NOT NULL CHECK (result IN ('adopt', 'fallback'))
);
CREATE INDEX idx_gapless_boundaries_ts ON gapless_boundaries (ts);

CREATE TABLE prefetches (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    ns         TEXT NOT NULL,
    song_value TEXT NOT NULL,
    source     TEXT NOT NULL CHECK (source IN ('local', 'remote', 'repeat_one')),
    resolution TEXT NOT NULL CHECK (resolution IN ('armed', 'vetoed', 'rewritten', 'failed'))
);
CREATE INDEX idx_prefetches_ts ON prefetches (ts);

CREATE TABLE cache_harvests (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    ns         TEXT NOT NULL,
    song_value TEXT NOT NULL,
    quality    TEXT NOT NULL,
    format     TEXT NOT NULL,
    outcome    TEXT NOT NULL CHECK (outcome IN ('cached', 'discarded')),
    bytes      INTEGER
);
CREATE INDEX idx_cache_harvests_ts ON cache_harvests (ts);

CREATE TABLE cache_evictions (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    cache_key  TEXT NOT NULL,
    bytes      INTEGER NOT NULL
);
CREATE INDEX idx_cache_evictions_ts ON cache_evictions (ts);

CREATE TABLE script_lifecycle (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    event      TEXT NOT NULL CHECK (event IN ('load', 'reload_ok', 'reload_fail', 'callback_error', 'watchdog_abort', 'config_warning')),
    detail     TEXT
);
CREATE INDEX idx_script_lifecycle_ts ON script_lifecycle (ts);

CREATE TABLE config_reloads (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id)
);
CREATE INDEX idx_config_reloads_ts ON config_reloads (ts);

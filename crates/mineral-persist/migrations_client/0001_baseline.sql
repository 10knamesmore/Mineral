-- baseline:client 库(tui.db)固定名表的全量 schema。与 server 库同一纪律:
-- 规范化、无 JSON 列(结构化数据开专表,ui_prefs.value 只放标量字符串)、
-- 裸 CREATE TABLE 让建于迁移机制之前的老库响亮撞错(重建走 `mineral cache reset`)。
--
-- cover_cache 表不在此:它由 CacheIndex 以运行时参数化表名建
-- (与 server 库的 audio_cache 共用同一段建表代码,静态 SQL 表达不了「同构不同名」)。

-- UI 偏好(通用 KV):value 只放标量字符串(枚举名等),禁 JSON blob。
CREATE TABLE ui_prefs (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL);

-- 歌单内光标位置记忆:双锚(song 优先、index 兜底)+ 屏上相对行,一歌单一行。
CREATE TABLE track_pos (
    playlist_namespace TEXT NOT NULL,
    playlist_value TEXT NOT NULL,
    song_namespace TEXT NOT NULL,
    song_value TEXT NOT NULL,
    sel_index INTEGER NOT NULL,
    screen_row INTEGER NOT NULL,
    PRIMARY KEY (playlist_namespace, playlist_value));

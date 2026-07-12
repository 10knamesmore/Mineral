-- shelf source(用户自管收藏)文件索引:uuid ↔ 位置 + 增量信号 + 探测快照。
--
-- 文件事实持久化,uuid 稳定跨 rescan(rename/移动按 size+mtime 调和复用同一 uuid,
-- 收藏与统计不断链——路径当 id 是 MPD 二十年的痛)。分组(album/artist/playlist)由
-- organize 从这些事实重算,不落表。
CREATE TABLE shelf_file (
    -- 稳定 SongId 裸值(define_uuid 随机生成)。
    uuid TEXT PRIMARY KEY,

    -- 所属 mount 根(跨 backend 防路径碰撞:两个 mount 下同相对路径是两条歌)。
    mount TEXT NOT NULL,

    -- 当前路径(mount 命名空间下)。
    path TEXT NOT NULL,

    -- 字节大小;NULL = backend 未给。size 或 mtime 变即重探(增量扫描)。
    size INTEGER,

    -- 最后修改毫秒(epoch ms);NULL = backend 未给。
    mtime_ms INTEGER,

    -- ---- 探测快照(按内容,mineral-probe;各列 NULL = 未探出,不用 0/'' 冒充) ----
    format TEXT,
    bitrate_kbps INTEGER,
    bit_depth INTEGER,
    duration_ms INTEGER,
    title TEXT,
    artist TEXT,
    album TEXT,
    album_artist TEXT,
    track_no INTEGER,
    genre TEXT,

    -- 一个 (mount, path) 只对一个 uuid;调和后 UPDATE path 保持唯一。
    -- UNIQUE 自带索引,兼作「按路径反查 uuid」「列一个 mount 全部文件(前导列 mount)」的加速。
    UNIQUE (mount, path));

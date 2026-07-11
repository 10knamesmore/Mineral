-- 每曲一行的全曲振幅包络缓存(points = 定长 u8 序列的 BLOB,全曲峰值归一)。
-- 不外键 song_meta:包络由播放路径产出,meta 可后补(同 song_stats 先例)。
-- 与音质无关(振幅形状跨码率基本一致),故主键不含 quality。
CREATE TABLE song_envelope (
    namespace TEXT NOT NULL,
    song_value TEXT NOT NULL,
    -- 产出算法版本;读取按版本过滤,不匹配视同缺失、由重算覆盖。
    version INTEGER NOT NULL,
    points BLOB NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (namespace, song_value));

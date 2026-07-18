-- 歌曲维表 + 起播语境显示名快照。
--
-- songs:歌曲展示元数据的库内维表(星型:事实表只存 ns+song_value 外键,报表 JOIN 此表
-- 取名)。写入挂播放路径(起播时 write-through upsert),覆盖面追着报表消费走——凡在
-- plays 出现过的歌必有行;维表后补的行照亮全部历史事实行(JOIN 按键回查,旧行免费回填)。
-- name 恒以新值为准(实体属性,改名全局生效);其余富化列 COALESCE「非空进步、NULL 不回退」,
-- 贫投影不得抹掉已知值(与 mineral.db song_meta 同语义)。album_id 是裸值,namespace
-- 随歌走(专辑与歌同源)。维度非流水,不参与 retention 裁剪。
CREATE TABLE songs (
    ns          TEXT NOT NULL,
    song_value  TEXT NOT NULL,
    name        TEXT NOT NULL,
    alias       TEXT,
    album_id    TEXT,
    album_name  TEXT,
    duration_ms INTEGER,
    PRIMARY KEY (ns, song_value)
);

-- 起播语境的显示名快照(专辑/艺人/歌单页当时的标题;search/manual/unknown 落 NULL——
-- 搜索词有 searches 表与隐私档管辖,不经此列旁路)。快照语义:属于事件时刻,不随后续
-- 改名回写;同 context_ref 组内取任意非空即当前名(MAX 聚合)。
ALTER TABLE plays ADD COLUMN context_name TEXT;

-- copy_renders.ctx_kind 放宽:新增 album / artist 上下文(复制模板可作用于专辑 / artist,
-- 与 config CopyContext / protocol CopyTemplateCtx 的新变体对齐)。
--
-- SQLite 无法 ALTER 既有 CHECK 约束,按官方「建新表→拷数据→换名」惯例重建 copy_renders:
-- 仅放宽 ctx_kind 取值集,其余列 / 索引原样重建,历史行照拷不丢。
CREATE TABLE copy_renders_rebuilt (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    ts             INTEGER NOT NULL,
    session_id     INTEGER REFERENCES sessions(id),
    actor          TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    template_index INTEGER NOT NULL,
    ctx_kind       TEXT NOT NULL CHECK (ctx_kind IN ('song', 'playlist', 'album', 'artist')),
    target_ref     TEXT,
    outcome        TEXT NOT NULL CHECK (outcome IN ('ok', 'failed'))
);

INSERT INTO copy_renders_rebuilt
    (id, ts, session_id, actor, template_index, ctx_kind, target_ref, outcome)
SELECT id, ts, session_id, actor, template_index, ctx_kind, target_ref, outcome
FROM copy_renders;

DROP TABLE copy_renders;

ALTER TABLE copy_renders_rebuilt RENAME TO copy_renders;

CREATE INDEX idx_copy_renders_ts ON copy_renders (ts);

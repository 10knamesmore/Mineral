-- 队列结构编辑的 op 判别扩容:move / clear_above / clear_below / transform / undo。
--
-- SQLite 改不了 CHECK 约束,只能重建表再搬数据。除 op 的取值集合外,列定义与
-- 0001_baseline 的 queue_ops 完全一致。
CREATE TABLE queue_ops_new (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    actor      TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),
    op         TEXT NOT NULL CHECK (op IN (
        'set', 'insert_next', 'append', 'clear', 'remove',
        'move', 'clear_above', 'clear_below', 'transform', 'undo'
    )),
    ns         TEXT,
    song_value TEXT,
    count      INTEGER NOT NULL
);

INSERT INTO queue_ops_new (id, ts, session_id, actor, op, ns, song_value, count)
SELECT id, ts, session_id, actor, op, ns, song_value, count FROM queue_ops;

DROP TABLE queue_ops;
ALTER TABLE queue_ops_new RENAME TO queue_ops;
CREATE INDEX idx_queue_ops_ts ON queue_ops (ts);

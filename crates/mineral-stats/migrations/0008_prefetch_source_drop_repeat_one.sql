-- 预取来源收敛为 local / remote:repeat_one 自始没有发射点(RepeatOne 预取实际走重新
-- resolve,按 origin 记 local / remote),Rust 侧变体已删,CHECK 同步收紧。
-- SQLite 不能改 CHECK,重建表;repeat_one 从未落库,全量复制安全。

CREATE TABLE prefetches_new (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    session_id INTEGER REFERENCES sessions(id),
    ns         TEXT NOT NULL,
    song_value TEXT NOT NULL,
    source     TEXT NOT NULL CHECK (source IN ('local', 'remote')),
    resolution TEXT NOT NULL CHECK (resolution IN ('armed', 'vetoed', 'rewritten', 'failed'))
);
INSERT INTO prefetches_new (id, ts, session_id, ns, song_value, source, resolution)
    SELECT id, ts, session_id, ns, song_value, source, resolution FROM prefetches;
DROP TABLE prefetches;
ALTER TABLE prefetches_new RENAME TO prefetches;
CREATE INDEX idx_prefetches_ts ON prefetches (ts);

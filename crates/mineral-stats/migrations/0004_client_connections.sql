-- 连接生命周期(行为域):client 断开时落一行,回答「TUI 开了几次挂了多久 /
-- oneshot 命令的调用节奏 / 多 client 并存是否真实发生」。
--
-- 只记握手完成的连接(探活 / 版本被拒的不落);daemon 停机时仍在线的连接
-- 不结算、丢行——与硬 kill 丢在播行同类取舍,fun 数据不苛求完备。

CREATE TABLE client_connections (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ts          INTEGER NOT NULL,
    session_id  INTEGER REFERENCES sessions(id),
    actor       TEXT NOT NULL CHECK (actor IN ('user', 'script', 'system', 'cli')),

    -- client 自报名(握手身份,如 tui / cli;开放命名空间,新 client 形态自取名,
    -- 刻意无 CHECK 枚举——具名入库才能按 client 分组看使用时长,别压扁成类别)。
    client      TEXT NOT NULL,

    -- 连接存续时长(ms;行 ts 即断开时刻,连接时刻 = ts - duration_ms)。
    duration_ms INTEGER NOT NULL,

    -- 连接建立时刻的在线连接数(含自己;> 1 = 多 client 并存)。
    concurrent  INTEGER NOT NULL
);
CREATE INDEX idx_client_connections_ts ON client_connections (ts);

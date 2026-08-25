-- 播放资源解析不再要求结果必须有直链，因此把统计表改为 stream 语义。
-- fetches 中既有 song_url 行是历史 channel 取数事实，保持原值。

ALTER TABLE url_resolutions RENAME TO stream_resolutions;
DROP INDEX idx_url_resolutions_ts;
CREATE INDEX idx_stream_resolutions_ts ON stream_resolutions (ts);

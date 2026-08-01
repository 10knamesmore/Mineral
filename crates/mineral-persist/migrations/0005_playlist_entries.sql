-- Playlist membership 显式保存 canonical index 和 Song 自己的 namespace。
CREATE TABLE playlist_entries (
    playlist_namespace TEXT NOT NULL,
    playlist_value TEXT NOT NULL,
    collection_index INTEGER NOT NULL CHECK (collection_index >= 0),
    song_namespace TEXT NOT NULL,
    song_value TEXT NOT NULL,
    PRIMARY KEY (playlist_namespace, playlist_value, collection_index),
    FOREIGN KEY (playlist_namespace, playlist_value)
        REFERENCES playlist_cache(namespace, playlist_id) ON DELETE CASCADE);

-- 老 schema 将 Song namespace 隐含为 Playlist namespace；原 position 原值迁移，不重排。
INSERT INTO playlist_entries (
    playlist_namespace,
    playlist_value,
    collection_index,
    song_namespace,
    song_value
)
SELECT namespace, playlist_id, position, namespace, song_value
FROM playlist_tracks;

DROP TABLE playlist_tracks;

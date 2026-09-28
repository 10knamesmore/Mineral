//! daemon 服务端数据库的迁移链、版本脚本与表结构快照。

pub(super) mod audio_cache;
pub(super) mod play_history;
pub(super) mod playlist_cache;
pub(super) mod playlist_entries;
pub(super) mod session_queue;
pub(super) mod session_state;
pub(super) mod song_artists;
pub(super) mod song_envelope;
pub(super) mod song_favorites;
pub(super) mod song_kv;
pub(super) mod song_meta;
pub(super) mod song_stats;

mod m20260906_server;
mod migrator;

pub(crate) use migrator::ServerMigrator;

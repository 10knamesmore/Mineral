//! Account and developer credentials share one atomically replaced private file.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::time::sleep;

use crate::{Error, Result};

/// Apple Music account token. Intentionally does not implement Debug.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StoredAppleMusicAuth {
    /// Only sent to Apple during authenticated requests.
    #[serde(rename = "value")]
    pub(crate) media_user_token: String,
}

/// Website token and its acquisition/expiry times, in Unix seconds.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeveloperToken {
    pub value: String,

    pub fetched_at_unix_secs: u64,

    /// JWT exp; cache freshness also applies the currently configured TTL.
    pub expires_at_unix_secs: u64,
}

impl DeveloperToken {
    pub(crate) fn is_valid(&self, ttl: Duration) -> Result<bool> {
        let age_limit =
            self.fetched_at_unix_secs
                .checked_add(ttl.as_secs())
                .ok_or(Error::InvalidData {
                    field: "developer token cache TTL",
                })?;
        Ok(now()? < self.expires_at_unix_secs.min(age_limit))
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Credentials {
    pub user_token: Option<StoredAppleMusicAuth>,

    pub developer_token: Option<DeveloperToken>,
}

fn path() -> Result<PathBuf> {
    Ok(mineral_paths::data_dir()?.join("apple-music.json"))
}

pub(crate) fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

/// Atomic replacement lets readers observe a complete file without taking the writer lock.
pub fn load_stored() -> Result<Option<StoredAppleMusicAuth>> {
    let credentials = read()?;
    credentials
        .user_token
        .map(|auth| from_token(&auth.media_user_token))
        .transpose()
}

pub(crate) fn read() -> Result<Credentials> {
    match fs::read(path()?) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| Error::CredentialFormat {
            line: error.line(),
            column: error.column(),
        }),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Credentials::default()),
        Err(error) => Err(error.into()),
    }
}

/// Rejects empty values and cookie delimiters before making network requests.
pub(crate) fn from_token(token: &str) -> Result<StoredAppleMusicAuth> {
    let token = token.trim();
    if token.is_empty() || token.contains(['\r', '\n', ';']) {
        return Err(Error::AuthRequired);
    }
    Ok(StoredAppleMusicAuth {
        media_user_token: token.to_owned(),
    })
}

/// The separate inode remains stable when apple-music.json is replaced.
/// Keep this file open through read/merge/write, including developer-token acquisition.
pub(crate) struct WriteLock {
    _file: File,
}

impl WriteLock {
    pub(crate) async fn acquire() -> Result<Self> {
        let file = tokio::task::spawn_blocking(|| {
            let directory = mineral_paths::data_dir()?;
            fs::create_dir_all(&directory)?;
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(directory.join("apple-music.lock"))?;
            Ok::<_, Error>(file)
        })
        .await??;
        // A blocking lock acquisition would outlive a cancelled request and delay shutdown.
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(TryLockError::WouldBlock) => sleep(Duration::from_millis(20)).await,
                Err(TryLockError::Error(error)) => return Err(error.into()),
            }
        }
    }

    /// NamedTempFile creates mode 0600; persist atomically replaces the credential file.
    pub(crate) fn save(&self, credentials: &Credentials) -> Result<PathBuf> {
        let path = path()?;
        let directory = path.parent().ok_or(Error::InvalidData {
            field: "credential path",
        })?;
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        temporary.write_all(&serde_json::to_vec_pretty(credentials)?)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&path)
            .map_err(|error| Error::Io(error.error))?;
        File::open(directory)?.sync_all()?;
        Ok(path)
    }
}

/// Called only after successful account validation. Merge under the same lock as refresh.
pub(crate) async fn save_login(auth: StoredAppleMusicAuth) -> Result<PathBuf> {
    let lock = WriteLock::acquire().await?;
    let mut credentials = read()?;
    credentials.user_token = Some(auth);
    let path = lock.save(&credentials)?;
    mineral_log::info!(target: "apple_music", "credential saved");
    Ok(path)
}

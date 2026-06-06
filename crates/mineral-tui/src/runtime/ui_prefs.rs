//! UI 偏好持久化:跨会话保留的客户端态(歌词副轨档等),落 `tui.db` 的 `ui_prefs` 表。
//!
//! 与封面缓存共用同一个 [`ClientStore`] 连接池(双开两个池会在 sqlite 文件锁上互撞);
//! 库不可用时整体降级——不读不存、一切走默认值,不拖垮 TUI。

use std::sync::Arc;

use mineral_persist::ClientStore;

use crate::runtime::state::LyricExtra;

/// 歌词副轨档的偏好键(`ui_prefs` 表)。
const LYRIC_EXTRA_KEY: &str = "lyric_extra";

/// UI 偏好句柄:启动读一次初值,运行时改动 fire-and-forget 落盘。
pub struct UiPrefs {
    /// 共享的客户端库句柄;`None` = 降级禁用(不读不存)。
    store: Option<Arc<ClientStore>>,

    /// 启动时读回的歌词副轨档(禁用 / 读失败 / 脏值 = 默认原文档)。
    initial_lyric_extra: LyricExtra,
}

impl UiPrefs {
    /// 从客户端库读回全部偏好初值,组装句柄。
    ///
    /// # Params:
    ///   - `store`: 共享的 `tui.db` 句柄(`None` = 降级禁用)
    ///
    /// # Return:
    ///   就绪句柄(读失败不冒泡,对应偏好落默认值)。
    pub async fn load(store: Option<Arc<ClientStore>>) -> Self {
        let mut initial = LyricExtra::default();
        if let Some(s) = &store {
            match s.get_pref(LYRIC_EXTRA_KEY).await {
                Ok(Some(v)) => match LyricExtra::from_name(&v) {
                    Some(extra) => initial = extra,
                    None => {
                        mineral_log::warn!(target: "prefs", value = %v, "歌词副轨档偏好值无法解析,用默认");
                    }
                },
                Ok(None) => {}
                Err(e) => {
                    mineral_log::warn!(target: "prefs", error = mineral_log::chain(&e), "读歌词副轨档偏好失败,用默认");
                }
            }
        }
        Self {
            store,
            initial_lyric_extra: initial,
        }
    }

    /// 禁用态(测试构造 App 用):初值全默认,save 静默 no-op,不依赖 tokio runtime。
    /// 生产降级路径不走这里——`load(None)` 同样得到全默认 + no-op。
    #[cfg(test)]
    pub fn disabled() -> Self {
        Self {
            store: None,
            initial_lyric_extra: LyricExtra::default(),
        }
    }

    /// 启动时读回的歌词副轨档初值。
    pub fn initial_lyric_extra(&self) -> LyricExtra {
        self.initial_lyric_extra
    }

    /// 把歌词副轨档落盘(fire-and-forget,失败仅 warn)。
    ///
    /// 在 tokio runtime 外调用(纯同步测试)静默跳过——偏好持久化是优化项,不值得 panic。
    ///
    /// # Params:
    ///   - `extra`: 当前档位
    pub fn save_lyric_extra(&self, extra: LyricExtra) {
        let Some(store) = &self.store else { return };
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let store = Arc::clone(store);
        handle.spawn(async move {
            if let Err(e) = store.set_pref(LYRIC_EXTRA_KEY, extra.name()).await {
                mineral_log::warn!(target: "prefs", error = mineral_log::chain(&e), "歌词副轨档落盘失败");
            }
        });
    }
}

/// 打开共享的客户端库(`tui.db`),封面缓存索引与 UI 偏好共用。
///
/// 路径解析 / 建目录 / 打开失败一律 warn + `None` 降级(封面不缓存、偏好不存不读,
/// 其余照常),与音频无设备降级 null 模式同理。
///
/// # Return:
///   就绪句柄;不可用时 `None`。
pub async fn open_client_store() -> Option<Arc<ClientStore>> {
    let db = match mineral_paths::tui_db() {
        Ok(db) => db,
        Err(e) => {
            mineral_log::warn!(target: "prefs", error = mineral_log::chain(&e), "tui.db 路径不可用,客户端持久化降级");
            return None;
        }
    };
    // sqlite mode=rwc 只建文件不建父目录,fresh env 下需先确保 data_dir 存在。
    if let Some(parent) = db.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        let e = color_eyre::Report::new(e);
        mineral_log::warn!(target: "prefs", error = mineral_log::chain(&e), "建 tui.db 目录失败,客户端持久化降级");
        return None;
    }
    match ClientStore::open(&db).await {
        Ok(s) => Some(Arc::new(s)),
        Err(e) => {
            mineral_log::warn!(target: "prefs", error = mineral_log::chain(&e), "打开 tui.db 失败,客户端持久化降级");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mineral_persist::ClientStore;

    use super::{LYRIC_EXTRA_KEY, UiPrefs};
    use crate::runtime::state::LyricExtra;

    /// `LyricExtra` name / from_name 对偶:三档 round-trip;脏值 / 大小写不符为 `None`。
    #[test]
    fn lyric_extra_name_round_trips() {
        for extra in [
            LyricExtra::None,
            LyricExtra::Translation,
            LyricExtra::Romanization,
        ] {
            assert_eq!(LyricExtra::from_name(extra.name()), Some(extra));
        }
        assert_eq!(LyricExtra::from_name(""), None);
        assert_eq!(LyricExtra::from_name("Translation"), None, "大小写敏感");
    }

    /// `load`:落库的档被读回;脏值降级默认;禁用态恒默认。
    #[tokio::test]
    async fn load_reads_back_persisted_extra() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Arc::new(ClientStore::open(&dir.path().join("tui.db")).await?);
        store.set_pref(LYRIC_EXTRA_KEY, "romanization").await?;
        let prefs = UiPrefs::load(Some(Arc::clone(&store))).await;
        assert_eq!(prefs.initial_lyric_extra(), LyricExtra::Romanization);

        store.set_pref(LYRIC_EXTRA_KEY, "garbage").await?;
        let prefs = UiPrefs::load(Some(store)).await;
        assert_eq!(
            prefs.initial_lyric_extra(),
            LyricExtra::None,
            "脏值应降级默认档"
        );

        assert_eq!(UiPrefs::disabled().initial_lyric_extra(), LyricExtra::None);
        Ok(())
    }

    /// `save_lyric_extra` fire-and-forget 落盘后,新一轮 `load` 能读回同档。
    #[tokio::test]
    async fn save_then_load_round_trips() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Arc::new(ClientStore::open(&dir.path().join("tui.db")).await?);
        let prefs = UiPrefs::load(Some(Arc::clone(&store))).await;
        prefs.save_lyric_extra(LyricExtra::Translation);
        // fire-and-forget 的 spawn 需要让出执行;轮询直到写入可见。
        let mut seen = None;
        for _ in 0..64 {
            tokio::task::yield_now().await;
            seen = store.get_pref(LYRIC_EXTRA_KEY).await?;
            if seen.is_some() {
                break;
            }
        }
        assert_eq!(seen.as_deref(), Some("translation"));
        let reloaded = UiPrefs::load(Some(store)).await;
        assert_eq!(reloaded.initial_lyric_extra(), LyricExtra::Translation);
        Ok(())
    }
}

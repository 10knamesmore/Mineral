//! 歌单菜单调用通用输入框，并将结果交给后端。

use super::App;
use crate::components::popup::{OverlayAction, OverlayKind, OverlayResponse, TextPrompt};
use mineral_model::PlaylistId;
use mineral_protocol::PlaylistOp;

impl App {
    /// 为当前队列打开空名称输入框。
    pub(crate) fn open_save_queue_prompt(&mut self) {
        if self.state.player.queue.is_empty() {
            return;
        }
        let prompt = TextPrompt::new("Save queue as playlist", |text| {
            submit_playlist_name(text, |name| PlaylistOp::SaveQueue { name })
        });
        self.overlays.push(OverlayKind::TextPrompt(prompt));
    }

    /// 打开预填名称的改名输入框。
    pub(crate) fn open_rename_playlist_prompt(&mut self, id: PlaylistId, name: String) {
        let mut prompt = TextPrompt::new("Rename playlist", move |text| {
            submit_playlist_name(text, |name| PlaylistOp::Rename {
                id: id.clone(),
                name,
            })
        });
        prompt.set_text(name);
        self.overlays.push(OverlayKind::TextPrompt(prompt));
    }
}

/// 校验歌单名称；不通过时只提示，保留输入供用户修改。
fn submit_playlist_name(
    text: &str,
    operation: impl FnOnce(String) -> PlaylistOp,
) -> OverlayResponse {
    let name = text.trim();
    if name.is_empty() || name.chars().any(char::is_control) {
        return OverlayResponse::Do(OverlayAction::FlashError(
            "Enter a valid playlist name".to_owned(),
        ));
    }
    OverlayResponse::CloseAndDo(OverlayAction::Playlist(operation(name.to_owned())))
}

#[cfg(test)]
mod tests {
    use super::App;
    use crate::runtime::{action::Action, state::View, view_model::PlaylistView};
    use crate::test_support::{TestClient, app_with_queue};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use mineral_model::{Playlist, PlaylistActions, PlaylistId, SourceKind};
    use mineral_protocol::PlaylistOp;
    use std::sync::{Arc, Mutex};

    /// 经 App 事件入口发送按键。
    fn key(app: &mut App, code: KeyCode) {
        app.handle_event(&Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    /// 输入框独占字符和粘贴，确认前不发送请求。
    #[test]
    fn queue_menu_input_owns_keys_and_paste_until_submit() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, 0)?;
        let operations = Arc::new(Mutex::new(Vec::new()));
        app.client = Arc::new(TestClient {
            playlist_operations: Arc::clone(&operations),
            ..TestClient::default()
        });
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24))?;
        terminal.draw(|frame| crate::view::draw(frame, &app))?;
        app.dispatch(Action::OpenQueue);
        key(&mut app, KeyCode::Char('o'));
        key(&mut app, KeyCode::Char('s'));
        assert!(app.overlays.in_text_input());
        let notice_count = app.notifications.entry_count();
        app.handle_event(&Event::Paste("  ".to_owned()));
        key(&mut app, KeyCode::Enter);
        assert!(operations.lock().is_ok_and(|ops| ops.is_empty()));
        assert!(app.overlays.in_text_input());
        assert_eq!(app.notifications.entry_count(), notice_count + 1);
        app.handle_event(&Event::Paste("夜跑\n".to_owned()));
        key(&mut app, KeyCode::Char('Q'));
        key(&mut app, KeyCode::Char(' '));
        assert!(!app.stop_daemon_on_quit);
        assert!(!app.should_quit);
        for _ in 0..100 {
            app.overlays.tick();
        }
        terminal.draw(|frame| crate::view::draw(frame, &app))?;
        println!(
            "Text prompt rendered by the application:\n{}",
            terminal.backend()
        );
        assert!(operations.lock().is_ok_and(|ops| ops.is_empty()));
        key(&mut app, KeyCode::Enter);
        assert!(!app.overlays.in_text_input());
        key(&mut app, KeyCode::Enter);
        assert_eq!(
            *operations
                .lock()
                .map_err(|error| color_eyre::eyre::eyre!("poisoned operations: {error}"))?,
            vec![PlaylistOp::SaveQueue {
                name: "夜跑 Q".to_owned()
            }]
        );
        Ok(())
    }

    /// 改名使用捕获的身份，取消不产生后端操作。
    #[test]
    fn rename_prefills_exact_identity_and_cancel_sends_nothing() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, 0)?;
        let operations = Arc::new(Mutex::new(Vec::new()));
        app.client = Arc::new(TestClient {
            playlist_operations: Arc::clone(&operations),
            ..TestClient::default()
        });
        let id = PlaylistId::new(SourceKind::MINERAL, "saved");
        let original_playlist = Playlist::builder()
            .id(id.clone())
            .name("夜跑".to_owned())
            .actions(PlaylistActions {
                rename: true,
                delete: true,
            })
            .build();
        app.state.library.playlists = vec![PlaylistView {
            data: original_playlist.clone(),
        }];
        app.state.browse.view.switch_to(View::Playlists);
        key(&mut app, KeyCode::Char('o'));
        key(&mut app, KeyCode::Char('r'));
        assert!(app.overlays.in_text_input());
        key(&mut app, KeyCode::Esc);
        assert!(operations.lock().is_ok_and(|ops| ops.is_empty()));
        for _ in 0..100 {
            app.overlays.tick();
        }
        key(&mut app, KeyCode::Char('o'));
        key(&mut app, KeyCode::Char('r'));
        let mut another_playlist = original_playlist.clone();
        another_playlist.id = PlaylistId::new(SourceKind::MINERAL, "another");
        app.state.apply(&mineral_task::TaskEvent::LibrarySnapshot {
            playlists: vec![another_playlist, original_playlist.clone()],
        });
        key(&mut app, KeyCode::Backspace);
        key(&mut app, KeyCode::Char('行'));
        key(&mut app, KeyCode::Enter);
        assert_eq!(
            *operations
                .lock()
                .map_err(|_e| color_eyre::eyre::eyre!("poisoned operations"))?,
            vec![PlaylistOp::Rename {
                id: id.clone(),
                name: "夜行".to_owned()
            }]
        );
        for _ in 0..100 {
            app.overlays.tick();
        }
        app.state.apply(&mineral_task::TaskEvent::LibrarySnapshot {
            playlists: vec![original_playlist],
        });
        key(&mut app, KeyCode::Char('o'));
        key(&mut app, KeyCode::Char('x'));
        assert!(
            operations
                .lock()
                .is_ok_and(|ops| ops.last() == Some(&PlaylistOp::Delete { id }))
        );
        Ok(())
    }

    /// 列表推送同步已打开歌单的名称；移除其缓存时保留其他歌单，并退出失效详情页。
    #[test]
    fn library_updates_rename_and_remove_the_open_playlist() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, 0)?;
        let id = PlaylistId::new(SourceKind::MINERAL, "saved");
        let playlist = Playlist::builder()
            .id(id.clone())
            .name("Old".to_owned())
            .build();
        let retained = Playlist::builder()
            .id(PlaylistId::new(SourceKind::MINERAL, "retained"))
            .name("Retained".to_owned())
            .build();
        app.state.apply(&mineral_task::TaskEvent::LibrarySnapshot {
            playlists: vec![playlist.clone(), retained.clone()],
        });
        for data in [&playlist, &retained] {
            app.state
                .apply(&mineral_task::TaskEvent::PlaylistDetailFetched {
                    id: data.id.clone(),
                    load: mineral_channel_core::PlaylistLoad::Complete,
                    detail: Box::new(mineral_channel_core::PlaylistDetail::complete(data.clone())),
                });
        }
        app.state.browse.nav.opened_playlist = Some(id.clone());
        app.state.browse.view.switch_to(View::Library);
        let mut renamed = playlist;
        renamed.name = "New".to_owned();
        app.state.apply(&mineral_task::TaskEvent::LibrarySnapshot {
            playlists: vec![retained.clone(), renamed.clone()],
        });
        assert_eq!(app.state.opened_playlist().map(|p| &p.data), Some(&renamed));
        assert!(app.state.library.playlist_complete(&id));
        app.state.apply(&mineral_task::TaskEvent::LibrarySnapshot {
            playlists: vec![retained.clone()],
        });
        assert!(app.state.opened_playlist().is_none());
        assert!(!app.state.library.tracks.contains_key(&id));
        assert!(!app.state.library.tracks_requested.contains_key(&id));
        assert!(app.state.library.playlist_complete(&retained.id));
        assert!(
            app.state
                .library
                .tracks_requested
                .contains_key(&retained.id)
        );
        assert_eq!(app.state.browse.view.current(), View::Playlists);
        app.state.apply(&mineral_task::TaskEvent::LibrarySnapshot {
            playlists: Vec::new(),
        });
        assert!(app.state.library.playlists.is_empty());
        assert!(!app.state.library.tracks.contains_key(&retained.id));
        assert!(
            !app.state
                .library
                .tracks_requested
                .contains_key(&retained.id)
        );
        Ok(())
    }
}

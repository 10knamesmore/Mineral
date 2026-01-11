use crossterm::event::{KeyCode, KeyEvent};

use crate::{
    app::App,
    event_handler::{action::PageAction, Action, AppEvent},
    state::{
        main_page::{MainPageState, MainPageSubState},
        Selectable,
    },
};

pub fn dispatch(key_event: KeyEvent) -> Option<Action> {
    let action = match key_event.code {
        KeyCode::Char('k') | KeyCode::Up => Action::Page(PageAction::NavUp(1)),
        KeyCode::Char('j') | KeyCode::Down => Action::Page(PageAction::NavDown(1)),
        KeyCode::Char('K') => Action::Page(PageAction::NavUp(5)),
        KeyCode::Char('J') => Action::Page(PageAction::NavDown(5)),
        KeyCode::Enter | KeyCode::Char('l') => Action::Page(PageAction::NavForward),
        KeyCode::Char('h') => Action::Page(PageAction::NavBackward),
        KeyCode::Char('H') => Action::Page(PageAction::NextPage),
        KeyCode::Char('L') => Action::Page(PageAction::LastPage),
        _ => return None,
    };

    Some(action)
}

pub fn handle_page_action(state: &mut MainPageState, action: PageAction) {
    match action {
        PageAction::NavUp(n) => state.now_tab_move_up_by(n),
        PageAction::NavDown(n) => state.now_tab_move_down_by(n),
        PageAction::NavBackward => state.nav_backward(),
        PageAction::NavForward => match &state.now_state {
            MainPageSubState::TabView(_) => {
                state.nav_forward();
            }
            MainPageSubState::ViewingPlayList(tab_list)
            | MainPageSubState::ViewingAlbum(tab_list)
            | MainPageSubState::ViewingArtist(tab_list) => {
                if let Some(song) = tab_list.selected_item() {
                    AppEvent::Action(Action::PlaySong(song.clone())).emit();
                }
            }
        },
        PageAction::NextPage => todo!(),
        PageAction::LastPage => todo!(),
    }
}

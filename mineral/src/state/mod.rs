use crate::app::Song;
use std::fmt::Debug;

pub(crate) mod main_page;

#[derive(Clone, Copy)]
pub(crate) enum Page {
    Main,
    Search,
    // TODO
}

#[derive(Clone, Copy)]
pub(crate) enum PopupState {
    None,
    ConfirmExit,
    Notificacion, // TODO
}

pub trait SongList: Debug {
    fn songs(&self) -> &[Song];
}

pub trait HasId {
    fn id(&self) -> u64;
}

#[allow(dead_code)]
pub(crate) trait Selectable {
    type Item: HasId;

    fn items(&self) -> &[Self::Item];
    fn selected_index(&self) -> Option<usize>;
    fn _select(&mut self, index: usize);

    fn len(&self) -> usize {
        self.items().len()
    }
    fn is_empty(&self) -> bool {
        self.items().is_empty()
    }

    fn selected_item(&self) -> Option<&Self::Item> {
        self.selected_index()
            .and_then(|index| self.items().get(index))
    }

    fn move_up(&mut self) {
        self.move_up_by(1);
    }
    fn move_down(&mut self) {
        self.move_down_by(1);
    }

    fn move_up_by(&mut self, n: usize) {
        if let Some(index) = self.selected_index() {
            if index >= n {
                self._select(index - n);
            } else {
                self._select(0);
            }
        } else if !self.is_empty() {
            self._select(self.len() - 1)
        }
    }
    fn move_down_by(&mut self, n: usize) {
        let items = self.items();
        if let Some(index) = self.selected_index() {
            if index + n < items.len() {
                self._select(index + n);
            } else if !items.is_empty() {
                self._select(items.len() - 1);
            }
        } else if !self.is_empty() {
            self._select(0)
        }
    }
}

pub(crate) trait HasDescription {
    fn description(&self) -> &str;
}

use crate::util::notification::Notification;

#[allow(unused)]
pub enum PageAction {
    NavUp(usize),
    NavDown(usize),
    NavBackward,
    NavForward,
    NextPage,
    LastPage,
}

pub enum PopupAction {
    ConfirmYes,
    ConfirmNo,
    ClosePopup,
}

#[allow(unused)]
pub enum Action {
    Quit,

    Help,

    Notification(Notification),

    Page(PageAction),
    Popup(PopupAction),

    PlaySelectedTrac,
}

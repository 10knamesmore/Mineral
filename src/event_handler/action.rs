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

pub enum PopupResponse {
    ConfirmExit { accepted: bool },
    ClosePopup,
}

#[allow(unused)]
pub enum Action {
    Quit,

    Help,

    Notification(Notification),

    Page(PageAction),
    PopupResponse(PopupResponse),

    PlaySelectedTrac,
}

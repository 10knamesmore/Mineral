use crate::util::notification::Notification;

#[allow(unused)]
#[derive(Debug, strum_macros::Display)]
pub enum PageAction {
    #[strum(to_string = "NavUp({0})")]
    NavUp(usize),

    #[strum(to_string = "NavDown({0})")]
    NavDown(usize),

    NavBackward,
    NavForward,
    NextPage,
    LastPage,
}

#[derive(Debug, strum_macros::Display)]
pub enum PopupResponse {
    ConfirmExit { accepted: bool },
    ClosePopup,
}

#[allow(unused)]
#[derive(Debug, strum_macros::Display)]
pub enum Action {
    Quit,

    Help,

    #[strum(to_string = "Notification({0:?})")]
    Notification(Notification),

    #[strum(to_string = "Page({0})")]
    Page(PageAction),

    #[strum(to_string = "PopupResponse({0})")]
    PopupResponse(PopupResponse),

    PlaySelectedTrac,

    LoadMusics,
}

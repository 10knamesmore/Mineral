pub(crate) mod main_page;
pub(crate) mod selectable;
pub(crate) mod song;

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

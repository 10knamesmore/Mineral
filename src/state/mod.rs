pub(crate) mod song;
pub(crate) mod detailed;
pub(crate) mod main_page;
pub(crate) mod selectable;

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
    // TODO
}

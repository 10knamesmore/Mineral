#[derive(Default, Debug)]
pub(crate) enum NotifyUrgency {
    Debug,
    #[default]
    Info,
    Warning,
    Error,
}

#[derive(Debug, Default)]
pub(crate) struct Notification {
    title: String,
    content: String,
    urgency: NotifyUrgency,
}

impl Notification {
    pub(crate) fn new<T: Into<String>>(title: T, content: T, urgency: NotifyUrgency) -> Self {
        Self {
            title: title.into(),
            content: content.into(),
            urgency,
        }
    }
    pub(crate) fn urgency(&self) -> &NotifyUrgency {
        &self.urgency
    }

    pub(crate) fn title(&self) -> &String {
        &self.title
    }

    pub(crate) fn content(&self) -> &String {
        &self.content
    }
}

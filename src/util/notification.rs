use derive_setters::Setters;

#[derive(Default, Debug)]
pub(crate) enum NotifyUrgency {
    Debug,
    #[default]
    Info,
    Warning,
    Error,
}

#[derive(Debug, Default, Setters)]
pub(crate) struct Notification {
    #[setters(into)]
    title: String,
    #[setters(into)]
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
    pub(crate) fn get_urgency(&self) -> &NotifyUrgency {
        &self.urgency
    }

    pub(crate) fn get_title(&self) -> &String {
        &self.title
    }

    pub(crate) fn get_content(&self) -> &String {
        &self.content
    }
}

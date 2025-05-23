use ratatui::layout::{Constraint, Flex, Layout, Rect};

pub(crate) fn center(area: Rect, horizontal: Constraint, vertical: Constraint) -> Rect {
    let [area] = Layout::horizontal([horizontal])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([vertical]).flex(Flex::Center).areas(area);

    area
}

/// aspect_ratio = width / height
pub(crate) fn aspect_fit_center(outer: Rect, aspect_ratio: f64) -> Rect {
    let outer_height = outer.height as f64;
    let outer_width = outer.width as f64;

    let (new_width, new_height) = if outer_width > outer_height {
        (outer_height * aspect_ratio, outer_height)
    } else {
        (outer_width, outer_width / aspect_ratio)
    };

    let new_width = new_width.round() as u16;
    let new_height = new_height.round() as u16;

    let x = outer.x + (outer.width - new_width) / 2;
    let y = outer.y + (outer.height - new_height) / 2;

    Rect {
        x,
        y,
        width: new_width,
        height: new_height,
    }
}

#[allow(dead_code)]
pub(crate) trait Selectable {
    type Item;

    fn items(&self) -> &[Self::Item];
    fn selected_index(&self) -> Option<usize>;
    fn select(&mut self, index: usize);

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

    fn move_up_by(&mut self, gap: usize) {
        if let Some(index) = self.selected_index() {
            if index >= gap {
                self.select(index - gap);
            } else {
                self.select(0);
            }
        }
    }
    fn move_down_by(&mut self, gap: usize) {
        let items = self.items();
        if let Some(index) = self.selected_index() {
            if index + gap < items.len() {
                self.select(index + gap);
            } else if !items.is_empty() {
                self.select(items.len() - 1);
            }
        }
    }
}

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    pub offset: u32,
    pub limit: u32,
}

impl Page {
    pub const fn new(offset: u32, limit: u32) -> Self {
        Self { offset, limit }
    }
}

impl Default for Page {
    fn default() -> Self {
        Self {
            offset: 0,
            limit: 30,
        }
    }
}

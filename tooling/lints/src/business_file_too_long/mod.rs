//! 限制扣除测试条目和纯注释后的业务源文件行数。

mod line_count;
mod rule;

#[cfg(test)]
mod tests;

pub(crate) use rule::{BusinessFileTooLong, MINERAL_BUSINESS_FILE_TOO_LONG};

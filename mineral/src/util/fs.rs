use std::{
    hash::{Hash, Hasher},
    path::PathBuf,
};

pub fn hash_path(path: &PathBuf) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    hasher.finish()
}

pub fn name_from_path(path: &PathBuf) -> String {
    match path.file_stem() {
        Some(os_str) => match os_str.to_str() {
            Some(s) => s.to_string(),
            None => {
                tracing::warn!("路径文件名无法转换为 UTF-8: {:?}", path);
                "未知名称".to_string()
            }
        },
        None => {
            tracing::warn!("路径没有文件名: {:?}", path);
            "未知名称".to_string()
        }
    }
}

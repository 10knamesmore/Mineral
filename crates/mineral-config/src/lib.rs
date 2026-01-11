use derive_getters::Getters;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Getters)]
pub struct MineralConfig {
    /// 目录下应是一系列深度为 1 的目录, 每个目录都有对应的音乐
    music_dirs: Vec<PathBuf>,
}

impl Default for MineralConfig {
    fn default() -> Self {
        Self {
            music_dirs: vec![mineral_platform::dir::audio_dir()],
        }
    }
}

impl MineralConfig {
    fn load() -> Self {
        // TODO: 添加default config 和user config的覆盖加载逻辑
        MineralConfig::default()
    }
}

pub static CONFIG: Lazy<MineralConfig> = Lazy::new(MineralConfig::load);

use std::path::PathBuf;

use once_cell::sync::OnceCell;

#[derive(Debug)]
pub struct Config {
    // 目录下应是一系列深度为 1 的目录, 每个目录都有对应的音乐
    music_dirs: Vec<PathBuf>,
}

static INSTANCE: OnceCell<Config> = OnceCell::new();

impl Config {
    fn init() -> Self {
        let home_path = PathBuf::from(std::env::var("HOME").unwrap());
        let _config_path = home_path.join(".config/ncm_tui/config.toml");

        // TODO: 解析config file

        Config {
            music_dirs: vec![home_path.join("musics")],
        }
    }

    pub fn get() -> &'static Self {
        INSTANCE.get_or_init(Config::init)
    }

    pub fn music_dirs(&self) -> &Vec<PathBuf> {
        &self.music_dirs
    }
}

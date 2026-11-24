use std::{
    error::Error,
    fs,
    fs::OpenOptions,
    io,
    net::{SocketAddr, TcpListener},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

use directories::ProjectDirs;

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub translate_bind: String,        // HTTP 监听地址
    pub queue_capacity: usize,         // 最大等待任务数
    pub session_cache_capacity: usize, // 缓存语言方向数量
    pub max_text_bytes: usize,         // KB; 单次翻译文本上限
    pub response_timeout: u64,         // seconds; HTTP 请求等待上限
    pub body_limit: usize,             // KB; JSON 请求体上限
    pub worker_count: usize,           // 同时执行的翻译任务数，目前没有使用
}

impl Default for Config {
    fn default() -> Self {
        Self {
            translate_bind: "127.0.0.1:3000".to_string(),
            queue_capacity: 16,
            session_cache_capacity: 4,
            max_text_bytes: 16,
            response_timeout: 90,
            body_limit: 64,
            worker_count: 1,
        }
    }
}

pub fn config_path() -> io::Result<PathBuf> {
    let dirs = ProjectDirs::from("", "", "yi")
        .ok_or_else(|| io::Error::other("Cannot find config directory"))?;

    let dir = dirs.config_dir();

    fs::create_dir_all(dir)?;

    let path = dir.join("config.toml");

    let _file = OpenOptions::new().write(true).create_new(true).open(&path);

    Ok(path)
}

pub fn save_config(config: &Config) -> Result<(), Box<dyn Error>> {
    let path = config_path()?;

    let content = toml::to_string_pretty(config)?;
    fs::write(path, content)?;

    Ok(())
}

pub fn load_config() -> Result<Config, Box<dyn Error>> {
    let path = config_path()?;

    let content = fs::read_to_string(path)?;

    if content.is_empty() {
        let config = Config::default();
        save_config(&config)?;
        return Ok(config);
    }
    let config = toml::from_str(&content)?;
    Ok(config)
}

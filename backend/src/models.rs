use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadConfig {
    pub username: String,
    pub api_key: String,
    pub tags: String,
    pub exclude_tags: String,
    pub score_threshold: i64,
    pub limit: i64,
    pub save_path: String,
    pub proxy: String,
    pub proxy_port: String,
    pub concurrency: usize,
    pub sort_by_score: bool,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            username: String::new(),
            api_key: String::new(),
            tags: String::new(),
            exclude_tags: String::new(),
            score_threshold: 0,
            limit: 10,
            save_path: "danbooru_images".into(),
            proxy: String::new(),
            proxy_port: String::new(),
            concurrency: 5,
            sort_by_score: false,
        }
    }
}

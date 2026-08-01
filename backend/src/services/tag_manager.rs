use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::{error, info, warn};
use walkdir::WalkDir;

const CACHE_VERSION: u32 = 1;
const TAG_API: &str = "https://danbooru.donmai.us/tags.json";
const USER_AGENT: &str = "DanbooruTagProcessor/2.0";

const CATEGORY_ARTIST: i64 = 1;
const CATEGORY_CHARACTER: i64 = 4;
const CATEGORY_COPYRIGHT: i64 = 3;
const CATEGORY_CIRCLE: i64 = 5;
const CATEGORY_META: i64 = 6;

const FILTERED_CATEGORIES: &[i64] = &[CATEGORY_COPYRIGHT, CATEGORY_CIRCLE, CATEGORY_META];

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtistPrefix {
    #[default]
    Artist,
    At,
}

impl ArtistPrefix {
    pub fn format(&self, tag_name: &str) -> String {
        match self {
            ArtistPrefix::Artist => format!("artist:{}", tag_name),
            ArtistPrefix::At => format!("@{}", tag_name),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagCacheData {
    pub version: u32,
    pub data: HashMap<String, Option<i64>>,
}

#[derive(Debug, Deserialize)]
struct TagApiResult {
    category: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct TagCache {
    pub data: HashMap<String, Option<i64>>,
    file_path: PathBuf,
    dirty: bool,
}

impl TagCache {
    pub fn load(file_path: &Path) -> Self {
        let data = if file_path.exists() {
            fs::read_to_string(file_path)
                .ok()
                .and_then(|s| serde_json::from_str::<TagCacheData>(&s).ok())
                .filter(|c| c.version == CACHE_VERSION)
                .map(|c| c.data)
                .unwrap_or_default()
        } else {
            HashMap::new()
        };
        Self { data, file_path: file_path.to_path_buf(), dirty: false }
    }

    pub fn get(&self, tag: &str) -> Option<Option<i64>> {
        self.data.get(tag).copied()
    }

    pub fn set(&mut self, tag: &str, category: Option<i64>) {
        if self.data.get(tag) != Some(&category) {
            self.data.insert(tag.to_string(), category);
            self.dirty = true;
        }
    }

    pub fn save(&self) {
        if !self.dirty { return; }
        let cache_data = TagCacheData { version: CACHE_VERSION, data: self.data.clone() };
        if let Ok(json) = serde_json::to_string_pretty(&cache_data) {
            let _ = fs::write(&self.file_path, json);
            info!("Tag cache saved: {} entries → {:?}", self.data.len(), self.file_path);
        }
    }
}

pub type ProgressFn = dyn Fn(usize, usize, &str) + Send + Sync;

pub struct TagSorter {
    pub cache: TagCache,
    client: Client,
    artist_prefix: ArtistPrefix,
}

impl TagSorter {
    pub fn new(cache_file: &Path, proxy: &str, proxy_port: &str, artist_prefix: ArtistPrefix) -> Self {
        let mut builder = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(10));

        if !proxy.is_empty() {
            let proxy_url = format!("http://{}:{}", proxy, proxy_port);
            if let Ok(p) = reqwest::Proxy::all(&proxy_url) {
                builder = builder.proxy(p);
                info!("Tag sorter using proxy: {}", proxy_url);
            } else {
                warn!("Invalid proxy URL: {} — connecting directly", proxy_url);
            }
        }

        Self { cache: TagCache::load(cache_file), client: builder.build().unwrap(), artist_prefix }
    }

    fn format_tag(&self, tag_name: &str, category: Option<i64>) -> Option<(String, u8)> {
        if category.map_or(false, |c| FILTERED_CATEGORIES.contains(&c)) {
            return None;
        }
        let priority = match category {
            Some(CATEGORY_ARTIST) => 0,
            Some(CATEGORY_CHARACTER) => 1,
            _ => 2,
        };
        if category == Some(CATEGORY_ARTIST) {
            Some((self.artist_prefix.format(tag_name), priority))
        } else {
            Some((tag_name.to_string(), priority))
        }
    }

    pub async fn process_directory(
        &mut self,
        input_dir: &str,
        request_concurrency: usize,
        progress: Option<&ProgressFn>,
    ) -> String {
        let dir = Path::new(input_dir);
        if !dir.exists() {
            return format!("目录不存在: {}", input_dir);
        }

        let txt_files: Vec<PathBuf> = WalkDir::new(dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map_or(false, |ext| ext == "txt"))
            .map(|e| e.path().to_path_buf())
            .collect();

        if txt_files.is_empty() {
            return format!("目录 {} 中未找到 .txt 标签文件", input_dir);
        }

        info!("Tag sort: found {} .txt files in {}", txt_files.len(), input_dir);
        let total_files = txt_files.len();
        let sem = Arc::new(Semaphore::new(request_concurrency.max(1)));
        let mut total_original = 0usize;
        let mut total_final = 0usize;

        // Step 1: collect all unique uncached tags across all files
        let mut all_uncached: HashMap<String, Vec<usize>> = HashMap::new();
        let mut file_elements: Vec<(PathBuf, Vec<String>)> = Vec::with_capacity(total_files);

        for file_path in &txt_files {
            let content = match fs::read_to_string(file_path) {
                Ok(c) => c,
                Err(e) => { error!("读取文件失败 {:?}: {}", file_path, e); continue; }
            };
            let elements: Vec<String> = content
                .split(&[',', '\n', ';'][..])
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if elements.is_empty() { continue; }
            file_elements.push((file_path.clone(), elements));
        }

        if let Some(ref prog) = progress {
            prog(0, total_files, "收集标签中...");
        }

        // Collect uncached tags
        for (_file_path, elements) in &file_elements {
            for tag in elements {
                if self.cache.get(tag).is_none() {
                    all_uncached.entry(tag.clone()).or_default();
                }
            }
        }

        let uncached_count = all_uncached.len();
        info!("Tag sort: {} unique tags, {} uncached",
              file_elements.iter().map(|(_, e)| e.len()).sum::<usize>(), uncached_count);

        // Step 2: batch-fetch uncached tags in parallel
        if uncached_count > 0 {
            if let Some(ref prog) = progress {
                prog(0, uncached_count, &format!("正在查询 {} 个未缓存标签...", uncached_count));
            }

            let tags: Vec<String> = all_uncached.keys().cloned().collect();
            let mut handles = Vec::with_capacity(tags.len());
            let sem_clone = sem.clone();

            // Process in batches of request_concurrency * 2 for better throughput
            for tag in &tags {
                let c = self.client.clone();
                let t = tag.clone();
                let s = sem_clone.clone();
                handles.push(tokio::spawn(async move {
                    let sem = s;
                    let _permit = sem.acquire().await.ok();
                    for attempt in 0..=3 {
                        match c.get(TAG_API)
                            .query(&[("search[name]", t.as_str()), ("limit", "1")])
                            .send().await
                        {
                            Ok(resp) => {
                                if resp.status().as_u16() == 429 {
                                    let wait = resp.headers().get("retry-after")
                                        .and_then(|v| v.to_str().ok())
                                        .and_then(|v| v.parse().ok())
                                        .unwrap_or(5u64);
                                    tokio::time::sleep(Duration::from_secs(wait)).await;
                                    continue;
                                }
                                return match resp.json::<Vec<TagApiResult>>().await {
                                    Ok(data) => (t.clone(), data.first().and_then(|r| r.category)),
                                    Err(_) => (t.clone(), None),
                                };
                            }
                            Err(e) => {
                                if attempt < 3 {
                                    tokio::time::sleep(Duration::from_millis(250 * (attempt + 1) as u64)).await;
                                } else {
                                    warn!("Tag API failed for '{}': {}", t, e);
                                }
                            }
                        }
                    }
                    (t.clone(), None)
                }));
            }

            let mut resolved = 0usize;
            for handle in handles {
                match handle.await {
                    Ok((tag, cat)) => {
                        self.cache.set(&tag, cat);
                        resolved += 1;
                        if resolved % 50 == 0 || resolved == uncached_count {
                            if let Some(ref prog) = progress {
                                prog(resolved, uncached_count, &format!("标签查询 {}/{}", resolved, uncached_count));
                            }
                            info!("Tag cache: {}/{} resolved", resolved, uncached_count);
                        }
                    }
                    Err(e) => error!("Tag task panic: {}", e),
                }
            }
            info!("Tag sort: resolved {} uncached tags", resolved);
        }

        // Step 3: process files with cached data
        for (idx, (file_path, elements)) in file_elements.iter().enumerate() {
            total_original += elements.len();
            let mut results: Vec<(String, u8)> = Vec::new();

            for element in elements {
                match self.cache.get(element) {
                    Some(cat) => {
                        if let Some(formatted) = self.format_tag(element, cat) {
                            results.push(formatted);
                        }
                    }
                    None => {
                        // Still uncached (API failed) — keep as-is, priority 2 (general)
                        results.push((element.clone(), 2));
                    }
                }
            }

            results.sort_by_key(|(_, p)| *p);
            let final_tags: Vec<String> = results.into_iter().map(|(t, _)| t).collect();
            total_final += final_tags.len();

            if let Err(e) = fs::write(file_path, final_tags.join(",")) {
                error!("写入文件失败 {:?}: {}", file_path, e);
            }

            if (idx + 1) % 10 == 0 || idx + 1 == total_files {
                if let Some(ref prog) = progress {
                    prog(idx + 1, total_files, &format!("处理文件 {}/{}", idx + 1, total_files));
                }
            }
        }

        self.cache.save();

        let filtered = total_original.saturating_sub(total_final);
        let msg = format!(
            "标签排序完成!\n文件: {}\n标签: {} -> {} (过滤 {})\n缓存: {} 条",
            total_files, total_original, total_final, filtered, self.cache.data.len()
        );
        info!("{}", msg.replace('\n', " | "));
        msg
    }
}

// ============================================================================
// Bracket/Underscore Processing
// ============================================================================

pub fn process_kuohao(input_dir: &str) -> String {
    let dir = Path::new(input_dir);
    if !dir.exists() {
        return format!("目录不存在: {}", input_dir);
    }

    let mut changed = 0usize;
    let mut unchanged = 0usize;
    let mut errors = 0usize;

    let txt_files: Vec<PathBuf> = WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "txt"))
        .map(|e| e.path().to_path_buf())
        .collect();

    info!("Kuohao processing: {} .txt files in {}", txt_files.len(), input_dir);

    for file_path in &txt_files {
        match fs::read_to_string(file_path) {
            Ok(content) => {
                // Escape unescaped parentheses: only add \ before ( or ) if not already preceded by \
                let mut modified = String::with_capacity(content.len());
                let chars: Vec<char> = content.chars().collect();
                let mut i = 0;
                while i < chars.len() {
                    if chars[i] == '(' || chars[i] == ')' {
                        // Check if preceded by backslash
                        if i == 0 || chars[i - 1] != '\\' {
                            modified.push('\\');
                        }
                    }
                    modified.push(chars[i]);
                    i += 1;
                }
                modified = modified.replace('_', " ");
                if modified != content {
                    if fs::write(file_path, &modified).is_ok() {
                        changed += 1;
                    } else {
                        errors += 1;
                        error!("写入失败: {:?}", file_path);
                    }
                } else {
                    unchanged += 1;
                }
            }
            Err(e) => {
                error!("读取失败 {:?}: {}", file_path, e);
                errors += 1;
            }
        }
    }

    let msg = format!(
        "括号/下划线处理完成!\n文件: {}\n修改: {}, 未变: {}, 错误: {}",
        txt_files.len(), changed, unchanged, errors
    );
    info!("{}", msg.replace('\n', " | "));
    msg
}

pub async fn run_full_pipeline(
    input_dir: &str,
    cache_file: &str,
    request_concurrency: usize,
    proxy: &str,
    proxy_port: &str,
    artist_prefix: ArtistPrefix,
    progress: Option<&ProgressFn>,
) -> String {
    info!("Starting full tag pipeline for {}", input_dir);
    let cache_path = Path::new(cache_file);
    let mut sorter = TagSorter::new(cache_path, proxy, proxy_port, artist_prefix);

    if let Some(ref prog) = progress { prog(0, 2, "步骤 1/2: API 标签排序..."); }
    let result1 = sorter.process_directory(input_dir, request_concurrency, progress).await;
    info!("Pipeline step 1 complete: {}", result1.lines().next().unwrap_or(""));

    if let Some(ref prog) = progress { prog(1, 2, "步骤 2/2: 括号/下划线处理..."); }
    let result2 = process_kuohao(input_dir);
    info!("Pipeline step 2 complete: {}", result2.lines().next().unwrap_or(""));

    let msg = format!(
        "全流程处理完成!\n\n[步骤 1 - API 排序]\n{}\n\n[步骤 2 - 括号/下划线]\n{}",
        result1, result2
    );
    info!("Tag pipeline complete");
    msg
}

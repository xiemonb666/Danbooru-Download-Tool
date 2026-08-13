use crate::models::DownloadConfig;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

pub struct Database {
    conn: ConnectionPool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatabasePoolDiagnostics {
    pub total: usize,
    pub in_use: usize,
    pub idle: usize,
    pub capacity: usize,
}

struct ConnectionPool {
    path: PathBuf,
    idle: Mutex<Vec<Connection>>,
    available: Condvar,
    total: AtomicUsize,
    capacity: usize,
}

struct PooledConnection<'a> {
    pool: &'a ConnectionPool,
    connection: Option<Connection>,
}

impl ConnectionPool {
    fn new(path: &Path, connection: Connection) -> Self {
        Self {
            path: path.to_path_buf(),
            idle: Mutex::new(vec![connection]),
            available: Condvar::new(),
            total: AtomicUsize::new(1),
            capacity: 4,
        }
    }

    fn open_connection(&self) -> rusqlite::Result<Connection> {
        let connection = Connection::open(&self.path)?;
        configure_connection(&connection)?;
        Ok(connection)
    }

    fn lock(&self) -> Result<PooledConnection<'_>, rusqlite::Error> {
        let started = std::time::Instant::now();
        let mut idle = self.idle.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
        loop {
            if let Some(connection) = idle.pop() {
                let waited = started.elapsed();
                if waited >= std::time::Duration::from_millis(50) {
                    tracing::warn!(wait_ms = waited.as_millis() as u64, "SQLite 连接池等待较慢");
                }
                return Ok(PooledConnection { pool: self, connection: Some(connection) });
            }
            let total = self.total.load(Ordering::Acquire);
            if total < self.capacity && self.total.compare_exchange(total, total + 1, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                drop(idle);
                return match self.open_connection() {
                    Ok(connection) => {
                        let waited = started.elapsed();
                        if waited >= std::time::Duration::from_millis(50) {
                            tracing::warn!(wait_ms = waited.as_millis() as u64, "SQLite 新连接创建较慢");
                        }
                        Ok(PooledConnection { pool: self, connection: Some(connection) })
                    },
                    Err(error) => {
                        self.total.fetch_sub(1, Ordering::AcqRel);
                        self.available.notify_one();
                        Err(error)
                    }
                };
            }
            idle = self.available.wait(idle).map_err(|_| rusqlite::Error::InvalidQuery)?;
        }
    }

    fn diagnostics(&self) -> DatabasePoolDiagnostics {
        let idle = self.idle.lock().map(|connections| connections.len()).unwrap_or_default();
        let total = self.total.load(Ordering::Acquire);
        DatabasePoolDiagnostics { total, in_use: total.saturating_sub(idle), idle, capacity: self.capacity }
    }
}

impl Deref for PooledConnection<'_> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.connection.as_ref().expect("pooled connection already returned")
    }
}

impl DerefMut for PooledConnection<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection.as_mut().expect("pooled connection already returned")
    }
}

impl Drop for PooledConnection<'_> {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.take() {
            if let Ok(mut idle) = self.pool.idle.lock() {
                idle.push(connection);
                self.pool.available.notify_one();
            } else {
                self.pool.total.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }
}

fn configure_connection(connection: &Connection) -> rusqlite::Result<()> {
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.execute_batch("PRAGMA synchronous=NORMAL; PRAGMA cache_size=-64000; PRAGMA foreign_keys=ON;")
}

fn migrate_partial_v1_tables(conn: &Connection) -> rusqlite::Result<()> {
    if table_exists(conn, "roots")? {
        ensure_columns(
            conn,
            "roots",
            &[
                ("name", "TEXT NOT NULL DEFAULT ''"),
                ("windows_path", "TEXT"),
                ("linux_path", "TEXT"),
                ("indexing_status", "TEXT NOT NULL DEFAULT 'not_indexed'"),
                ("catalog_revision", "INTEGER NOT NULL DEFAULT 1"),
                ("created_at", "TEXT NOT NULL DEFAULT ''"),
                ("updated_at", "TEXT NOT NULL DEFAULT ''"),
            ],
        )?;
        backfill_timestamps(conn, "roots")?;
    }
    if table_exists(conn, "tasks")? {
        ensure_columns(
            conn,
            "tasks",
            &[
                ("progress_json", "TEXT NOT NULL DEFAULT '{}'"),
                ("result_json", "TEXT"),
                ("error_json", "TEXT"),
                ("revision", "INTEGER NOT NULL DEFAULT 1"),
                ("items_total", "INTEGER NOT NULL DEFAULT 0"),
                ("items_completed", "INTEGER NOT NULL DEFAULT 0"),
                ("bytes_total", "INTEGER NOT NULL DEFAULT 0"),
                ("bytes_processed", "INTEGER NOT NULL DEFAULT 0"),
                ("resumable", "INTEGER NOT NULL DEFAULT 0"),
                ("created_at", "TEXT NOT NULL DEFAULT ''"),
                ("updated_at", "TEXT NOT NULL DEFAULT ''"),
                ("started_at", "TEXT"),
                ("finished_at", "TEXT"),
            ],
        )?;
        backfill_timestamps(conn, "tasks")?;
    }
    if table_exists(conn, "media_files")? {
        ensure_columns(
            conn,
            "media_files",
            &[
                ("post_id", "INTEGER"),
                ("variant", "TEXT NOT NULL DEFAULT 'original'"),
                ("byte_size", "INTEGER NOT NULL DEFAULT 0"),
                ("sha256", "TEXT"),
                ("md5", "TEXT"),
                ("width", "INTEGER"),
                ("height", "INTEGER"),
                ("duration", "REAL"),
                ("catalog_seq", "INTEGER"),
                ("directory_key", "TEXT NOT NULL DEFAULT ''"),
                ("short_edge", "INTEGER"),
                ("file_mtime", "INTEGER NOT NULL DEFAULT 0"),
                ("catalog_revision", "INTEGER NOT NULL DEFAULT 1"),
                ("status", "TEXT NOT NULL DEFAULT 'active'"),
                ("created_at", "TEXT NOT NULL DEFAULT ''"),
                ("updated_at", "TEXT NOT NULL DEFAULT ''"),
            ],
        )?;
        backfill_timestamps(conn, "media_files")?;
    }
    if table_exists(conn, "library_reindex_staging")? {
        ensure_columns(
            conn,
            "library_reindex_staging",
            &[("file_mtime", "INTEGER NOT NULL DEFAULT 0")],
        )?;
    }
    if table_exists(conn, "task_items")? {
        ensure_columns(
            conn,
            "task_items",
            &[
                ("status", "TEXT NOT NULL DEFAULT 'queued'"),
                ("payload_json", "TEXT NOT NULL DEFAULT '{}'"),
                ("result_json", "TEXT"),
                ("error_json", "TEXT"),
                ("attempts", "INTEGER NOT NULL DEFAULT 0"),
                ("updated_at", "TEXT NOT NULL DEFAULT ''"),
            ],
        )?;
        conn.execute(
            "UPDATE task_items SET updated_at=datetime('now') WHERE updated_at=''",
            [],
        )?;
    }
    if table_exists(conn, "quarantine")? {
        ensure_columns(
            conn,
            "quarantine",
            &[
                ("media_file_id", "TEXT"),
                ("sha256", "TEXT"),
                ("quarantined_at", "TEXT NOT NULL DEFAULT ''"),
                ("restored_at", "TEXT"),
            ],
        )?;
        conn.execute(
            "UPDATE quarantine SET quarantined_at=datetime('now') WHERE quarantined_at=''",
            [],
        )?;
    }
    if table_exists(conn, "posts")? {
        ensure_columns(
            conn,
            "posts",
            &[
                ("md5", "TEXT"),
                ("rating", "TEXT NOT NULL DEFAULT 'g'"),
                ("score", "INTEGER NOT NULL DEFAULT 0"),
                ("fav_count", "INTEGER NOT NULL DEFAULT 0"),
                ("width", "INTEGER NOT NULL DEFAULT 0"),
                ("height", "INTEGER NOT NULL DEFAULT 0"),
                ("file_ext", "TEXT"),
                ("file_size", "INTEGER"),
                ("source", "TEXT"),
                ("duration", "REAL"),
                ("posted_at", "INTEGER"),
                ("status", "TEXT NOT NULL DEFAULT 'available'"),
                ("tag_string", "TEXT NOT NULL DEFAULT ''"),
                ("tag_string_general", "TEXT NOT NULL DEFAULT ''"),
                ("tag_string_character", "TEXT NOT NULL DEFAULT ''"),
                ("tag_string_copyright", "TEXT NOT NULL DEFAULT ''"),
                ("tag_string_artist", "TEXT NOT NULL DEFAULT ''"),
                ("tag_string_meta", "TEXT NOT NULL DEFAULT ''"),
                ("fetched_at", "TEXT NOT NULL DEFAULT ''"),
            ],
        )?;
        conn.execute(
            "UPDATE posts SET fetched_at=datetime('now') WHERE fetched_at=''",
            [],
        )?;
    }
    if table_exists(conn, "tags")? {
        ensure_columns(
            conn,
            "tags",
            &[
                ("category", "INTEGER NOT NULL DEFAULT 0"),
                ("post_count", "INTEGER NOT NULL DEFAULT 0"),
                ("updated_at", "TEXT NOT NULL DEFAULT ''"),
            ],
        )?;
        conn.execute(
            "UPDATE tags SET updated_at=datetime('now') WHERE updated_at=''",
            [],
        )?;
    }
    Ok(())
}

fn table_exists(conn: &Connection, table: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |row| row.get(0),
    )
}

fn ensure_columns(
    conn: &Connection,
    table: &str,
    columns: &[(&str, &str)],
) -> rusqlite::Result<()> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let existing = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<std::collections::HashSet<_>, _>>()?;
    drop(statement);
    for (name, definition) in columns {
        if !existing.contains(*name) {
            conn.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {name} {definition};"
            ))?;
        }
    }
    Ok(())
}

fn backfill_timestamps(conn: &Connection, table: &str) -> rusqlite::Result<()> {
    conn.execute_batch(&format!(
        "UPDATE {table} SET
            created_at=CASE WHEN created_at='' THEN datetime('now') ELSE created_at END,
            updated_at=CASE WHEN updated_at='' THEN datetime('now') ELSE updated_at END;"
    ))
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DownloadRecord {
    pub id: i64,
    pub task_id: String,
    pub tags: String,
    pub exclude_tags: String,
    pub score_threshold: i64,
    pub limit_count: i64,
    pub save_path: String,
    pub total_images: i64,
    pub failed_count: i64,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImageMeta {
    pub id: i64,
    pub danbooru_id: i64,
    pub file_path: String,
    pub tags: String,
    pub file_size: i64,
    pub width: u32,
    pub height: u32,
    pub file_ext: String,
    pub score: i64,
    pub hash: Option<String>,
    pub download_task_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RootRecord {
    pub id: String,
    pub name: String,
    pub windows_path: Option<String>,
    pub linux_path: Option<String>,
    pub indexing_status: String,
    pub catalog_revision: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MediaFileRecord {
    pub id: String,
    pub root_id: String,
    pub post_id: Option<i64>,
    pub relative_path: String,
    pub variant: String,
    pub mime_type: String,
    pub byte_size: i64,
    pub sha256: Option<String>,
    pub md5: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration: Option<f64>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MediaFileInput {
    pub id: String,
    pub root_id: String,
    pub post_id: Option<i64>,
    pub relative_path: String,
    pub variant: String,
    pub mime_type: String,
    pub byte_size: i64,
    pub sha256: Option<String>,
    pub md5: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaIndexFingerprint {
    pub id: String,
    pub byte_size: i64,
    pub file_mtime: i64,
    pub width: Option<i64>,
    pub height: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LibraryMediaPage {
    pub items: Vec<MediaFileRecord>,
    pub total: i64,
    pub previous_cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub catalog_revision: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryMediaFilters {
    pub score_min: Option<i64>,
    pub score_max: Option<i64>,
    pub min_resolution: Option<i64>,
    pub max_resolution: Option<i64>,
    pub post_created_from: Option<i64>,
    pub post_created_to: Option<i64>,
    pub relative_directory: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LibraryScoreRange {
    pub score_min: i64,
    pub score_max: i64,
    pub count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LibraryResolutionRange {
    pub resolution_min: i64,
    pub resolution_max: i64,
    pub count: i64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LibraryMediaNumberedPage {
    pub items: Vec<MediaFileRecord>,
    pub total: i64,
    pub page: usize,
    pub total_pages: usize,
    pub score_ranges: Vec<LibraryScoreRange>,
    pub resolution_ranges: Vec<LibraryResolutionRange>,
}

fn library_query_filter_parts(
    root_id: &str,
    exact_tag_query: &str,
    filters: &LibraryMediaFilters,
) -> (String, Vec<rusqlite::types::Value>) {
    let mut tags = Vec::<String>::new();
    for tag in exact_tag_query.split_whitespace() {
        if !tags.iter().any(|existing| existing == tag) {
            tags.push(tag.to_string());
        }
    }
    let mut clause = String::new();
    let mut values = vec![root_id.to_string().into()];
    for tag in tags {
        let parameter = values.len() + 1;
        clause.push_str(&format!(
            " AND EXISTS (
                SELECT 1 FROM post_tags pt
                JOIN tags t ON t.id=pt.tag_id
                WHERE pt.post_id=m.post_id AND t.name=?{parameter}
              )"
        ));
        values.push(rusqlite::types::Value::Text(tag));
    }
    if let Some(score_min) = filters.score_min {
        let parameter = values.len() + 1;
        clause.push_str(&format!(
            " AND COALESCE((SELECT score FROM posts p WHERE p.id=m.post_id), 0)>=?{parameter}"
        ));
        values.push(rusqlite::types::Value::Integer(score_min));
    }
    if let Some(score_max) = filters.score_max {
        let parameter = values.len() + 1;
        clause.push_str(&format!(
            " AND COALESCE((SELECT score FROM posts p WHERE p.id=m.post_id), 0)<=?{parameter}"
        ));
        values.push(rusqlite::types::Value::Integer(score_max));
    }
    if let Some(post_created_from) = filters.post_created_from {
        let parameter = values.len() + 1;
        clause.push_str(&format!(
            " AND (SELECT posted_at FROM posts p WHERE p.id=m.post_id)>=?{parameter}"
        ));
        values.push(rusqlite::types::Value::Integer(post_created_from));
    }
    if let Some(post_created_to) = filters.post_created_to {
        let parameter = values.len() + 1;
        clause.push_str(&format!(
            " AND (SELECT posted_at FROM posts p WHERE p.id=m.post_id)<=?{parameter}"
        ));
        values.push(rusqlite::types::Value::Integer(post_created_to));
    }
    let minimum_resolution = filters.min_resolution.filter(|value| *value > 0);
    if minimum_resolution.is_some() || filters.max_resolution.is_some() {
        clause.push_str(" AND m.width IS NOT NULL AND m.height IS NOT NULL");
    }
    if let Some(min_resolution) = minimum_resolution {
        let parameter = values.len() + 1;
        clause.push_str(&format!(" AND MIN(m.width, m.height)>=?{parameter}"));
        values.push(rusqlite::types::Value::Integer(min_resolution));
    }
    if let Some(max_resolution) = filters.max_resolution {
        let parameter = values.len() + 1;
        clause.push_str(&format!(" AND MIN(m.width, m.height)<=?{parameter}"));
        values.push(rusqlite::types::Value::Integer(max_resolution));
    }
    if let Some(directory) = filters.relative_directory.as_deref() {
        let parameter = values.len() + 1;
        if directory.is_empty() {
            clause.push_str(" AND instr(m.relative_path, '/')=0");
        } else {
            clause.push_str(&format!(
                " AND substr(m.relative_path, 1, length(?{parameter}))=?{parameter}
                   AND substr(m.relative_path, length(?{parameter}) + 1, 1)='/'"
            ));
            values.push(rusqlite::types::Value::Text(directory.to_string()));
        }
    }
    (clause, values)
}

fn dynamic_numeric_range_size(minimum: i64, maximum: i64) -> i64 {
    let span = maximum.saturating_sub(minimum).saturating_add(1).max(1) as u64;
    let target_size = span.div_ceil(8).max(1);
    let mut scale = 1_u64;
    while scale <= target_size / 10 {
        scale = scale.saturating_mul(10);
    }
    for multiplier in [1_u64, 2, 5, 10] {
        let candidate = multiplier.saturating_mul(scale);
        if candidate >= target_size {
            return candidate.min(i64::MAX as u64) as i64;
        }
    }
    i64::MAX
}

fn dynamic_numeric_ranges(values: Vec<(i64, i64)>) -> Vec<(i64, i64, i64)> {
    let (Some((minimum, _)), Some((maximum, _))) = (values.first(), values.last()) else {
        return Vec::new();
    };
    let range_size = dynamic_numeric_range_size(*minimum, *maximum).max(1);
    let range_start = minimum.div_euclid(range_size).saturating_mul(range_size);
    let mut ranges = Vec::<(i64, i64, i64)>::new();
    for (value, count) in values {
        let offset = value.saturating_sub(range_start);
        let range_min =
            range_start.saturating_add(offset.div_euclid(range_size).saturating_mul(range_size));
        let range_max = range_min.saturating_add(range_size.saturating_sub(1));
        if let Some((_, _, last_count)) = ranges
            .last_mut()
            .filter(|(minimum, _, _)| *minimum == range_min)
        {
            *last_count = last_count.saturating_add(count);
        } else {
            ranges.push((range_min, range_max, count));
        }
    }
    ranges
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PostRecordInput {
    pub id: i64,
    pub md5: Option<String>,
    pub rating: String,
    pub score: i64,
    pub fav_count: i64,
    pub width: i64,
    pub height: i64,
    pub file_ext: Option<String>,
    pub file_size: Option<i64>,
    pub source: Option<String>,
    pub duration: Option<f64>,
    #[serde(default)]
    pub post_created_at: Option<String>,
    pub status: String,
    pub tag_string: String,
    pub tag_string_general: String,
    pub tag_string_character: String,
    pub tag_string_copyright: String,
    pub tag_string_artist: String,
    pub tag_string_meta: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PostTagInput {
    pub name: String,
    pub category: i64,
    pub post_count: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PostLibraryTag {
    pub name: String,
    pub category: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PostLibraryMetadata {
    pub post_id: i64,
    pub rating: String,
    pub post_created_at: Option<i64>,
    pub tags: Vec<PostLibraryTag>,
}

impl PostTagInput {
    pub fn new(name: impl Into<String>, category: i64) -> Self {
        Self {
            name: name.into(),
            category,
            post_count: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TaskRecord {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub payload: serde_json::Value,
    pub progress: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub error: Option<serde_json::Value>,
    pub revision: i64,
    pub items_total: i64,
    pub items_completed: i64,
    pub bytes_total: i64,
    pub bytes_processed: i64,
    pub resumable: bool,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DownloadTaskHistoryCursor {
    pub updated_at: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TaskItemInput {
    pub item_key: String,
    pub status: String,
    pub payload: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub error: Option<serde_json::Value>,
    pub attempts: i64,
}

impl TaskItemInput {
    #[cfg(test)]
    pub fn new(item_key: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            item_key: item_key.into(),
            status: "queued".to_string(),
            payload,
            result: None,
            error: None,
            attempts: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TaskItemRecord {
    pub id: i64,
    pub task_id: String,
    pub item_key: String,
    pub status: String,
    pub payload: serde_json::Value,
    pub result: Option<serde_json::Value>,
    pub error: Option<serde_json::Value>,
    pub attempts: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskItemCounts {
    pub total: u64,
    pub queued: u64,
    pub completed: u64,
    pub skipped: u64,
    pub failed: u64,
    pub retryable_failed: u64,
    pub completed_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TaskItemsPage {
    pub items: Vec<TaskItemRecord>,
    pub next_cursor: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QuarantineInput {
    pub id: String,
    pub root_id: String,
    pub media_file_id: Option<String>,
    pub original_relative_path: String,
    pub quarantine_relative_path: String,
    pub reason: String,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct QuarantineRecord {
    pub id: String,
    pub root_id: String,
    pub media_file_id: Option<String>,
    pub original_relative_path: String,
    pub quarantine_relative_path: String,
    pub reason: String,
    pub sha256: Option<String>,
    pub quarantined_at: String,
    pub restored_at: Option<String>,
}

#[allow(dead_code)]
impl Database {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        configure_connection(&conn)?;

        let db = Self {
            conn: ConnectionPool::new(path, conn),
        };
        db.init_tables()?;
        Ok(db)
    }

    pub fn pool_diagnostics(&self) -> DatabasePoolDiagnostics {
        self.conn.diagnostics()
    }

    pub fn health_check(&self) -> rusqlite::Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
            .map(|_| ())
    }

    fn init_tables(&self) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        migrate_partial_v1_tables(&transaction)?;
        transaction.execute_batch(
            "CREATE TABLE IF NOT EXISTS download_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL UNIQUE,
                tags TEXT NOT NULL DEFAULT '',
                exclude_tags TEXT NOT NULL DEFAULT '',
                score_threshold INTEGER NOT NULL DEFAULT 0,
                limit_count INTEGER NOT NULL DEFAULT 0,
                save_path TEXT NOT NULL DEFAULT '',
                total_images INTEGER NOT NULL DEFAULT 0,
                failed_count INTEGER NOT NULL DEFAULT 0,
                started_at TEXT NOT NULL DEFAULT (datetime('now')),
                finished_at TEXT,
                status TEXT NOT NULL DEFAULT 'running'
            );

            CREATE INDEX IF NOT EXISTS idx_dl_task_id ON download_history(task_id);
            CREATE INDEX IF NOT EXISTS idx_dl_status ON download_history(status);

            CREATE TABLE IF NOT EXISTS image_metadata (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                danbooru_id INTEGER NOT NULL,
                file_path TEXT NOT NULL,
                tags TEXT NOT NULL DEFAULT '',
                file_size INTEGER NOT NULL DEFAULT 0,
                width INTEGER NOT NULL DEFAULT 0,
                height INTEGER NOT NULL DEFAULT 0,
                file_ext TEXT NOT NULL DEFAULT 'jpg',
                score INTEGER NOT NULL DEFAULT 0,
                hash TEXT,
                download_task_id TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(danbooru_id, file_path)
            );

            CREATE INDEX IF NOT EXISTS idx_img_danbooru_id ON image_metadata(danbooru_id);
            CREATE INDEX IF NOT EXISTS idx_img_task_id ON image_metadata(download_task_id);
            CREATE INDEX IF NOT EXISTS idx_img_tags ON image_metadata(tags);

            CREATE TABLE IF NOT EXISTS tag_category_cache (
                tag_name TEXT PRIMARY KEY,
                category INTEGER,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_tag_cat ON tag_category_cache(category);

            CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS roots (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                windows_path TEXT,
                linux_path TEXT,
                indexing_status TEXT NOT NULL DEFAULT 'not_indexed',
                catalog_revision INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                CHECK (windows_path IS NOT NULL OR linux_path IS NOT NULL)
            );

            CREATE TABLE IF NOT EXISTS posts (
                id INTEGER PRIMARY KEY,
                md5 TEXT,
                rating TEXT NOT NULL DEFAULT 'g',
                score INTEGER NOT NULL DEFAULT 0,
                fav_count INTEGER NOT NULL DEFAULT 0,
                width INTEGER NOT NULL DEFAULT 0,
                height INTEGER NOT NULL DEFAULT 0,
                file_ext TEXT,
                file_size INTEGER,
                source TEXT,
                duration REAL,
                posted_at INTEGER,
                status TEXT NOT NULL DEFAULT 'available',
                tag_string TEXT NOT NULL DEFAULT '',
                tag_string_general TEXT NOT NULL DEFAULT '',
                tag_string_character TEXT NOT NULL DEFAULT '',
                tag_string_copyright TEXT NOT NULL DEFAULT '',
                tag_string_artist TEXT NOT NULL DEFAULT '',
                tag_string_meta TEXT NOT NULL DEFAULT '',
                fetched_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_posts_md5
                ON posts(md5) WHERE md5 IS NOT NULL AND md5 != '';
            CREATE INDEX IF NOT EXISTS idx_posts_posted_at
                ON posts(posted_at, id) WHERE posted_at IS NOT NULL;

            CREATE TABLE IF NOT EXISTS media_files (
                id TEXT PRIMARY KEY,
                root_id TEXT NOT NULL REFERENCES roots(id) ON DELETE RESTRICT,
                post_id INTEGER REFERENCES posts(id) ON DELETE SET NULL,
                relative_path TEXT NOT NULL,
                variant TEXT NOT NULL DEFAULT 'original',
                mime_type TEXT NOT NULL,
                byte_size INTEGER NOT NULL DEFAULT 0,
                sha256 TEXT,
                md5 TEXT,
                width INTEGER,
                height INTEGER,
                duration REAL,
                catalog_seq INTEGER,
                directory_key TEXT NOT NULL DEFAULT '',
                short_edge INTEGER,
                file_mtime INTEGER NOT NULL DEFAULT 0,
                catalog_revision INTEGER NOT NULL DEFAULT 1,
                status TEXT NOT NULL DEFAULT 'active',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(root_id, relative_path)
            );
            CREATE INDEX IF NOT EXISTS idx_media_root_id ON media_files(root_id, id);
            CREATE INDEX IF NOT EXISTS idx_media_catalog_cursor
                ON media_files(root_id, catalog_revision, catalog_seq, id) WHERE status='active';
            CREATE INDEX IF NOT EXISTS idx_media_root_cursor
                ON media_files(root_id, catalog_seq, id) WHERE status='active';
            CREATE INDEX IF NOT EXISTS idx_media_directory_cursor
                ON media_files(root_id, catalog_revision, directory_key, catalog_seq, id)
                WHERE status='active';
            CREATE INDEX IF NOT EXISTS idx_media_short_edge
                ON media_files(root_id, catalog_revision, short_edge, catalog_seq)
                WHERE status='active';
            CREATE TRIGGER IF NOT EXISTS trg_media_catalog_seq
            AFTER INSERT ON media_files
            WHEN NEW.catalog_seq IS NULL
            BEGIN
                UPDATE media_files SET catalog_seq=NEW.rowid WHERE rowid=NEW.rowid;
            END;
            CREATE INDEX IF NOT EXISTS idx_media_post_id ON media_files(post_id);
            CREATE INDEX IF NOT EXISTS idx_media_md5 ON media_files(md5);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_media_root_path_unique
                ON media_files(root_id, relative_path);

            CREATE TABLE IF NOT EXISTS library_reindex_staging (
                task_id TEXT NOT NULL,
                root_id TEXT NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
                id TEXT NOT NULL,
                post_id INTEGER,
                relative_path TEXT NOT NULL,
                variant TEXT NOT NULL,
                mime_type TEXT NOT NULL,
                byte_size INTEGER NOT NULL,
                sha256 TEXT,
                md5 TEXT,
                width INTEGER,
                height INTEGER,
                duration REAL,
                catalog_seq INTEGER NOT NULL,
                directory_key TEXT NOT NULL,
                short_edge INTEGER,
                file_mtime INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(task_id, id),
                UNIQUE(task_id, root_id, relative_path)
            );
            CREATE INDEX IF NOT EXISTS idx_library_reindex_staging_root
                ON library_reindex_staging(task_id, root_id, catalog_seq);
            CREATE TABLE IF NOT EXISTS library_reindex_post_staging (
                task_id TEXT NOT NULL,
                root_id TEXT NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
                post_id INTEGER NOT NULL,
                post_json TEXT NOT NULL,
                tags_json TEXT NOT NULL,
                PRIMARY KEY(task_id, post_id)
            );

            CREATE TABLE IF NOT EXISTS downloaded_post_locations (
                root_id TEXT NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
                relative_directory TEXT NOT NULL,
                post_id INTEGER NOT NULL,
                last_task_id TEXT,
                downloaded_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (root_id, relative_directory, post_id)
            );
            CREATE INDEX IF NOT EXISTS idx_downloaded_post_locations_lookup
                ON downloaded_post_locations(root_id, relative_directory, post_id);

            CREATE TABLE IF NOT EXISTS tags (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                category INTEGER NOT NULL DEFAULT 0,
                post_count INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS post_tags (
                post_id INTEGER NOT NULL REFERENCES posts(id) ON DELETE CASCADE,
                tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
                PRIMARY KEY (post_id, tag_id)
            );
            CREATE INDEX IF NOT EXISTS idx_post_tags_tag_id ON post_tags(tag_id, post_id);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_tags_name_unique ON tags(name);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_post_tags_pair_unique
                ON post_tags(post_id, tag_id);

            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                status TEXT NOT NULL,
                payload_json TEXT NOT NULL DEFAULT '{}',
                progress_json TEXT NOT NULL DEFAULT '{}',
                result_json TEXT,
                error_json TEXT,
                revision INTEGER NOT NULL DEFAULT 1,
                items_total INTEGER NOT NULL DEFAULT 0,
                items_completed INTEGER NOT NULL DEFAULT 0,
                bytes_total INTEGER NOT NULL DEFAULT 0,
                bytes_processed INTEGER NOT NULL DEFAULT 0,
                resumable INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                started_at TEXT,
                finished_at TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_tasks_status_updated ON tasks(status, updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_tasks_download_history
                ON tasks(kind, status, updated_at DESC, id DESC);

            CREATE TABLE IF NOT EXISTS task_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                item_key TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'queued',
                payload_json TEXT NOT NULL DEFAULT '{}',
                result_json TEXT,
                error_json TEXT,
                attempts INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(task_id, item_key)
            );
            CREATE INDEX IF NOT EXISTS idx_task_items_status ON task_items(task_id, status, id);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_task_items_task_key_unique
                ON task_items(task_id, item_key);

            CREATE TABLE IF NOT EXISTS quarantine (
                id TEXT PRIMARY KEY,
                root_id TEXT NOT NULL REFERENCES roots(id) ON DELETE RESTRICT,
                media_file_id TEXT REFERENCES media_files(id) ON DELETE SET NULL,
                original_relative_path TEXT NOT NULL,
                quarantine_relative_path TEXT NOT NULL,
                reason TEXT NOT NULL,
                sha256 TEXT,
                quarantined_at TEXT NOT NULL DEFAULT (datetime('now')),
                restored_at TEXT,
                UNIQUE(root_id, quarantine_relative_path)
            );
            CREATE INDEX IF NOT EXISTS idx_quarantine_root_active
                ON quarantine(root_id, restored_at, quarantined_at DESC);
            CREATE UNIQUE INDEX IF NOT EXISTS idx_quarantine_root_path_unique
                ON quarantine(root_id, quarantine_relative_path);

            INSERT OR IGNORE INTO schema_migrations(version) VALUES (1);
            INSERT OR IGNORE INTO schema_migrations(version) VALUES (2);
            INSERT OR IGNORE INTO schema_migrations(version) VALUES (3);
            INSERT OR IGNORE INTO schema_migrations(version) VALUES (4);
            PRAGMA user_version=4;
            ",
        )?;
        transaction.execute_batch(
            "UPDATE media_files
             SET catalog_seq=COALESCE(catalog_seq, rowid),
                 short_edge=CASE
                     WHEN width IS NULL OR height IS NULL THEN NULL
                     WHEN width < height THEN width ELSE height END,
                 catalog_revision=COALESCE(
                     (SELECT roots.catalog_revision FROM roots WHERE roots.id=media_files.root_id),
                     1
                 )
             WHERE catalog_seq IS NULL OR short_edge IS NULL;",
        )?;
        backfill_downloaded_post_locations(&transaction)?;
        transaction.commit()
    }

    // =========================================================================
    // Download History
    // =========================================================================

    pub fn was_post_downloaded_in_directory(
        &self,
        root_id: &str,
        relative_directory: &str,
        post_id: i64,
    ) -> rusqlite::Result<bool> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM downloaded_post_locations
                WHERE root_id=?1 AND relative_directory=?2 AND post_id=?3
             )",
            params![root_id, relative_directory, post_id],
            |row| row.get(0),
        )
    }

    pub fn record_downloaded_post_in_directory(
        &self,
        root_id: &str,
        relative_directory: &str,
        post_id: i64,
        task_id: Option<&str>,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO downloaded_post_locations(
                root_id, relative_directory, post_id, last_task_id
             ) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(root_id, relative_directory, post_id) DO UPDATE SET
                last_task_id=excluded.last_task_id,
                downloaded_at=datetime('now')",
            params![root_id, relative_directory, post_id, task_id],
        )?;
        Ok(())
    }

    pub fn start_download(&self, task_id: &str, config: &DownloadConfig) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO download_history (task_id, tags, exclude_tags, score_threshold, limit_count, save_path, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running')",
            params![task_id, config.tags, config.exclude_tags, config.score_threshold, config.limit, config.save_path],
        )?;
        Ok(())
    }

    pub fn finish_download(
        &self,
        task_id: &str,
        total: usize,
        failed: usize,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE download_history SET total_images=?1, failed_count=?2, finished_at=datetime('now'), status='completed' WHERE task_id=?3",
            params![total as i64, failed as i64, task_id],
        )?;
        Ok(())
    }

    pub fn fail_download(&self, task_id: &str, error: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE download_history SET finished_at=datetime('now'), status=?1 WHERE task_id=?2",
            params![format!("failed: {}", error), task_id],
        )?;
        Ok(())
    }

    pub fn get_download_history(&self, limit: usize) -> rusqlite::Result<Vec<DownloadRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, task_id, tags, exclude_tags, score_threshold, limit_count, save_path, total_images, failed_count, started_at, finished_at, status
             FROM download_history ORDER BY id DESC LIMIT ?1"
        )?;
        let records = stmt
            .query_map(params![limit as i64], map_download_record)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    // =========================================================================
    // Image Metadata
    // =========================================================================

    pub fn insert_image(&self, meta: &ImageMeta) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO image_metadata (danbooru_id, file_path, tags, file_size, width, height, file_ext, score, hash, download_task_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![meta.danbooru_id, meta.file_path, meta.tags, meta.file_size, meta.width, meta.height, meta.file_ext, meta.score, meta.hash, meta.download_task_id],
        )?;
        Ok(())
    }

    pub fn insert_images_batch(&self, metas: &[ImageMeta]) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO image_metadata (danbooru_id, file_path, tags, file_size, width, height, file_ext, score, hash, download_task_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
            )?;
            for meta in metas {
                stmt.execute(params![
                    meta.danbooru_id,
                    meta.file_path,
                    meta.tags,
                    meta.file_size,
                    meta.width,
                    meta.height,
                    meta.file_ext,
                    meta.score,
                    meta.hash,
                    meta.download_task_id
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_image_by_danbooru_id(
        &self,
        danbooru_id: i64,
    ) -> rusqlite::Result<Option<ImageMeta>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, danbooru_id, file_path, tags, file_size, width, height, file_ext, score, hash, download_task_id, created_at
             FROM image_metadata WHERE danbooru_id=?1"
        )?;
        let mut rows = stmt.query_map(params![danbooru_id], |row| {
            Ok(ImageMeta {
                id: row.get(0)?,
                danbooru_id: row.get(1)?,
                file_path: row.get(2)?,
                tags: row.get(3)?,
                file_size: row.get(4)?,
                width: row.get(5)?,
                height: row.get(6)?,
                file_ext: row.get(7)?,
                score: row.get(8)?,
                hash: row.get(9)?,
                download_task_id: row.get(10)?,
                created_at: row.get(11)?,
            })
        })?;
        rows.next().transpose()
    }

    pub fn search_images_by_tag(
        &self,
        tag: &str,
        limit: usize,
    ) -> rusqlite::Result<Vec<ImageMeta>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, danbooru_id, file_path, tags, file_size, width, height, file_ext, score, hash, download_task_id, created_at
             FROM image_metadata WHERE tags LIKE ?1 ORDER BY id DESC LIMIT ?2"
        )?;
        let pattern = format!("%{}%", tag);
        let records = stmt
            .query_map(params![pattern, limit as i64], |row| {
                Ok(ImageMeta {
                    id: row.get(0)?,
                    danbooru_id: row.get(1)?,
                    file_path: row.get(2)?,
                    tags: row.get(3)?,
                    file_size: row.get(4)?,
                    width: row.get(5)?,
                    height: row.get(6)?,
                    file_ext: row.get(7)?,
                    score: row.get(8)?,
                    hash: row.get(9)?,
                    download_task_id: row.get(10)?,
                    created_at: row.get(11)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    pub fn get_image_count(&self) -> rusqlite::Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM image_metadata", [], |r| r.get(0))
    }

    pub fn get_total_size(&self) -> rusqlite::Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(SUM(file_size), 0) FROM image_metadata",
            [],
            |r| r.get(0),
        )
    }

    // =========================================================================
    // Tag Category Cache
    // =========================================================================

    pub fn get_tag_category(&self, tag: &str) -> rusqlite::Result<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT category FROM tag_category_cache WHERE tag_name=?1")?;
        let mut rows = stmt.query_map(params![tag], |row| row.get::<_, Option<i64>>(0))?;
        match rows.next() {
            Some(Ok(cat)) => Ok(cat),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    pub fn set_tag_category(&self, tag: &str, category: Option<i64>) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO tag_category_cache (tag_name, category, updated_at) VALUES (?1, ?2, datetime('now'))",
            params![tag, category],
        )?;
        Ok(())
    }

    pub fn find_known_tag_category(&self, tag: &str) -> rusqlite::Result<Option<Option<i64>>> {
        let conn = self.conn.lock().unwrap();
        let cached = conn
            .query_row(
                "SELECT category FROM tag_category_cache WHERE tag_name=?1",
                [tag],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?;
        if cached.is_some() {
            return Ok(cached);
        }
        conn.query_row("SELECT category FROM tags WHERE name=?1", [tag], |row| {
            row.get::<_, i64>(0).map(Some)
        })
        .optional()
    }

    pub fn get_tag_cache_count(&self) -> rusqlite::Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM tag_category_cache", [], |r| r.get(0))
    }

    // =========================================================================
    // Stats
    // =========================================================================

    pub fn get_stats(&self) -> rusqlite::Result<serde_json::Value> {
        let conn = self.conn.lock().unwrap();
        let total_images: i64 =
            conn.query_row("SELECT COUNT(*) FROM image_metadata", [], |r| r.get(0))?;
        let total_size: i64 = conn.query_row(
            "SELECT COALESCE(SUM(file_size), 0) FROM image_metadata",
            [],
            |r| r.get(0),
        )?;
        let total_downloads: i64 = conn.query_row(
            "SELECT COUNT(*) FROM download_history WHERE status='completed'",
            [],
            |r| r.get(0),
        )?;
        let unique_tags: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT tag_name) FROM tag_category_cache",
            [],
            |r| r.get(0),
        )?;

        Ok(serde_json::json!({
            "total_images": total_images,
            "total_size_bytes": total_size,
            "total_size_mb": format!("{:.1}", total_size as f64 / 1_048_576.0),
            "total_downloads": total_downloads,
            "unique_tags_cached": unique_tags,
        }))
    }

    pub fn create_root(
        &self,
        id: &str,
        name: &str,
        windows_path: Option<&str>,
        linux_path: Option<&str>,
    ) -> rusqlite::Result<RootRecord> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO roots(id, name, windows_path, linux_path) VALUES (?1, ?2, ?3, ?4)",
            params![id, name, windows_path, linux_path],
        )?;
        query_root(&conn, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn get_root(&self, id: &str) -> rusqlite::Result<Option<RootRecord>> {
        let conn = self.conn.lock().unwrap();
        query_root(&conn, id)
    }

    pub fn list_roots(&self) -> rusqlite::Result<Vec<RootRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT id, name, windows_path, linux_path, indexing_status, catalog_revision, created_at, updated_at
             FROM roots ORDER BY created_at, id",
        )?;
        let roots = statement
            .query_map([], map_root)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(roots)
    }

    pub fn update_root(
        &self,
        id: &str,
        name: &str,
        windows_path: Option<&str>,
        linux_path: Option<&str>,
        indexing_status: &str,
    ) -> rusqlite::Result<RootRecord> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE roots SET name=?2, windows_path=?3, linux_path=?4,
                    indexing_status=?5, updated_at=datetime('now') WHERE id=?1",
            params![id, name, windows_path, linux_path, indexing_status],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        query_root(&conn, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn advance_catalog_revision(&self, root_id: &str) -> rusqlite::Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE roots
             SET catalog_revision=catalog_revision+1, updated_at=datetime('now')
             WHERE id=?1",
            [root_id],
        )?;
        conn.query_row(
            "SELECT catalog_revision FROM roots WHERE id=?1",
            [root_id],
            |row| row.get(0),
        )
    }

    pub fn remove_root_catalog(&self, id: &str) -> rusqlite::Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        transaction.execute("DELETE FROM quarantine WHERE root_id=?1", [id])?;
        transaction.execute("DELETE FROM media_files WHERE root_id=?1", [id])?;
        let deleted = transaction.execute("DELETE FROM roots WHERE id=?1", [id])? > 0;
        transaction.commit()?;
        Ok(deleted)
    }

    pub fn remove_missing_active_media_files(
        &self,
        root_id: &str,
        scanned_paths: &HashSet<String>,
    ) -> rusqlite::Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        let stale_ids = {
            let mut statement = transaction.prepare(
                "SELECT id, relative_path FROM media_files WHERE root_id=?1 AND status='active'",
            )?;
            let records = statement
                .query_map([root_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            records
                .into_iter()
                .filter_map(|(id, path)| (!scanned_paths.contains(&path)).then_some(id))
                .collect::<Vec<_>>()
        };
        for id in &stale_ids {
            transaction.execute("DELETE FROM media_files WHERE id=?1", [id])?;
        }
        transaction.commit()?;
        Ok(stale_ids.len())
    }

    pub fn list_media_files(
        &self,
        root_id: &str,
        after_id: Option<&str>,
        limit: usize,
    ) -> rusqlite::Result<Vec<MediaFileRecord>> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.clamp(1, 200) as i64;
        let mut statement = conn.prepare(
            "SELECT id, root_id, post_id, relative_path, variant, mime_type, byte_size,
                    sha256, md5, width, height, duration, status, created_at, updated_at
             FROM media_files
             WHERE root_id=?1 AND (?2 IS NULL OR id > ?2)
             ORDER BY id LIMIT ?3",
        )?;
        let media = statement
            .query_map(params![root_id, after_id, limit], map_media_file)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(media)
    }

    pub fn count_media_files(&self, root_id: &str) -> rusqlite::Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM media_files WHERE root_id=?1 AND status='active'",
            [root_id],
            |row| row.get(0),
        )
    }

    pub fn list_active_media_in_directory(
        &self,
        root_id: &str,
        relative_directory: &str,
        limit: usize,
    ) -> rusqlite::Result<Vec<MediaFileRecord>> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.clamp(1, 10_001) as i64;
        let mut statement = conn.prepare(
            "SELECT id, root_id, post_id, relative_path, variant, mime_type, byte_size,
                    sha256, md5, width, height, duration, status, created_at, updated_at
             FROM media_files
             WHERE root_id=?1 AND status='active'
               AND (
                    ?2=''
                    OR (
                        substr(relative_path, 1, length(?2))=?2
                        AND substr(relative_path, length(?2) + 1, 1)='/'
                    )
               )
             ORDER BY relative_path COLLATE BINARY, id
             LIMIT ?3",
        )?;
        let media = statement
            .query_map(params![root_id, relative_directory, limit], map_media_file)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(media)
    }

    pub fn list_active_source_media_in_directory(
        &self,
        root_id: &str,
        relative_directory: &str,
        limit: usize,
    ) -> rusqlite::Result<Vec<MediaFileRecord>> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.clamp(1, 10_001) as i64;
        let mut statement = conn.prepare(
            r#"SELECT id, root_id, post_id, relative_path, variant, mime_type, byte_size,
                    sha256, md5, width, height, duration, status, created_at, updated_at
             FROM media_files
             WHERE root_id=?1 AND status='active'
               AND (
                    ?2=''
                    OR (
                        substr(relative_path, 1, length(?2))=?2
                        AND substr(relative_path, length(?2) + 1, 1)='/'
                    )
               )
               AND lower('/' || replace(relative_path, '\', '/') || '/') NOT LIKE '%/.augmentation/%'
               AND lower('/' || replace(relative_path, '\', '/') || '/') NOT LIKE '%/.augmentation-metadata/%'
               AND lower('/' || replace(relative_path, '\', '/') || '/') NOT LIKE '%/.danbooru-quarantine/%'
             ORDER BY relative_path COLLATE BINARY, id
             LIMIT ?3"#,
        )?;
        let media = statement
            .query_map(params![root_id, relative_directory, limit], map_media_file)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(media)
    }

    pub fn list_library_media(
        &self,
        root_id: &str,
        after_id: Option<&str>,
        limit: usize,
        exact_tag_query: &str,
    ) -> rusqlite::Result<LibraryMediaPage> {
        self.list_library_media_filtered(
            root_id,
            after_id,
            limit,
            exact_tag_query,
            &LibraryMediaFilters::default(),
        )
    }

    pub fn list_library_media_filtered(
        &self,
        root_id: &str,
        after_id: Option<&str>,
        limit: usize,
        exact_tag_query: &str,
        filters: &LibraryMediaFilters,
    ) -> rusqlite::Result<LibraryMediaPage> {
        self.list_library_media_bidirectional(
            root_id,
            after_id,
            false,
            limit,
            exact_tag_query,
            filters,
        )
    }

    pub fn list_library_media_bidirectional(
        &self,
        root_id: &str,
        cursor_id: Option<&str>,
        before: bool,
        limit: usize,
        exact_tag_query: &str,
        filters: &LibraryMediaFilters,
    ) -> rusqlite::Result<LibraryMediaPage> {
        let limit = limit.clamp(1, 200);
        let (filter_clause, filter_values) =
            library_query_filter_parts(root_id, exact_tag_query, filters);
        let conn = self.conn.lock().unwrap();
        let catalog_revision = conn
            .query_row(
                "SELECT catalog_revision FROM roots WHERE id=?1",
                [root_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(1);
        let count_sql = format!(
            "SELECT COUNT(*) FROM media_files m
             WHERE m.root_id=?1 AND m.status='active'{filter_clause}"
        );
        let base_values = filter_values;
        let total = conn.query_row(
            &count_sql,
            rusqlite::params_from_iter(base_values.iter()),
            |row| row.get(0),
        )?;
        let cursor = cursor_id
            .map(|cursor| {
                conn.query_row(
                    "SELECT catalog_seq, id FROM media_files
                     WHERE root_id=?1 AND id=?2",
                    params![root_id, cursor],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
            })
            .transpose()?;
        let comparison = if before { "<" } else { ">" };
        let order = if before { "DESC" } else { "ASC" };
        let (cursor_clause, limit_parameter) = if cursor.is_some() {
            let cursor_seq_parameter = base_values.len() + 1;
            let cursor_id_parameter = cursor_seq_parameter + 1;
            (
                format!(
                    " AND (m.catalog_seq {comparison} ?{cursor_seq_parameter}
                            OR (m.catalog_seq=?{cursor_seq_parameter}
                                AND m.id {comparison} ?{cursor_id_parameter}))"
                ),
                cursor_id_parameter + 1,
            )
        } else {
            (String::new(), base_values.len() + 1)
        };
        let page_sql = format!(
            "SELECT m.id, m.root_id, m.post_id, m.relative_path, m.variant, m.mime_type,
                    m.byte_size, m.sha256, m.md5, m.width, m.height, m.duration, m.status,
                    m.created_at, m.updated_at
             FROM media_files m
             WHERE m.root_id=?1 AND m.status='active'{filter_clause}{cursor_clause}
             ORDER BY m.catalog_seq {order}, m.id {order} LIMIT ?{limit_parameter}"
        );
        let mut page_values = base_values;
        if let Some((sequence, id)) = cursor {
            page_values.push(rusqlite::types::Value::Integer(sequence));
            page_values.push(rusqlite::types::Value::Text(id));
        }
        page_values.push(rusqlite::types::Value::Integer((limit + 1) as i64));
        let mut statement = conn.prepare(&page_sql)?;
        let mut items = statement
            .query_map(
                rusqlite::params_from_iter(page_values.iter()),
                map_media_file,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let has_more = items.len() > limit;
        items.truncate(limit);
        if before {
            items.reverse();
        }
        let previous_cursor = (!items.is_empty() && (before && has_more || !before && cursor_id.is_some()))
            .then(|| items.first().expect("non-empty page").id.clone());
        let next_cursor = (!items.is_empty() && (!before && has_more || before && cursor_id.is_some()))
            .then(|| items.last().expect("non-empty page").id.clone());
        Ok(LibraryMediaPage {
            items,
            total,
            previous_cursor,
            next_cursor,
            catalog_revision,
        })
    }

    pub fn list_library_media_by_page(
        &self,
        root_id: &str,
        requested_page: usize,
        limit: usize,
        exact_tag_query: &str,
        filters: &LibraryMediaFilters,
    ) -> rusqlite::Result<LibraryMediaNumberedPage> {
        let limit = limit.clamp(1, 200);
        let (filter_clause, filter_values) =
            library_query_filter_parts(root_id, exact_tag_query, filters);
        let conn = self.conn.lock().unwrap();
        let count_sql = format!(
            "SELECT COUNT(*) FROM media_files m
             WHERE m.root_id=?1 AND m.status='active'{filter_clause}"
        );
        let total = conn.query_row(
            &count_sql,
            rusqlite::params_from_iter(filter_values.iter()),
            |row| row.get::<_, i64>(0),
        )?;
        let total_pages = if total <= 0 {
            0
        } else {
            ((total as usize).saturating_sub(1) / limit).saturating_add(1)
        };
        let page = if total_pages == 0 {
            1
        } else {
            requested_page.clamp(1, total_pages)
        };
        let offset = page.saturating_sub(1).saturating_mul(limit);
        let limit_parameter = filter_values.len() + 1;
        let offset_parameter = limit_parameter + 1;
        let page_sql = format!(
            "SELECT m.id, m.root_id, m.post_id, m.relative_path, m.variant, m.mime_type,
                    m.byte_size, m.sha256, m.md5, m.width, m.height, m.duration, m.status,
                    m.created_at, m.updated_at
             FROM media_files m
             WHERE m.root_id=?1 AND m.status='active'{filter_clause}
             ORDER BY m.id LIMIT ?{limit_parameter} OFFSET ?{offset_parameter}"
        );
        let mut page_values = filter_values.clone();
        page_values.push(rusqlite::types::Value::Integer(limit as i64));
        page_values.push(rusqlite::types::Value::Integer(
            offset.min(i64::MAX as usize) as i64,
        ));
        let mut statement = conn.prepare(&page_sql)?;
        let items = statement
            .query_map(
                rusqlite::params_from_iter(page_values.iter()),
                map_media_file,
            )?
            .collect::<Result<Vec<_>, _>>()?;

        let score_filters = LibraryMediaFilters {
            min_resolution: filters.min_resolution,
            max_resolution: filters.max_resolution,
            post_created_from: filters.post_created_from,
            post_created_to: filters.post_created_to,
            relative_directory: filters.relative_directory.clone(),
            ..LibraryMediaFilters::default()
        };
        let (score_filter_clause, score_filter_values) =
            library_query_filter_parts(root_id, exact_tag_query, &score_filters);
        let score_sql = format!(
            "SELECT COALESCE(p.score, 0), COUNT(*)
             FROM media_files m
             LEFT JOIN posts p ON p.id=m.post_id
             WHERE m.root_id=?1 AND m.status='active'{score_filter_clause}
             GROUP BY COALESCE(p.score, 0)
             ORDER BY COALESCE(p.score, 0)"
        );
        let mut score_statement = conn.prepare(&score_sql)?;
        let scores = score_statement
            .query_map(
                rusqlite::params_from_iter(score_filter_values.iter()),
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let score_ranges = dynamic_numeric_ranges(scores)
            .into_iter()
            .map(|(score_min, score_max, count)| LibraryScoreRange {
                score_min,
                score_max,
                count,
            })
            .collect();

        let resolution_filters = LibraryMediaFilters {
            score_min: filters.score_min,
            score_max: filters.score_max,
            post_created_from: filters.post_created_from,
            post_created_to: filters.post_created_to,
            relative_directory: filters.relative_directory.clone(),
            ..LibraryMediaFilters::default()
        };
        let (resolution_filter_clause, resolution_filter_values) =
            library_query_filter_parts(root_id, exact_tag_query, &resolution_filters);
        let resolution_sql = format!(
            "SELECT MIN(m.width, m.height), COUNT(*)
             FROM media_files m
             WHERE m.root_id=?1 AND m.status='active'
               AND m.width IS NOT NULL AND m.height IS NOT NULL
               AND MIN(m.width, m.height)>0{resolution_filter_clause}
             GROUP BY MIN(m.width, m.height)
             ORDER BY MIN(m.width, m.height)"
        );
        let mut resolution_statement = conn.prepare(&resolution_sql)?;
        let resolutions = resolution_statement
            .query_map(
                rusqlite::params_from_iter(resolution_filter_values.iter()),
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let resolution_ranges = dynamic_numeric_ranges(resolutions)
            .into_iter()
            .map(
                |(resolution_min, resolution_max, count)| LibraryResolutionRange {
                    resolution_min,
                    resolution_max,
                    count,
                },
            )
            .collect();

        Ok(LibraryMediaNumberedPage {
            items,
            total,
            page,
            total_pages,
            score_ranges,
            resolution_ranges,
        })
    }

    pub fn upsert_media_file(&self, media: &MediaFileInput) -> rusqlite::Result<MediaFileRecord> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        upsert_media_file_in_transaction(&transaction, media)?;
        let record = query_media_file(&transaction, &media.id)?
            .or(query_media_file_by_root_path(
                &transaction,
                &media.root_id,
                &media.relative_path,
            )?)
            .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        transaction.commit()?;
        Ok(record)
    }

    pub fn upsert_post_with_tags(
        &self,
        post: &PostRecordInput,
        tags: &[PostTagInput],
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        upsert_post_with_tags_in_transaction(&transaction, post, tags)?;
        transaction.commit()
    }

    pub fn register_downloaded_post(
        &self,
        post: &PostRecordInput,
        tags: &[PostTagInput],
        media_files: &[MediaFileInput],
        root_id: &str,
        relative_directory: &str,
        task_id: Option<&str>,
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        upsert_post_with_tags_in_transaction(&transaction, post, tags)?;
        for media in media_files {
            upsert_media_file_in_transaction(&transaction, media)?;
        }
        record_downloaded_post_location_in_transaction(
            &transaction,
            root_id,
            relative_directory,
            post.id,
            task_id,
        )?;
        transaction.commit()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn register_downloaded_post_and_finish_task_item(
        &self,
        task_id: &str,
        item_key: &str,
        status: &str,
        result: &serde_json::Value,
        post: &PostRecordInput,
        tags: &[PostTagInput],
        media_files: &[MediaFileInput],
        root_id: &str,
        relative_directory: &str,
    ) -> rusqlite::Result<()> {
        if !matches!(status, "completed" | "skipped") {
            return Err(rusqlite::Error::InvalidParameterName(
                "download task item terminal status".to_string(),
            ));
        }
        let result_json = json_to_sql(result)?;
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        upsert_post_with_tags_in_transaction(&transaction, post, tags)?;
        for media in media_files {
            upsert_media_file_in_transaction(&transaction, media)?;
        }
        record_downloaded_post_location_in_transaction(
            &transaction,
            root_id,
            relative_directory,
            post.id,
            Some(task_id),
        )?;
        let changed = transaction.execute(
            "UPDATE task_items
             SET status=?3, result_json=?4, error_json=NULL, attempts=attempts+1,
                 updated_at=datetime('now')
             WHERE task_id=?1 AND item_key=?2 AND status='queued'",
            params![task_id, item_key, status, result_json],
        )?;
        if changed != 1 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        transaction.commit()
    }

    pub fn insert_local_post_with_tags_if_missing(
        &self,
        post: &PostRecordInput,
        tags: &[PostTagInput],
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        insert_local_post_with_tags_if_missing_in_transaction(&transaction, post, tags)?;
        transaction.commit()
    }

    pub fn upsert_indexed_media_batch(
        &self,
        local_posts: &[(PostRecordInput, Vec<PostTagInput>)],
        media_files: &[MediaFileInput],
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        for (post, tags) in local_posts {
            insert_local_post_with_tags_if_missing_in_transaction(&transaction, post, tags)?;
        }
        for media in media_files {
            upsert_media_file_in_transaction(&transaction, media)?;
        }
        transaction.commit()
    }

    pub fn active_media_index_fingerprints(
        &self,
        root_id: &str,
    ) -> rusqlite::Result<HashMap<String, MediaIndexFingerprint>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT relative_path, id, byte_size, file_mtime, width, height
             FROM media_files WHERE root_id=?1 AND status='active'",
        )?;
        let fingerprints = statement
            .query_map([root_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    MediaIndexFingerprint {
                        id: row.get(1)?,
                        byte_size: row.get(2)?,
                        file_mtime: row.get(3)?,
                        width: row.get(4)?,
                        height: row.get(5)?,
                    },
                ))
            })?
            .collect();
        fingerprints
    }

    pub fn upsert_indexed_media_batch_with_mtimes(
        &self,
        local_posts: &[(PostRecordInput, Vec<PostTagInput>)],
        media_files: &[MediaFileInput],
        file_mtimes: &[(String, i64)],
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        for (post, tags) in local_posts {
            insert_local_post_with_tags_if_missing_in_transaction(&transaction, post, tags)?;
        }
        for media in media_files {
            upsert_media_file_in_transaction(&transaction, media)?;
        }
        for (id, file_mtime) in file_mtimes {
            transaction.execute(
                "UPDATE media_files SET file_mtime=?2 WHERE id=?1",
                params![id, file_mtime],
            )?;
        }
        transaction.commit()
    }

    pub fn begin_library_reindex_staging(
        &self,
        task_id: &str,
        root_id: &str,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM library_reindex_staging
             WHERE task_id=?1 AND root_id<>?2",
            params![task_id, root_id],
        )?;
        conn.execute(
            "DELETE FROM library_reindex_post_staging
             WHERE task_id=?1 AND root_id<>?2",
            params![task_id, root_id],
        )?;
        Ok(())
    }

    pub fn library_reindex_staging_fingerprints(
        &self,
        task_id: &str,
        root_id: &str,
    ) -> rusqlite::Result<HashMap<String, MediaIndexFingerprint>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT relative_path, id, byte_size, file_mtime, width, height
             FROM library_reindex_staging
             WHERE task_id=?1 AND root_id=?2",
        )?;
        let fingerprints = statement
            .query_map(params![task_id, root_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    MediaIndexFingerprint {
                        id: row.get(1)?,
                        byte_size: row.get(2)?,
                        file_mtime: row.get(3)?,
                        width: row.get(4)?,
                        height: row.get(5)?,
                    },
                ))
            })?
            .collect();
        fingerprints
    }

    pub fn prune_library_reindex_staging(
        &self,
        task_id: &str,
        root_id: &str,
        scanned_paths: &HashSet<String>,
    ) -> rusqlite::Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        let stale_ids = {
            let mut statement = transaction.prepare(
                "SELECT id, relative_path FROM library_reindex_staging
                 WHERE task_id=?1 AND root_id=?2",
            )?;
            let records = statement
                .query_map(params![task_id, root_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            records
                .into_iter()
                .filter_map(|(id, path)| (!scanned_paths.contains(&path)).then_some(id))
                .collect::<Vec<_>>()
        };
        for id in &stale_ids {
            transaction.execute(
                "DELETE FROM library_reindex_staging
                 WHERE task_id=?1 AND root_id=?2 AND id=?3",
                params![task_id, root_id, id],
            )?;
        }
        transaction.execute(
            "DELETE FROM library_reindex_post_staging
             WHERE task_id=?1 AND root_id=?2
               AND post_id NOT IN (
                   SELECT post_id FROM library_reindex_staging
                   WHERE task_id=?1 AND root_id=?2 AND post_id IS NOT NULL
               )",
            params![task_id, root_id],
        )?;
        transaction.commit()?;
        Ok(stale_ids.len())
    }

    pub fn stage_library_reindex_batch(
        &self,
        task_id: &str,
        root_id: &str,
        local_posts: &[(PostRecordInput, Vec<PostTagInput>)],
        media_files: &[MediaFileInput],
        file_mtimes: &[(String, i64)],
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        let next_sequence: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(catalog_seq), 0) + 1
             FROM library_reindex_staging WHERE task_id=?1 AND root_id=?2",
            params![task_id, root_id],
            |row| row.get(0),
        )?;
        for (post, tags) in local_posts {
            transaction.execute(
                "INSERT INTO library_reindex_post_staging(
                    task_id, root_id, post_id, post_json, tags_json)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(task_id, post_id) DO UPDATE SET
                    post_json=excluded.post_json, tags_json=excluded.tags_json",
                params![
                    task_id,
                    root_id,
                    post.id,
                    serde_json::to_string(post).map_err(|error| {
                        rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                    })?,
                    serde_json::to_string(tags).map_err(|error| {
                        rusqlite::Error::ToSqlConversionFailure(Box::new(error))
                    })?,
                ],
            )?;
        }
        for (index, media) in media_files.iter().enumerate() {
            let file_mtime = file_mtimes
                .iter()
                .find_map(|(id, value)| (id == &media.id).then_some(*value))
                .unwrap_or(0);
            transaction.execute(
                "INSERT INTO library_reindex_staging(
                    task_id, root_id, id, post_id, relative_path, variant, mime_type,
                    byte_size, sha256, md5, width, height, duration, catalog_seq,
                    directory_key, short_edge, file_mtime
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                           ?13, ?14, ?15,
                           CASE WHEN ?11 IS NULL OR ?12 IS NULL THEN NULL
                                WHEN ?11 < ?12 THEN ?11 ELSE ?12 END, ?16)
                 ON CONFLICT(task_id, id) DO UPDATE SET
                    post_id=excluded.post_id, relative_path=excluded.relative_path,
                    variant=excluded.variant, mime_type=excluded.mime_type,
                    byte_size=excluded.byte_size, sha256=excluded.sha256,
                    md5=excluded.md5, width=excluded.width, height=excluded.height,
                    duration=excluded.duration, directory_key=excluded.directory_key,
                    short_edge=excluded.short_edge, file_mtime=excluded.file_mtime",
                params![
                    task_id,
                    root_id,
                    media.id,
                    media.post_id,
                    media.relative_path,
                    media.variant,
                    media.mime_type,
                    media.byte_size,
                    media.sha256,
                    media.md5,
                    media.width,
                    media.height,
                    media.duration,
                    next_sequence.saturating_add(index as i64),
                    Path::new(&media.relative_path)
                        .parent()
                        .and_then(Path::to_str)
                        .unwrap_or("")
                        .replace('\\', "/"),
                    file_mtime,
                ],
            )?;
        }
        transaction.commit()
    }

    pub fn discard_library_reindex_staging(&self, task_id: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM library_reindex_staging WHERE task_id=?1",
            [task_id],
        )?;
        conn.execute(
            "DELETE FROM library_reindex_post_staging WHERE task_id=?1",
            [task_id],
        )?;
        Ok(())
    }

    pub fn commit_library_reindex(
        &self,
        task_id: &str,
        root_id: &str,
        expected_count: usize,
    ) -> rusqlite::Result<(i64, usize)> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        let staged_count: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM library_reindex_staging
             WHERE task_id=?1 AND root_id=?2",
            params![task_id, root_id],
            |row| row.get(0),
        )?;
        if staged_count != expected_count as i64 {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "reindex staged count {staged_count} does not match {expected_count}"
            )));
        }
        let staged_posts = {
            let mut statement = transaction.prepare(
                "SELECT post_json, tags_json FROM library_reindex_post_staging
                 WHERE task_id=?1 AND root_id=?2 ORDER BY post_id",
            )?;
            let rows = statement
                .query_map(params![task_id, root_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        for (post_json, tags_json) in staged_posts {
            let post: PostRecordInput = serde_json::from_str(&post_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            let tags: Vec<PostTagInput> = serde_json::from_str(&tags_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            insert_local_post_with_tags_if_missing_in_transaction(&transaction, &post, &tags)?;
        }
        let removed = transaction.execute(
            "DELETE FROM media_files WHERE root_id=?1 AND status='active'",
            [root_id],
        )?;
        let next_revision: i64 = transaction.query_row(
            "SELECT catalog_revision + 1 FROM roots WHERE id=?1",
            [root_id],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT INTO media_files(
                id, root_id, post_id, relative_path, variant, mime_type, byte_size,
                sha256, md5, width, height, duration, catalog_seq, directory_key,
                short_edge, file_mtime, catalog_revision, status, created_at, updated_at)
             SELECT id, root_id, post_id, relative_path, variant, mime_type, byte_size,
                    sha256, md5, width, height, duration, catalog_seq, directory_key,
                    short_edge, file_mtime, ?3, 'active', datetime('now'), datetime('now')
             FROM library_reindex_staging
             WHERE task_id=?1 AND root_id=?2 ORDER BY catalog_seq",
            params![task_id, root_id, next_revision],
        )?;
        transaction.execute(
            "UPDATE roots SET catalog_revision=?2, indexing_status='indexed',
                              updated_at=datetime('now') WHERE id=?1",
            params![root_id, next_revision],
        )?;
        transaction.execute(
            "DELETE FROM library_reindex_staging WHERE task_id=?1",
            [task_id],
        )?;
        transaction.execute(
            "DELETE FROM library_reindex_post_staging WHERE task_id=?1",
            [task_id],
        )?;
        transaction.commit()?;
        Ok((next_revision, removed))
    }

    pub fn get_post_library_metadata(
        &self,
        post_id: i64,
    ) -> rusqlite::Result<Option<PostLibraryMetadata>> {
        let conn = self.conn.lock().unwrap();
        let post_metadata = conn
            .query_row("SELECT rating, posted_at FROM posts WHERE id=?1", [post_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
            })
            .optional()?;
        let Some((rating, post_created_at)) = post_metadata else {
            return Ok(None);
        };
        let mut statement = conn.prepare(
            "SELECT t.name, t.category
             FROM post_tags pt
             JOIN tags t ON t.id=pt.tag_id
             WHERE pt.post_id=?1
             ORDER BY t.category, t.name COLLATE BINARY",
        )?;
        let tags = statement
            .query_map([post_id], |row| {
                Ok(PostLibraryTag {
                    name: row.get(0)?,
                    category: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(PostLibraryMetadata {
            post_id,
            rating,
            post_created_at,
            tags,
        }))
    }

    pub fn get_post_library_metadata_batch(
        &self,
        post_ids: &[i64],
    ) -> rusqlite::Result<HashMap<i64, PostLibraryMetadata>> {
        if post_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let mut unique = post_ids.to_vec();
        unique.sort_unstable();
        unique.dedup();
        let placeholders = (1..=unique.len())
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(",");
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT p.id, p.rating, p.posted_at, t.name, t.category
             FROM posts p
             LEFT JOIN post_tags pt ON pt.post_id=p.id
             LEFT JOIN tags t ON t.id=pt.tag_id
             WHERE p.id IN ({placeholders})
             ORDER BY p.id, t.category, t.name COLLATE BINARY"
        );
        let mut statement = conn.prepare(&sql)?;
        let mut result = HashMap::<i64, PostLibraryMetadata>::new();
        let mut rows = statement.query(rusqlite::params_from_iter(unique.iter()))?;
        while let Some(row) = rows.next()? {
            let post_id = row.get::<_, i64>(0)?;
            let metadata = result.entry(post_id).or_insert(PostLibraryMetadata {
                post_id,
                rating: row.get(1)?,
                post_created_at: row.get(2)?,
                tags: Vec::new(),
            });
            if let Some(name) = row.get::<_, Option<String>>(3)? {
                metadata.tags.push(PostLibraryTag {
                    name,
                    category: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                });
            }
        }
        Ok(result)
    }

    pub fn get_media_file(&self, id: &str) -> rusqlite::Result<Option<MediaFileRecord>> {
        let conn = self.conn.lock().unwrap();
        query_media_file(&conn, id)
    }

    pub fn find_media_by_post_or_md5(
        &self,
        post_id: Option<i64>,
        md5: Option<&str>,
    ) -> rusqlite::Result<Option<MediaFileRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT id, root_id, post_id, relative_path, variant, mime_type, byte_size,
                    sha256, md5, width, height, duration, status, created_at, updated_at
             FROM media_files
             WHERE status='active' AND ((?1 IS NOT NULL AND post_id=?1) OR (?2 IS NOT NULL AND md5=?2))
             ORDER BY id LIMIT 1",
        )?;
        let mut rows = statement.query_map(params![post_id, md5], map_media_file)?;
        rows.next().transpose()
    }

    pub fn find_active_media_for_download(
        &self,
        root_id: &str,
        post_id: Option<i64>,
        md5: Option<&str>,
        variant: &str,
    ) -> rusqlite::Result<Option<MediaFileRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT id, root_id, post_id, relative_path, variant, mime_type, byte_size,
                    sha256, md5, width, height, duration, status, created_at, updated_at
             FROM media_files
             WHERE root_id=?1 AND variant=?2 AND status='active'
               AND ((?3 IS NOT NULL AND post_id=?3) OR (?4 IS NOT NULL AND md5=?4))
             ORDER BY id LIMIT 1",
        )?;
        let mut rows =
            statement.query_map(params![root_id, variant, post_id, md5], map_media_file)?;
        rows.next().transpose()
    }

    pub fn find_media_by_root_path(
        &self,
        root_id: &str,
        relative_path: &str,
    ) -> rusqlite::Result<Option<MediaFileRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT id, root_id, post_id, relative_path, variant, mime_type, byte_size,
                    sha256, md5, width, height, duration, status, created_at, updated_at
             FROM media_files
             WHERE root_id=?1 AND relative_path=?2 AND status='active'
             LIMIT 1",
        )?;
        let mut rows = statement.query_map(params![root_id, relative_path], map_media_file)?;
        rows.next().transpose()
    }

    pub fn create_task(
        &self,
        id: &str,
        kind: &str,
        payload: &serde_json::Value,
        status: &str,
    ) -> rusqlite::Result<TaskRecord> {
        let payload_json = serde_json::to_string(payload)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO tasks(id, kind, status, payload_json) VALUES (?1, ?2, ?3, ?4)",
            params![id, kind, status, payload_json],
        )?;
        query_task(&conn, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn get_task(&self, id: &str) -> rusqlite::Result<Option<TaskRecord>> {
        let conn = self.conn.lock().unwrap();
        query_task(&conn, id)
    }

    pub fn list_tasks(&self, limit: usize) -> rusqlite::Result<Vec<TaskRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT id, kind, status, payload_json, progress_json, result_json, error_json,
                    revision, items_total, items_completed, bytes_total, bytes_processed,
                    resumable, created_at, updated_at, started_at, finished_at
             FROM tasks ORDER BY updated_at DESC, id DESC LIMIT ?1",
        )?;
        let tasks = statement
            .query_map([limit.clamp(1, 1_000) as i64], map_task)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(tasks)
    }

    /// Deletes the task together with its task items. The schema's foreign-key
    /// cascade keeps this operation atomic for callers that have already
    /// completed filesystem cleanup.
    pub fn delete_task(&self, id: &str) -> rusqlite::Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        let deleted = transaction.execute("DELETE FROM tasks WHERE id=?1", [id])? == 1;
        if deleted {
            transaction.execute("DELETE FROM task_items WHERE task_id=?1", [id])?;
        }
        transaction.commit()?;
        Ok(deleted)
    }

    pub fn list_all_tasks(&self) -> rusqlite::Result<Vec<TaskRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT id, kind, status, payload_json, progress_json, result_json, error_json,
                    revision, items_total, items_completed, bytes_total, bytes_processed,
                    resumable, created_at, updated_at, started_at, finished_at
             FROM tasks ORDER BY updated_at DESC, id DESC",
        )?;
        let tasks = statement
            .query_map([], map_task)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(tasks)
    }

    pub fn get_terminal_download_task_cursor(
        &self,
        id: &str,
    ) -> rusqlite::Result<Option<DownloadTaskHistoryCursor>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT updated_at, id FROM tasks
             WHERE id=?1 AND kind='download'
               AND status IN ('completed','failed','cancelled')",
            [id],
            |row| {
                Ok(DownloadTaskHistoryCursor {
                    updated_at: row.get(0)?,
                    id: row.get(1)?,
                })
            },
        )
        .optional()
    }

    pub fn list_terminal_download_tasks(
        &self,
        cursor: Option<&DownloadTaskHistoryCursor>,
        limit: usize,
    ) -> rusqlite::Result<Vec<TaskRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT id, kind, status, payload_json, progress_json, result_json, error_json,
                    revision, items_total, items_completed, bytes_total, bytes_processed,
                    resumable, created_at, updated_at, started_at, finished_at
             FROM tasks
             WHERE kind='download'
               AND status IN ('completed','failed','cancelled')
               AND (?1 IS NULL OR updated_at < ?1 OR (updated_at = ?1 AND id < ?2))
             ORDER BY updated_at DESC, id DESC
             LIMIT ?3",
        )?;
        let records = statement
            .query_map(
                params![
                    cursor.map(|cursor| cursor.updated_at.as_str()),
                    cursor.map(|cursor| cursor.id.as_str()),
                    limit.clamp(1, 1_001) as i64,
                ],
                map_task,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    pub fn list_legacy_download_history(
        &self,
        before_id: Option<i64>,
        limit: usize,
    ) -> rusqlite::Result<Vec<DownloadRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT h.id, h.task_id, h.tags, h.exclude_tags, h.score_threshold,
                    h.limit_count, h.save_path, h.total_images, h.failed_count,
                    h.started_at, h.finished_at, h.status
             FROM download_history h
             WHERE h.status != 'running'
               AND (?1 IS NULL OR h.id < ?1)
               AND NOT EXISTS (
                   SELECT 1 FROM tasks t
                   WHERE t.id=h.task_id AND t.kind='download'
                     AND t.status IN ('completed','failed','cancelled')
               )
             ORDER BY h.id DESC
             LIMIT ?2",
        )?;
        let records = statement
            .query_map(
                params![before_id, limit.clamp(1, 1_001) as i64],
                map_download_record,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_task_snapshot(
        &self,
        id: &str,
        status: &str,
        revision: i64,
        progress: &serde_json::Value,
        result: Option<&serde_json::Value>,
        error: Option<&serde_json::Value>,
        items_total: i64,
        items_completed: i64,
        bytes_total: i64,
        bytes_processed: i64,
        resumable: bool,
    ) -> rusqlite::Result<TaskRecord> {
        let progress_json = json_to_sql(progress)?;
        let result_json = result.map(json_to_sql).transpose()?;
        let error_json = error.map(json_to_sql).transpose()?;
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE tasks SET status=?2, revision=?3, progress_json=?4, result_json=?5,
                    error_json=?6, items_total=?7, items_completed=?8, bytes_total=?9,
                    bytes_processed=?10, resumable=?11, updated_at=datetime('now'),
                    started_at=CASE WHEN ?2='running' THEN COALESCE(started_at, datetime('now')) ELSE started_at END,
                    finished_at=CASE WHEN ?2 IN ('completed','failed','cancelled') THEN datetime('now') ELSE NULL END
             WHERE id=?1",
            params![
                id,
                status,
                revision,
                progress_json,
                result_json,
                error_json,
                items_total,
                items_completed,
                bytes_total,
                bytes_processed,
                i64::from(resumable),
            ],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        query_task(&conn, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn recover_interrupted_tasks(&self) -> rusqlite::Result<usize> {
        let interrupted = serde_json::json!({
            "code": "interrupted",
            "message": "应用退出时任务仍在运行",
            "retryable": false
        });
        let interrupted_json = json_to_sql(&interrupted)?;
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        let resumable = transaction.execute(
            "UPDATE tasks SET status='paused', revision=revision+1, error_json=NULL,
                    updated_at=datetime('now'), resumable=1
             WHERE status='running' AND kind IN ('download', 'training', 'reindex_library')",
            [],
        )?;
        let other = transaction.execute(
            "UPDATE tasks SET status='failed', revision=revision+1, error_json=?1,
                    updated_at=datetime('now'), finished_at=datetime('now'), resumable=0
             WHERE status='running' AND kind NOT IN ('download', 'training', 'reindex_library')",
            [interrupted_json],
        )?;
        transaction.commit()?;
        Ok(resumable + other)
    }

    pub fn replace_task_items(
        &self,
        task_id: &str,
        items: &[TaskItemInput],
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        transaction.execute("DELETE FROM task_items WHERE task_id=?1", [task_id])?;
        {
            let mut statement = transaction.prepare(
                "INSERT INTO task_items(task_id, item_key, status, payload_json, result_json,
                                        error_json, attempts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for item in items {
                statement.execute(params![
                    task_id,
                    item.item_key,
                    item.status,
                    json_to_sql(&item.payload)?,
                    item.result.as_ref().map(json_to_sql).transpose()?,
                    item.error.as_ref().map(json_to_sql).transpose()?,
                    item.attempts,
                ])?;
            }
        }
        transaction.commit()
    }

    pub fn ensure_task_items(
        &self,
        task_id: &str,
        items: &[TaskItemInput],
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        {
            let mut statement = transaction.prepare(
                "INSERT OR IGNORE INTO task_items(
                    task_id, item_key, status, payload_json, result_json, error_json, attempts
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for item in items {
                statement.execute(params![
                    task_id,
                    item.item_key,
                    item.status,
                    json_to_sql(&item.payload)?,
                    item.result.as_ref().map(json_to_sql).transpose()?,
                    item.error.as_ref().map(json_to_sql).transpose()?,
                    item.attempts,
                ])?;
            }
        }
        transaction.commit()
    }

    pub fn finish_task_item(
        &self,
        task_id: &str,
        item_key: &str,
        status: &str,
        result: Option<&serde_json::Value>,
        error: Option<&serde_json::Value>,
    ) -> rusqlite::Result<bool> {
        if !matches!(status, "completed" | "skipped" | "failed") {
            return Err(rusqlite::Error::InvalidParameterName(
                "task item terminal status".to_string(),
            ));
        }
        let result_json = result.map(json_to_sql).transpose()?;
        let error_json = error.map(json_to_sql).transpose()?;
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE task_items
             SET status=?3, result_json=?4, error_json=?5, attempts=attempts+1,
                 updated_at=datetime('now')
             WHERE task_id=?1 AND item_key=?2 AND status='queued'",
            params![task_id, item_key, status, result_json, error_json],
        )?;
        Ok(changed == 1)
    }

    pub fn requeue_retryable_task_items(&self, task_id: &str) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE task_items
             SET status='queued', result_json=NULL, error_json=NULL,
                 updated_at=datetime('now')
             WHERE task_id=?1 AND status='failed'
               AND json_extract(error_json, '$.retryable')=1",
            [task_id],
        )
    }

    pub fn list_task_items(&self, task_id: &str) -> rusqlite::Result<Vec<TaskItemRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT id, task_id, item_key, status, payload_json, result_json, error_json,
                    attempts, updated_at
             FROM task_items WHERE task_id=?1 ORDER BY id",
        )?;
        let items = statement
            .query_map([task_id], map_task_item)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(items)
    }

    pub fn task_item_counts(&self, task_id: &str) -> rusqlite::Result<TaskItemCounts> {
        let conn = self.conn.lock().unwrap();
        let (total, queued, completed, skipped, failed, retryable_failed, completed_bytes) = conn
            .query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(CASE WHEN status='queued' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN status='completed' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN status='skipped' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN status='failed' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN status='failed'
                        AND json_extract(error_json, '$.retryable')=1 THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN status='completed'
                        THEN CAST(json_extract(result_json, '$.bytes') AS INTEGER)
                        ELSE 0 END), 0)
             FROM task_items WHERE task_id=?1",
            [task_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )?;
        Ok(TaskItemCounts {
            total: total.max(0) as u64,
            queued: queued.max(0) as u64,
            completed: completed.max(0) as u64,
            skipped: skipped.max(0) as u64,
            failed: failed.max(0) as u64,
            retryable_failed: retryable_failed.max(0) as u64,
            completed_bytes: completed_bytes.max(0) as u64,
        })
    }

    pub fn list_task_items_page(
        &self,
        task_id: &str,
        status: Option<&str>,
        after_id: Option<i64>,
        limit: usize,
    ) -> rusqlite::Result<TaskItemsPage> {
        let limit = limit.clamp(1, 100);
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT id, task_id, item_key, status, payload_json, result_json, error_json,
                    attempts, updated_at
             FROM task_items
             WHERE task_id=?1 AND (?2 IS NULL OR status=?2) AND id>?3
             ORDER BY id ASC LIMIT ?4",
        )?;
        let mut items = statement
            .query_map(
                params![task_id, status, after_id.unwrap_or(0), (limit + 1) as i64],
                map_task_item,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let has_more = items.len() > limit;
        items.truncate(limit);
        let next_cursor = has_more.then(|| items.last().expect("non-empty task item page").id);
        Ok(TaskItemsPage { items, next_cursor })
    }

    pub fn quarantine_media(&self, entry: &QuarantineInput) -> rusqlite::Result<QuarantineRecord> {
        self.quarantine_media_batch(std::slice::from_ref(entry))?
            .into_iter()
            .next()
            .ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn quarantine_media_batch(
        &self,
        entries: &[QuarantineInput],
    ) -> rusqlite::Result<Vec<QuarantineRecord>> {
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        for entry in entries {
            transaction.execute(
                "INSERT INTO quarantine(id, root_id, media_file_id, original_relative_path,
                                        quarantine_relative_path, reason, sha256)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    entry.id,
                    entry.root_id,
                    entry.media_file_id,
                    entry.original_relative_path,
                    entry.quarantine_relative_path,
                    entry.reason,
                    entry.sha256,
                ],
            )?;
            if let Some(media_file_id) = &entry.media_file_id {
                let changed = transaction.execute(
                    "UPDATE media_files
                     SET status='quarantined', updated_at=datetime('now')
                     WHERE id=?1 AND root_id=?2",
                    params![media_file_id, entry.root_id],
                )?;
                if changed != 1 {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
            }
        }
        let records = entries
            .iter()
            .map(|entry| {
                query_quarantine(&transaction, &entry.id)?
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)
            })
            .collect::<rusqlite::Result<Vec<_>>>()?;
        transaction.commit()?;
        Ok(records)
    }

    pub fn quarantine_and_replace_media(
        &self,
        entry: &QuarantineInput,
        replacement: &MediaFileInput,
    ) -> rusqlite::Result<(QuarantineRecord, MediaFileRecord)> {
        self.quarantine_and_replace_media_batch(&[(entry.clone(), replacement.clone())])?
            .into_iter()
            .next()
            .ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn quarantine_and_replace_media_batch(
        &self,
        replacements: &[(QuarantineInput, MediaFileInput)],
    ) -> rusqlite::Result<Vec<(QuarantineRecord, MediaFileRecord)>> {
        if replacements.is_empty() {
            return Ok(Vec::new());
        }
        if replacements.iter().any(|(entry, replacement)| {
            entry.media_file_id.is_some() || entry.root_id != replacement.root_id
        }) {
            return Err(rusqlite::Error::InvalidParameterName(
                "replacement quarantine must be unlinked and use the replacement root".to_string(),
            ));
        }
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        for (entry, replacement) in replacements {
            transaction.execute(
                "INSERT INTO quarantine(id, root_id, media_file_id, original_relative_path,
                                        quarantine_relative_path, reason, sha256)
                 VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6)",
                params![
                    entry.id,
                    entry.root_id,
                    entry.original_relative_path,
                    entry.quarantine_relative_path,
                    entry.reason,
                    entry.sha256,
                ],
            )?;
            transaction.execute(
                "INSERT INTO media_files(id, root_id, post_id, relative_path, variant, mime_type,
                                         byte_size, sha256, md5, width, height, duration)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(id) DO UPDATE SET
                    root_id=excluded.root_id, post_id=excluded.post_id,
                    relative_path=excluded.relative_path, variant=excluded.variant,
                    mime_type=excluded.mime_type, byte_size=excluded.byte_size,
                    sha256=excluded.sha256, md5=excluded.md5, width=excluded.width,
                    height=excluded.height, duration=excluded.duration, status='active',
                    updated_at=datetime('now')",
                params![
                    replacement.id,
                    replacement.root_id,
                    replacement.post_id,
                    replacement.relative_path,
                    replacement.variant,
                    replacement.mime_type,
                    replacement.byte_size,
                    replacement.sha256,
                    replacement.md5,
                    replacement.width,
                    replacement.height,
                    replacement.duration,
                ],
            )?;
        }
        let records = replacements
            .iter()
            .map(|(entry, replacement)| {
                let quarantine = query_quarantine(&transaction, &entry.id)?
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                let media = query_media_file(&transaction, &replacement.id)?
                    .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
                Ok((quarantine, media))
            })
            .collect::<rusqlite::Result<Vec<_>>>()?;
        transaction.commit()?;
        Ok(records)
    }

    pub fn list_quarantine(
        &self,
        root_id: &str,
        include_restored: bool,
    ) -> rusqlite::Result<Vec<QuarantineRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT id, root_id, media_file_id, original_relative_path,
                    quarantine_relative_path, reason, sha256, quarantined_at, restored_at
             FROM quarantine
             WHERE root_id=?1 AND (?2=1 OR restored_at IS NULL)
             ORDER BY quarantined_at DESC, id DESC",
        )?;
        let records = statement
            .query_map(
                params![root_id, i64::from(include_restored)],
                map_quarantine,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    pub fn get_quarantine(&self, id: &str) -> rusqlite::Result<Option<QuarantineRecord>> {
        let conn = self.conn.lock().unwrap();
        query_quarantine(&conn, id)
    }

    pub fn mark_quarantine_restored(&self, id: &str) -> rusqlite::Result<QuarantineRecord> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        let changed = transaction.execute(
            "UPDATE quarantine SET restored_at=datetime('now')
             WHERE id=?1 AND restored_at IS NULL",
            [id],
        )?;
        if changed == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        transaction.execute(
            "UPDATE media_files SET status='active', updated_at=datetime('now')
             WHERE id=(SELECT media_file_id FROM quarantine WHERE id=?1)",
            [id],
        )?;
        transaction.commit()?;
        query_quarantine(&conn, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn delete_quarantine_record(&self, id: &str) -> rusqlite::Result<bool> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.execute(
            "DELETE FROM quarantine WHERE id=?1 AND restored_at IS NOT NULL",
            [id],
        )? > 0)
    }

    pub fn purge_quarantine_record(&self, id: &str) -> rusqlite::Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        transaction.execute(
            "UPDATE media_files SET status='deleted', updated_at=datetime('now')
             WHERE id=(SELECT media_file_id FROM quarantine WHERE id=?1 AND restored_at IS NULL)",
            [id],
        )?;
        let changed = transaction.execute(
            "DELETE FROM quarantine WHERE id=?1 AND restored_at IS NULL",
            [id],
        )?;
        transaction.commit()?;
        Ok(changed > 0)
    }

    pub fn restore_purged_quarantine_record(
        &self,
        record: &QuarantineRecord,
    ) -> rusqlite::Result<()> {
        if record.restored_at.is_some() {
            return Err(rusqlite::Error::InvalidParameterName(
                "cannot restore an already restored quarantine record".to_string(),
            ));
        }
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn.transaction()?;
        transaction.execute(
            "INSERT INTO quarantine(id, root_id, media_file_id, original_relative_path,
                                    quarantine_relative_path, reason, sha256, quarantined_at,
                                    restored_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
            params![
                record.id,
                record.root_id,
                record.media_file_id,
                record.original_relative_path,
                record.quarantine_relative_path,
                record.reason,
                record.sha256,
                record.quarantined_at,
            ],
        )?;
        if let Some(media_file_id) = &record.media_file_id {
            let changed = transaction.execute(
                "UPDATE media_files SET status='quarantined', updated_at=datetime('now')
                 WHERE id=?1 AND root_id=?2",
                params![media_file_id, record.root_id],
            )?;
            if changed != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
        }
        transaction.commit()
    }
}

fn insert_local_post_with_tags_if_missing_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    post: &PostRecordInput,
    tags: &[PostTagInput],
) -> rusqlite::Result<()> {
    let inserted = transaction.execute(
        "INSERT INTO posts(
            id, md5, rating, score, fav_count, width, height, file_ext, file_size,
            source, duration, status, tag_string, tag_string_general,
            tag_string_character, tag_string_copyright, tag_string_artist, tag_string_meta,
            posted_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
            ?16, ?17, ?18, CAST(strftime('%s', ?19) AS INTEGER)
         )
         ON CONFLICT DO NOTHING",
        params![
            post.id,
            post.md5,
            post.rating,
            post.score,
            post.fav_count,
            post.width,
            post.height,
            post.file_ext,
            post.file_size,
            post.source,
            post.duration,
            post.status,
            post.tag_string,
            post.tag_string_general,
            post.tag_string_character,
            post.tag_string_copyright,
            post.tag_string_artist,
            post.tag_string_meta,
            post.post_created_at,
        ],
    )?;
    if inserted == 0 {
        return Ok(());
    }
    for tag in tags {
        let name = tag.name.trim();
        if name.is_empty() {
            continue;
        }
        transaction.execute(
            "INSERT INTO tags(name, category, post_count) VALUES (?1, ?2, COALESCE(?3, 0))
             ON CONFLICT(name) DO UPDATE SET
                category=excluded.category,
                post_count=COALESCE(?3, tags.post_count),
                updated_at=datetime('now')",
            params![name, tag.category, tag.post_count],
        )?;
        let tag_id: i64 =
            transaction.query_row("SELECT id FROM tags WHERE name=?1", [name], |row| {
                row.get(0)
            })?;
        transaction.execute(
            "INSERT OR IGNORE INTO post_tags(post_id, tag_id) VALUES (?1, ?2)",
            params![post.id, tag_id],
        )?;
    }
    Ok(())
}

fn upsert_post_with_tags_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    post: &PostRecordInput,
    tags: &[PostTagInput],
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO posts(
            id, md5, rating, score, fav_count, width, height, file_ext, file_size,
            source, duration, status, tag_string, tag_string_general,
            tag_string_character, tag_string_copyright, tag_string_artist, tag_string_meta,
            posted_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
            ?16, ?17, ?18, CAST(strftime('%s', ?19) AS INTEGER)
         )
         ON CONFLICT(id) DO UPDATE SET
            md5=excluded.md5, rating=excluded.rating, score=excluded.score,
            fav_count=excluded.fav_count, width=excluded.width, height=excluded.height,
            file_ext=excluded.file_ext, file_size=excluded.file_size,
            source=excluded.source, duration=excluded.duration, status=excluded.status,
            tag_string=excluded.tag_string,
            tag_string_general=excluded.tag_string_general,
            tag_string_character=excluded.tag_string_character,
            tag_string_copyright=excluded.tag_string_copyright,
            tag_string_artist=excluded.tag_string_artist,
            tag_string_meta=excluded.tag_string_meta,
            posted_at=COALESCE(excluded.posted_at, posts.posted_at),
            fetched_at=datetime('now')",
        params![
            post.id,
            post.md5,
            post.rating,
            post.score,
            post.fav_count,
            post.width,
            post.height,
            post.file_ext,
            post.file_size,
            post.source,
            post.duration,
            post.status,
            post.tag_string,
            post.tag_string_general,
            post.tag_string_character,
            post.tag_string_copyright,
            post.tag_string_artist,
            post.tag_string_meta,
            post.post_created_at,
        ],
    )?;
    transaction.execute("DELETE FROM post_tags WHERE post_id=?1", [post.id])?;
    for tag in tags {
        let name = tag.name.trim();
        if name.is_empty() {
            continue;
        }
        transaction.execute(
            "INSERT INTO tags(name, category, post_count) VALUES (?1, ?2, COALESCE(?3, 0))
             ON CONFLICT(name) DO UPDATE SET
                category=excluded.category,
                post_count=COALESCE(?3, tags.post_count),
                updated_at=datetime('now')",
            params![name, tag.category, tag.post_count],
        )?;
        let tag_id: i64 =
            transaction.query_row("SELECT id FROM tags WHERE name=?1", [name], |row| {
                row.get(0)
            })?;
        transaction.execute(
            "INSERT OR IGNORE INTO post_tags(post_id, tag_id) VALUES (?1, ?2)",
            params![post.id, tag_id],
        )?;
    }
    Ok(())
}

fn upsert_media_file_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    media: &MediaFileInput,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO media_files(id, root_id, post_id, relative_path, variant, mime_type,
                                 byte_size, sha256, md5, width, height, duration,
                                 catalog_seq, directory_key, short_edge, catalog_revision)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                 COALESCE((SELECT MAX(catalog_seq) + 1 FROM media_files WHERE root_id=?2), 1),
                 ?13,
                 CASE WHEN ?10 IS NULL OR ?11 IS NULL THEN NULL
                      WHEN ?10 < ?11 THEN ?10 ELSE ?11 END,
                 COALESCE((SELECT catalog_revision FROM roots WHERE id=?2), 1))
         ON CONFLICT(id) DO UPDATE SET
            root_id=excluded.root_id, post_id=excluded.post_id,
            relative_path=excluded.relative_path, variant=excluded.variant,
            mime_type=excluded.mime_type, byte_size=excluded.byte_size,
            sha256=excluded.sha256, md5=excluded.md5, width=excluded.width,
            height=excluded.height, duration=excluded.duration,
            directory_key=excluded.directory_key, short_edge=excluded.short_edge,
            status='active',
            updated_at=datetime('now')
         ON CONFLICT(root_id, relative_path) DO UPDATE SET
            post_id=COALESCE(excluded.post_id, media_files.post_id),
            variant=excluded.variant, mime_type=excluded.mime_type,
            byte_size=excluded.byte_size,
            sha256=COALESCE(excluded.sha256, media_files.sha256),
            md5=COALESCE(excluded.md5, media_files.md5),
            width=COALESCE(excluded.width, media_files.width),
            height=COALESCE(excluded.height, media_files.height),
            directory_key=excluded.directory_key,
            short_edge=COALESCE(excluded.short_edge, media_files.short_edge),
            duration=COALESCE(excluded.duration, media_files.duration),
            status='active', updated_at=datetime('now')",
        params![
            media.id,
            media.root_id,
            media.post_id,
            media.relative_path,
            media.variant,
            media.mime_type,
            media.byte_size,
            media.sha256,
            media.md5,
            media.width,
            media.height,
            media.duration,
            Path::new(&media.relative_path)
                .parent()
                .and_then(Path::to_str)
                .unwrap_or("")
                .replace('\\', "/"),
        ],
    )?;
    Ok(())
}

fn record_downloaded_post_location_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    root_id: &str,
    relative_directory: &str,
    post_id: i64,
    task_id: Option<&str>,
) -> rusqlite::Result<()> {
    transaction.execute(
        "INSERT INTO downloaded_post_locations(
            root_id, relative_directory, post_id, last_task_id
         ) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(root_id, relative_directory, post_id) DO UPDATE SET
            last_task_id=excluded.last_task_id,
            downloaded_at=datetime('now')",
        params![root_id, relative_directory, post_id, task_id],
    )?;
    Ok(())
}

fn backfill_downloaded_post_locations(
    transaction: &rusqlite::Transaction<'_>,
) -> rusqlite::Result<()> {
    let records = {
        let mut statement = transaction.prepare(
            "SELECT root_id, post_id, relative_path
             FROM media_files
             WHERE status='active' AND post_id IS NOT NULL",
        )?;
        let records = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        records
    };
    for (root_id, post_id, relative_path) in records {
        let normalized_path = relative_path.replace('\\', "/");
        let relative_directory = normalized_path
            .rsplit_once('/')
            .map_or("", |(directory, _)| directory);
        transaction.execute(
            "INSERT OR IGNORE INTO downloaded_post_locations(
                root_id, relative_directory, post_id
             ) VALUES (?1, ?2, ?3)",
            params![root_id, relative_directory, post_id],
        )?;
    }
    Ok(())
}

fn map_root(row: &rusqlite::Row<'_>) -> rusqlite::Result<RootRecord> {
    Ok(RootRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        windows_path: row.get(2)?,
        linux_path: row.get(3)?,
        indexing_status: row.get(4)?,
        catalog_revision: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn query_root(conn: &Connection, id: &str) -> rusqlite::Result<Option<RootRecord>> {
    let mut statement = conn.prepare(
        "SELECT id, name, windows_path, linux_path, indexing_status, catalog_revision, created_at, updated_at
         FROM roots WHERE id=?1",
    )?;
    let mut rows = statement.query_map([id], map_root)?;
    rows.next().transpose()
}

fn map_media_file(row: &rusqlite::Row<'_>) -> rusqlite::Result<MediaFileRecord> {
    Ok(MediaFileRecord {
        id: row.get(0)?,
        root_id: row.get(1)?,
        post_id: row.get(2)?,
        relative_path: row.get(3)?,
        variant: row.get(4)?,
        mime_type: row.get(5)?,
        byte_size: row.get(6)?,
        sha256: row.get(7)?,
        md5: row.get(8)?,
        width: row.get(9)?,
        height: row.get(10)?,
        duration: row.get(11)?,
        status: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn query_media_file(conn: &Connection, id: &str) -> rusqlite::Result<Option<MediaFileRecord>> {
    let mut statement = conn.prepare(
        "SELECT id, root_id, post_id, relative_path, variant, mime_type, byte_size,
                sha256, md5, width, height, duration, status, created_at, updated_at
         FROM media_files WHERE id=?1",
    )?;
    let mut rows = statement.query_map([id], map_media_file)?;
    rows.next().transpose()
}

fn query_media_file_by_root_path(
    conn: &Connection,
    root_id: &str,
    relative_path: &str,
) -> rusqlite::Result<Option<MediaFileRecord>> {
    conn.query_row(
        "SELECT id, root_id, post_id, relative_path, variant, mime_type, byte_size, sha256, md5,
                width, height, duration, status, created_at, updated_at
         FROM media_files WHERE root_id=?1 AND relative_path=?2",
        params![root_id, relative_path],
        map_media_file,
    )
    .optional()
}

fn query_task(conn: &Connection, id: &str) -> rusqlite::Result<Option<TaskRecord>> {
    let mut statement = conn.prepare(
        "SELECT id, kind, status, payload_json, progress_json, result_json, error_json,
                revision, items_total, items_completed, bytes_total, bytes_processed,
                resumable, created_at, updated_at, started_at, finished_at
         FROM tasks WHERE id=?1",
    )?;
    let mut rows = statement.query_map([id], map_task)?;
    rows.next().transpose()
}

fn map_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRecord> {
    let payload: String = row.get(3)?;
    let progress: String = row.get(4)?;
    let result: Option<String> = row.get(5)?;
    let error: Option<String> = row.get(6)?;
    Ok(TaskRecord {
        id: row.get(0)?,
        kind: row.get(1)?,
        status: row.get(2)?,
        payload: parse_json_column(payload, 3)?,
        progress: parse_json_column(progress, 4)?,
        result: result
            .map(|value| parse_json_column(value, 5))
            .transpose()?,
        error: error.map(|value| parse_json_column(value, 6)).transpose()?,
        revision: row.get(7)?,
        items_total: row.get(8)?,
        items_completed: row.get(9)?,
        bytes_total: row.get(10)?,
        bytes_processed: row.get(11)?,
        resumable: row.get::<_, i64>(12)? != 0,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
        started_at: row.get(15)?,
        finished_at: row.get(16)?,
    })
}

fn map_download_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<DownloadRecord> {
    Ok(DownloadRecord {
        id: row.get(0)?,
        task_id: row.get(1)?,
        tags: row.get(2)?,
        exclude_tags: row.get(3)?,
        score_threshold: row.get(4)?,
        limit_count: row.get(5)?,
        save_path: row.get(6)?,
        total_images: row.get(7)?,
        failed_count: row.get(8)?,
        started_at: row.get(9)?,
        finished_at: row.get(10)?,
        status: row.get(11)?,
    })
}

fn parse_json_column(value: String, column: usize) -> rusqlite::Result<serde_json::Value> {
    serde_json::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn json_to_sql(value: &serde_json::Value) -> rusqlite::Result<String> {
    serde_json::to_string(value)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))
}

fn map_task_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskItemRecord> {
    let payload: String = row.get(4)?;
    let result: Option<String> = row.get(5)?;
    let error: Option<String> = row.get(6)?;
    Ok(TaskItemRecord {
        id: row.get(0)?,
        task_id: row.get(1)?,
        item_key: row.get(2)?,
        status: row.get(3)?,
        payload: parse_json_column(payload, 4)?,
        result: result
            .map(|value| parse_json_column(value, 5))
            .transpose()?,
        error: error.map(|value| parse_json_column(value, 6)).transpose()?,
        attempts: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn map_quarantine(row: &rusqlite::Row<'_>) -> rusqlite::Result<QuarantineRecord> {
    Ok(QuarantineRecord {
        id: row.get(0)?,
        root_id: row.get(1)?,
        media_file_id: row.get(2)?,
        original_relative_path: row.get(3)?,
        quarantine_relative_path: row.get(4)?,
        reason: row.get(5)?,
        sha256: row.get(6)?,
        quarantined_at: row.get(7)?,
        restored_at: row.get(8)?,
    })
}

fn query_quarantine(conn: &Connection, id: &str) -> rusqlite::Result<Option<QuarantineRecord>> {
    let mut statement = conn.prepare(
        "SELECT id, root_id, media_file_id, original_relative_path, quarantine_relative_path,
                reason, sha256, quarantined_at, restored_at
         FROM quarantine WHERE id=?1",
    )?;
    let mut rows = statement.query_map([id], map_quarantine)?;
    rows.next().transpose()
}

#[cfg(test)]
mod migration_tests {
    use super::{
        Database, LibraryMediaFilters, LibraryScoreRange, MediaFileInput, PostRecordInput,
        PostTagInput, QuarantineInput, TaskItemInput,
    };
    use crate::models::DownloadConfig;

    #[test]
    fn reindex_staging_keeps_old_catalog_until_atomic_count_checked_cutover() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-reindex-cutover-{}-{}.db",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library", None, Some("/media")).unwrap();
        let media = |id: &str, relative_path: &str| MediaFileInput {
            id: id.to_string(),
            root_id: "root-1".to_string(),
            post_id: None,
            relative_path: relative_path.to_string(),
            variant: "original".to_string(),
            mime_type: "image/jpeg".to_string(),
            byte_size: 42,
            sha256: None,
            md5: None,
            width: Some(100),
            height: Some(80),
            duration: None,
        };
        db.upsert_media_file(&media("old", "old.jpg")).unwrap();
        db.begin_library_reindex_staging("task-1", "root-1").unwrap();
        db.stage_library_reindex_batch(
            "task-1",
            "root-1",
            &[],
            &[media("new-a", "new-a.jpg"), media("new-b", "new-b.jpg")],
            &[("new-a".to_string(), 1), ("new-b".to_string(), 2)],
        ).unwrap();
        db.begin_library_reindex_staging("task-1", "root-1").unwrap();
        db.begin_library_reindex_staging("task-2", "root-1").unwrap();

        assert_eq!(
            db.list_library_media("root-1", None, 60, "").unwrap().items[0].id,
            "old"
        );
        assert!(db.commit_library_reindex("task-1", "root-1", 3).is_err());
        assert_eq!(
            db.list_library_media("root-1", None, 60, "").unwrap().items[0].id,
            "old"
        );

        let (revision, removed) = db.commit_library_reindex("task-1", "root-1", 2).unwrap();
        let page = db.list_library_media("root-1", None, 60, "").unwrap();
        assert_eq!(revision, 2);
        assert_eq!(removed, 1);
        assert_eq!(page.catalog_revision, 2);
        assert_eq!(
            page.items.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
            ["new-a", "new-b"]
        );
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn five_hundred_thousand_item_cursor_page_stays_bounded_at_depth() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-library-500k-{}-{}.db",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let db = Database::open(&path).unwrap();
        db.create_root("root-500k", "Scale fixture", None, Some("/media")).unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute_batch(
                "WITH RECURSIVE sequence(value) AS (
                    SELECT 1 UNION ALL SELECT value + 1 FROM sequence WHERE value < 500000
                 )
                 INSERT INTO media_files(
                    id, root_id, relative_path, variant, mime_type, byte_size,
                    width, height, catalog_seq, directory_key, short_edge,
                    file_mtime, catalog_revision, status)
                 SELECT printf('media-%06d', value), 'root-500k',
                        printf('folder-%03d/media-%06d.jpg', value / 1000, value),
                        'original', 'image/jpeg', 4096, 1024, 768, value,
                        printf('folder-%03d', value / 1000), 768,
                        1700000000 + value, 1, 'active'
                 FROM sequence;",
            ).unwrap();
        }

        let query_first = || db.list_library_media("root-500k", None, 60, "").unwrap();
        let query_deep = || db.list_library_media("root-500k", Some("media-450000"), 60, "").unwrap();
        let first = query_first();
        let deep = query_deep();
        let percentile_95 = |mut samples: Vec<std::time::Duration>| {
            samples.sort_unstable();
            samples[((samples.len() * 95).div_ceil(100)).saturating_sub(1)]
        };
        let mut first_samples = Vec::new();
        let mut deep_samples = Vec::new();
        for _ in 0..20 {
            let started = std::time::Instant::now();
            let page = query_first();
            first_samples.push(started.elapsed());
            assert_eq!(page.items.len(), 60);
            let started = std::time::Instant::now();
            let page = query_deep();
            deep_samples.push(started.elapsed());
            assert_eq!(page.items.len(), 60);
        }
        let first_p95 = percentile_95(first_samples);
        let deep_p95 = percentile_95(deep_samples);

        assert_eq!(first.total, 500_000);
        assert_eq!(first.items.len(), 60);
        assert_eq!(deep.items.first().unwrap().id, "media-450001");
        assert_eq!(deep.items.len(), 60);
        assert!(first_p95 < std::time::Duration::from_millis(300), "first-page p95 was {first_p95:?}");
        assert!(deep_p95 < std::time::Duration::from_millis(300), "deep-page p95 was {deep_p95:?}");
        let larger = first_p95.max(deep_p95);
        let smaller = first_p95.min(deep_p95);
        assert!(
            larger <= smaller.mul_f64(1.2) + std::time::Duration::from_millis(5),
            "cursor depth changed p95 too much: first={first_p95:?}, deep={deep_p95:?}"
        );

        {
            let conn = db.conn.lock().unwrap();
            conn.execute_batch(
                "WITH RECURSIVE selected(value) AS (
                    SELECT 10 UNION ALL SELECT value + 10 FROM selected WHERE value < 500000
                 )
                 INSERT INTO posts(id, rating, score, width, height)
                 SELECT value, 'g', 50, 1024, 768 FROM selected;
                 INSERT INTO tags(id, name, category) VALUES (1, 'cat', 0), (2, 'blue_hair', 0);
                 INSERT INTO post_tags(post_id, tag_id) SELECT id, 1 FROM posts;
                 INSERT INTO post_tags(post_id, tag_id) SELECT id, 2 FROM posts;
                 UPDATE media_files SET post_id=catalog_seq WHERE catalog_seq % 10 = 0;",
            ).unwrap();
        }
        let filters = LibraryMediaFilters {
            score_min: Some(40),
            score_max: Some(60),
            min_resolution: Some(700),
            max_resolution: Some(800),
            post_created_from: None,
            post_created_to: None,
            relative_directory: None,
        };
        let mut filtered_samples = Vec::new();
        for _ in 0..10 {
            let started = std::time::Instant::now();
            let page = db.list_library_media_bidirectional(
                "root-500k", None, false, 60, "cat blue_hair", &filters,
            ).unwrap();
            filtered_samples.push(started.elapsed());
            assert_eq!(page.total, 50_000);
            assert_eq!(page.items.len(), 60);
        }
        let filtered_p95 = percentile_95(filtered_samples);
        assert!(filtered_p95 < std::time::Duration::from_millis(500), "filtered page p95 was {filtered_p95:?}");
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn migration_adds_normalized_schema_without_losing_legacy_history() {
        let path = std::env::temp_dir().join(format!("danbooru-schema-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.start_download("legacy-task", &DownloadConfig::default())
            .unwrap();

        let conn = db.conn.lock().unwrap();
        for table in [
            "roots",
            "posts",
            "media_files",
            "tags",
            "post_tags",
            "tasks",
            "task_items",
            "quarantine",
        ] {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists, "missing migrated table {table}");
        }
        drop(conn);
        assert_eq!(db.get_download_history(10).unwrap().len(), 1);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn versioned_migration_upgrades_a_partial_v1_tasks_table_transactionally() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("partial-v1.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE tasks (
                    id TEXT PRIMARY KEY,
                    kind TEXT NOT NULL,
                    status TEXT NOT NULL,
                    payload_json TEXT NOT NULL DEFAULT '{}'
                );
                CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at TEXT NOT NULL DEFAULT (datetime('now'))
                );
                INSERT INTO schema_migrations(version) VALUES (1);
                PRAGMA user_version=1;",
            )
            .unwrap();
        drop(connection);

        let database = Database::open(&path).unwrap();
        let created = database
            .create_task(
                "migrated-task",
                "download",
                &serde_json::json!({}),
                "queued",
            )
            .unwrap();
        let connection = database.conn.lock().unwrap();
        let user_version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        let version_recorded: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version=2)",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(created.id, "migrated-task");
        assert_eq!(user_version, 4);
        assert!(version_recorded);
    }

    #[test]
    fn versioned_migration_upgrades_partial_v1_media_columns_and_indexes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("partial-media-v1.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE roots (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    windows_path TEXT,
                    linux_path TEXT,
                    indexing_status TEXT NOT NULL DEFAULT 'not_indexed',
                    created_at TEXT NOT NULL DEFAULT '',
                    updated_at TEXT NOT NULL DEFAULT ''
                );
                CREATE TABLE media_files (
                    id TEXT PRIMARY KEY,
                    root_id TEXT NOT NULL,
                    relative_path TEXT NOT NULL,
                    mime_type TEXT NOT NULL
                );
                PRAGMA user_version=1;",
            )
            .unwrap();
        drop(connection);

        let database = Database::open(&path).unwrap();
        database
            .create_root("root-v1", "Migrated", Some(r"C:\Media"), Some("/media"))
            .unwrap();
        database
            .upsert_media_file(&MediaFileInput {
                id: "media-v1".to_string(),
                root_id: "root-v1".to_string(),
                post_id: None,
                relative_path: "image.jpg".to_string(),
                variant: "original".to_string(),
                mime_type: "image/jpeg".to_string(),
                byte_size: 4,
                sha256: None,
                md5: None,
                width: Some(1),
                height: Some(1),
                duration: None,
            })
            .unwrap();

        assert_eq!(
            database
                .get_media_file("media-v1")
                .unwrap()
                .unwrap()
                .byte_size,
            4
        );
    }

    #[test]
    fn versioned_migration_upgrades_partial_task_items_and_quarantine_tables() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("partial-items-v1.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE tasks (
                    id TEXT PRIMARY KEY, kind TEXT NOT NULL, status TEXT NOT NULL,
                    payload_json TEXT NOT NULL DEFAULT '{}'
                );
                CREATE TABLE task_items (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    task_id TEXT NOT NULL,
                    item_key TEXT NOT NULL
                );
                CREATE TABLE roots (
                    id TEXT PRIMARY KEY, name TEXT NOT NULL,
                    windows_path TEXT, linux_path TEXT,
                    indexing_status TEXT NOT NULL DEFAULT 'not_indexed',
                    created_at TEXT NOT NULL DEFAULT '', updated_at TEXT NOT NULL DEFAULT ''
                );
                CREATE TABLE media_files (
                    id TEXT PRIMARY KEY, root_id TEXT NOT NULL,
                    relative_path TEXT NOT NULL, mime_type TEXT NOT NULL
                );
                CREATE TABLE quarantine (
                    id TEXT PRIMARY KEY, root_id TEXT NOT NULL,
                    original_relative_path TEXT NOT NULL,
                    quarantine_relative_path TEXT NOT NULL,
                    reason TEXT NOT NULL
                );
                PRAGMA user_version=1;",
            )
            .unwrap();
        drop(connection);

        let database = Database::open(&path).unwrap();
        database
            .create_task("task-v1", "download", &serde_json::json!({}), "queued")
            .unwrap();
        database
            .ensure_task_items(
                "task-v1",
                &[TaskItemInput::new(
                    "post:1",
                    serde_json::json!({"post_id": 1}),
                )],
            )
            .unwrap();

        assert_eq!(database.task_item_counts("task-v1").unwrap().queued, 1);
    }

    #[test]
    fn versioned_migration_upgrades_partial_posts_and_tag_tables() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("partial-posts-v1.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE posts (id INTEGER PRIMARY KEY, rating TEXT NOT NULL DEFAULT 'g');
                CREATE TABLE tags (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL);
                CREATE TABLE post_tags (post_id INTEGER NOT NULL, tag_id INTEGER NOT NULL);
                PRAGMA user_version=1;",
            )
            .unwrap();
        drop(connection);

        let database = Database::open(&path).unwrap();
        database
            .upsert_post_with_tags(
                &PostRecordInput {
                    id: 42,
                    md5: Some("abcd".to_string()),
                    rating: "g".to_string(),
                    score: 1,
                    fav_count: 2,
                    width: 3,
                    height: 4,
                    file_ext: Some("jpg".to_string()),
                    file_size: Some(5),
                    source: None,
                    duration: None,
                    post_created_at: None,
                    status: "available".to_string(),
                    tag_string: "cat".to_string(),
                    tag_string_general: "cat".to_string(),
                    tag_string_character: String::new(),
                    tag_string_copyright: String::new(),
                    tag_string_artist: String::new(),
                    tag_string_meta: String::new(),
                },
                &[PostTagInput::new("cat", 0)],
            )
            .unwrap();

        assert_eq!(
            database
                .get_post_library_metadata(42)
                .unwrap()
                .unwrap()
                .tags[0]
                .name,
            "cat"
        );
    }

    #[test]
    fn versioned_migration_adds_posted_at_to_existing_posts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("partial-post-time-v3.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE posts (id INTEGER PRIMARY KEY, rating TEXT NOT NULL DEFAULT 'g');
                 CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at TEXT NOT NULL DEFAULT (datetime('now'))
                 );
                 INSERT INTO schema_migrations(version) VALUES (1), (2), (3);
                 PRAGMA user_version=3;",
            )
            .unwrap();
        drop(connection);

        let _database = Database::open(&path).unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        let has_posted_at: bool = connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM pragma_table_info('posts') WHERE name='posted_at'
                 )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let user_version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();

        assert!(has_posted_at);
        assert_eq!(user_version, 4);
    }

    #[test]
    fn versioned_migration_upgrades_partial_roots_without_registering_media() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("partial-roots-v1.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE roots (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    windows_path TEXT
                );
                INSERT INTO roots(id, name, windows_path)
                    VALUES ('root-v1', 'Legacy root', 'C:\\Media');
                PRAGMA user_version=1;",
            )
            .unwrap();
        drop(connection);

        let database = Database::open(&path).unwrap();
        let roots = database.list_roots().unwrap();

        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id, "root-v1");
        assert_eq!(roots[0].indexing_status, "not_indexed");
        assert_eq!(database.count_media_files("root-v1").unwrap(), 0);
    }

    #[test]
    fn failed_versioned_migration_rolls_back_schema_and_version() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("conflicting-v1.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE tasks (
                    id TEXT PRIMARY KEY, kind TEXT NOT NULL, status TEXT NOT NULL,
                    payload_json TEXT NOT NULL DEFAULT '{}'
                );
                CREATE TABLE task_items (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    task_id TEXT NOT NULL,
                    item_key TEXT NOT NULL
                );
                INSERT INTO task_items(task_id, item_key) VALUES ('task', 'same');
                INSERT INTO task_items(task_id, item_key) VALUES ('task', 'same');
                PRAGMA user_version=1;",
            )
            .unwrap();
        drop(connection);

        assert!(Database::open(&path).is_err());
        let connection = rusqlite::Connection::open(&path).unwrap();
        let user_version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        let has_status: bool = connection
            .prepare("PRAGMA table_info(task_items)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .iter()
            .any(|column| column == "status");

        assert_eq!(user_version, 1);
        assert!(!has_status);
    }

    #[test]
    fn roots_are_created_and_listed_without_indexing_media() {
        let path = std::env::temp_dir().join(format!("danbooru-roots-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();

        db.create_root(
            "root-1",
            "Training set",
            Some(r"C:\Media\Danbooru"),
            Some("/mnt/c/Media/Danbooru"),
        )
        .unwrap();

        let roots = db.list_roots().unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id, "root-1");
        assert_eq!(roots[0].indexing_status, "not_indexed");
        assert_eq!(db.list_media_files("root-1", None, 60).unwrap().len(), 0);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn task_snapshots_are_persisted_and_loaded() {
        let path = std::env::temp_dir().join(format!("danbooru-tasks-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();

        db.create_task(
            "task-1",
            "download",
            &serde_json::json!({"query": "cat"}),
            "queued",
        )
        .unwrap();

        let loaded = db.get_task("task-1").unwrap().unwrap();
        assert_eq!(loaded.payload["query"], "cat");
        assert_eq!(loaded.revision, 1);
        assert_eq!(db.list_tasks(20).unwrap().len(), 1);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn startup_recovery_pauses_downloads_and_fails_other_running_tasks() {
        let path =
            std::env::temp_dir().join(format!("danbooru-recovery-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        for (id, kind) in [("download-1", "download"), ("index-1", "index")] {
            db.create_task(id, kind, &serde_json::json!({}), "running")
                .unwrap();
            db.update_task_snapshot(
                id,
                "running",
                2,
                &serde_json::json!({"progress": 0.25}),
                None,
                None,
                10,
                2,
                1_000,
                250,
                kind == "download",
            )
            .unwrap();
        }

        assert_eq!(db.recover_interrupted_tasks().unwrap(), 2);

        assert_eq!(db.get_task("download-1").unwrap().unwrap().status, "paused");
        let index = db.get_task("index-1").unwrap().unwrap();
        assert_eq!(index.status, "failed");
        assert_eq!(index.error.unwrap()["code"], "interrupted");
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn task_items_are_replaced_transactionally() {
        let path =
            std::env::temp_dir().join(format!("danbooru-task-items-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_task("task-1", "download", &serde_json::json!({}), "queued")
            .unwrap();
        let items = vec![
            TaskItemInput::new("post:1", serde_json::json!({"post_id": 1})),
            TaskItemInput::new("post:2", serde_json::json!({"post_id": 2})),
        ];

        db.replace_task_items("task-1", &items).unwrap();

        let loaded = db.list_task_items("task-1").unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[1].payload["post_id"], 2);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn task_items_are_ensured_without_replacing_existing_results() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-task-items-ensure-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_task("task-1", "download", &serde_json::json!({}), "queued")
            .unwrap();
        let first = TaskItemInput {
            item_key: "post:1".to_string(),
            status: "completed".to_string(),
            payload: serde_json::json!({"post_id": 1}),
            result: Some(serde_json::json!({"bytes": 42})),
            error: None,
            attempts: 1,
        };
        db.replace_task_items("task-1", std::slice::from_ref(&first))
            .unwrap();

        db.ensure_task_items(
            "task-1",
            &[
                first,
                TaskItemInput::new("post:2", serde_json::json!({"post_id": 2})),
            ],
        )
        .unwrap();

        let loaded = db.list_task_items("task-1").unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].status, "completed");
        assert_eq!(loaded[0].result.as_ref().unwrap()["bytes"], 42);
        assert_eq!(loaded[1].status, "queued");
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn queued_task_item_can_be_finished_with_a_structured_result() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-task-item-finish-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_task("task-1", "download", &serde_json::json!({}), "queued")
            .unwrap();
        db.ensure_task_items(
            "task-1",
            &[TaskItemInput::new(
                "post:7",
                serde_json::json!({"post_id": 7}),
            )],
        )
        .unwrap();

        assert!(db
            .finish_task_item(
                "task-1",
                "post:7",
                "completed",
                Some(&serde_json::json!({"bytes": 99})),
                None,
            )
            .unwrap());

        let loaded = db.list_task_items("task-1").unwrap();
        assert_eq!(loaded[0].status, "completed");
        assert_eq!(loaded[0].result.as_ref().unwrap()["bytes"], 99);
        assert!(loaded[0].error.is_none());
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn downloaded_media_and_task_item_finish_share_one_transaction() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-download-item-atomic-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library", None, Some("/media"))
            .unwrap();
        db.create_task("task-1", "download", &serde_json::json!({}), "running")
            .unwrap();
        db.ensure_task_items(
            "task-1",
            &[TaskItemInput::new(
                "post:7",
                serde_json::json!({"post_id": 7}),
            )],
        )
        .unwrap();
        let post = PostRecordInput {
            id: 7,
            md5: Some("0123456789abcdef0123456789abcdef".to_string()),
            rating: "g".to_string(),
            score: 10,
            fav_count: 2,
            width: 100,
            height: 80,
            file_ext: Some("jpg".to_string()),
            file_size: Some(99),
            source: None,
            duration: None,
            post_created_at: None,
            status: "available".to_string(),
            tag_string: "cat".to_string(),
            tag_string_general: "cat".to_string(),
            tag_string_character: String::new(),
            tag_string_copyright: String::new(),
            tag_string_artist: String::new(),
            tag_string_meta: String::new(),
        };
        let media = MediaFileInput {
            id: "media-7".to_string(),
            root_id: "root-1".to_string(),
            post_id: Some(7),
            relative_path: "7.jpg".to_string(),
            variant: "original".to_string(),
            mime_type: "image/jpeg".to_string(),
            byte_size: 99,
            sha256: None,
            md5: post.md5.clone(),
            width: Some(100),
            height: Some(80),
            duration: None,
        };

        db.register_downloaded_post_and_finish_task_item(
            "task-1",
            "post:7",
            "completed",
            &serde_json::json!({"bytes": 99}),
            &post,
            &[PostTagInput::new("cat", 0)],
            std::slice::from_ref(&media),
            "root-1",
            "",
        )
        .unwrap();

        assert_eq!(db.get_media_file("media-7").unwrap().unwrap().byte_size, 99);
        assert!(db
            .was_post_downloaded_in_directory("root-1", "", 7)
            .unwrap());
        let item = db.list_task_items("task-1").unwrap().remove(0);
        assert_eq!(item.status, "completed");
        assert_eq!(item.result.unwrap()["bytes"], 99);

        let missing_item_media = MediaFileInput {
            id: "media-8".to_string(),
            relative_path: "8.jpg".to_string(),
            post_id: Some(8),
            ..media
        };
        assert!(db
            .register_downloaded_post_and_finish_task_item(
                "task-1",
                "post:8",
                "completed",
                &serde_json::json!({"bytes": 100}),
                &PostRecordInput { id: 8, ..post },
                &[],
                std::slice::from_ref(&missing_item_media),
                "root-1",
                "",
            )
            .is_err());
        assert!(db.get_media_file("media-8").unwrap().is_none());
        assert!(db.get_post_library_metadata(8).unwrap().is_none());
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn task_item_counts_include_terminal_states_and_completed_bytes() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-task-item-counts-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_task("task-1", "download", &serde_json::json!({}), "queued")
            .unwrap();
        db.ensure_task_items(
            "task-1",
            &[
                TaskItemInput::new("post:1", serde_json::json!({"post_id": 1})),
                TaskItemInput::new("post:2", serde_json::json!({"post_id": 2})),
                TaskItemInput::new("post:3", serde_json::json!({"post_id": 3})),
                TaskItemInput::new("post:4", serde_json::json!({"post_id": 4})),
            ],
        )
        .unwrap();
        db.finish_task_item(
            "task-1",
            "post:1",
            "completed",
            Some(&serde_json::json!({"bytes": 40})),
            None,
        )
        .unwrap();
        db.finish_task_item(
            "task-1",
            "post:2",
            "skipped",
            Some(&serde_json::json!({"reason": "already_exists"})),
            None,
        )
        .unwrap();
        db.finish_task_item(
            "task-1",
            "post:3",
            "failed",
            None,
            Some(&serde_json::json!({
                "code": "download_failed",
                "message": "failed",
                "retryable": true
            })),
        )
        .unwrap();

        let counts = db.task_item_counts("task-1").unwrap();
        assert_eq!(counts.total, 4);
        assert_eq!(counts.queued, 1);
        assert_eq!(counts.completed, 1);
        assert_eq!(counts.skipped, 1);
        assert_eq!(counts.failed, 1);
        assert_eq!(counts.completed_bytes, 40);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn task_item_counts_track_only_retryable_failures() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-task-item-retryable-counts-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_task("task-1", "download", &serde_json::json!({}), "queued")
            .unwrap();
        db.ensure_task_items(
            "task-1",
            &[
                TaskItemInput::new("post:1", serde_json::json!({"post_id": 1})),
                TaskItemInput::new("post:2", serde_json::json!({"post_id": 2})),
            ],
        )
        .unwrap();
        for (item_key, retryable) in [("post:1", true), ("post:2", false)] {
            db.finish_task_item(
                "task-1",
                item_key,
                "failed",
                None,
                Some(&serde_json::json!({
                    "code": "download_failed",
                    "message": "failed",
                    "retryable": retryable
                })),
            )
            .unwrap();
        }

        assert_eq!(db.task_item_counts("task-1").unwrap().retryable_failed, 1);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn only_retryable_failed_task_items_are_requeued() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-task-item-requeue-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_task("task-1", "download", &serde_json::json!({}), "failed")
            .unwrap();
        db.ensure_task_items(
            "task-1",
            &[
                TaskItemInput::new("post:1", serde_json::json!({"post_id": 1})),
                TaskItemInput::new("post:2", serde_json::json!({"post_id": 2})),
            ],
        )
        .unwrap();
        for (item_key, retryable) in [("post:1", true), ("post:2", false)] {
            db.finish_task_item(
                "task-1",
                item_key,
                "failed",
                None,
                Some(&serde_json::json!({
                    "code": "download_failed",
                    "message": "failed",
                    "retryable": retryable
                })),
            )
            .unwrap();
        }

        assert_eq!(db.requeue_retryable_task_items("task-1").unwrap(), 1);
        let items = db.list_task_items("task-1").unwrap();
        assert_eq!(items[0].status, "queued");
        assert!(items[0].error.is_none());
        assert_eq!(items[0].attempts, 1);
        assert_eq!(items[1].status, "failed");
        assert!(items[1].error.is_some());
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn task_items_use_stable_keyset_pagination() {
        let path =
            std::env::temp_dir().join(format!("danbooru-task-item-page-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_task("task-1", "download", &serde_json::json!({}), "queued")
            .unwrap();
        let items = (1..=4)
            .map(|post_id| {
                TaskItemInput::new(
                    format!("post:{post_id}"),
                    serde_json::json!({"post_id": post_id}),
                )
            })
            .collect::<Vec<_>>();
        db.ensure_task_items("task-1", &items).unwrap();

        let first = db.list_task_items_page("task-1", None, None, 2).unwrap();
        assert_eq!(
            first
                .items
                .iter()
                .map(|item| item.item_key.as_str())
                .collect::<Vec<_>>(),
            ["post:1", "post:2"]
        );
        let second = db
            .list_task_items_page("task-1", None, first.next_cursor, 2)
            .unwrap();
        assert_eq!(
            second
                .items
                .iter()
                .map(|item| item.item_key.as_str())
                .collect::<Vec<_>>(),
            ["post:3", "post:4"]
        );
        assert!(second.next_cursor.is_none());
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn media_is_resolved_by_opaque_id_or_hash() {
        let path = std::env::temp_dir().join(format!("danbooru-media-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library", None, Some("/media"))
            .unwrap();
        db.upsert_media_file(&MediaFileInput {
            id: "media-1".into(),
            root_id: "root-1".into(),
            post_id: None,
            relative_path: "images/1.jpg".into(),
            variant: "original".into(),
            mime_type: "image/jpeg".into(),
            byte_size: 42,
            sha256: Some("sha".into()),
            md5: Some("md5".into()),
            width: Some(100),
            height: Some(80),
            duration: None,
        })
        .unwrap();

        assert_eq!(
            db.get_media_file("media-1").unwrap().unwrap().relative_path,
            "images/1.jpg"
        );
        assert_eq!(
            db.find_media_by_post_or_md5(None, Some("md5"))
                .unwrap()
                .unwrap()
                .id,
            "media-1"
        );
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn indexing_upsert_reuses_an_existing_record_for_the_same_root_path() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-index-path-upsert-{}-{}.db",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library", None, Some("/media"))
            .unwrap();
        db.upsert_media_file(&MediaFileInput {
            id: "downloaded-42".into(),
            root_id: "root-1".into(),
            post_id: None,
            relative_path: "42_score_9.jpg".into(),
            variant: "original".into(),
            mime_type: "image/jpeg".into(),
            byte_size: 40,
            sha256: Some("trusted-sha".into()),
            md5: Some("trusted-md5".into()),
            width: Some(100),
            height: Some(80),
            duration: None,
        })
        .unwrap();

        let indexed = db
            .upsert_media_file(&MediaFileInput {
                id: "indexed-new-hash".into(),
                root_id: "root-1".into(),
                post_id: None,
                relative_path: "42_score_9.jpg".into(),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 42,
                sha256: None,
                md5: None,
                width: Some(120),
                height: Some(90),
                duration: None,
            })
            .unwrap();

        assert_eq!(indexed.id, "downloaded-42");
        assert_eq!(indexed.md5.as_deref(), Some("trusted-md5"));
        assert_eq!(indexed.sha256.as_deref(), Some("trusted-sha"));
        assert_eq!(indexed.byte_size, 42);
        assert_eq!(db.count_media_files("root-1").unwrap(), 1);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn quarantine_records_can_be_listed_and_restored() {
        let path =
            std::env::temp_dir().join(format!("danbooru-quarantine-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library", None, Some("/media"))
            .unwrap();
        db.quarantine_media(&QuarantineInput {
            id: "q-1".into(),
            root_id: "root-1".into(),
            media_file_id: None,
            original_relative_path: "1.jpg".into(),
            quarantine_relative_path: ".danbooru-quarantine/q-1/1.jpg".into(),
            reason: "duplicate".into(),
            sha256: Some("sha".into()),
        })
        .unwrap();

        assert_eq!(db.list_quarantine("root-1", false).unwrap().len(), 1);
        db.mark_quarantine_restored("q-1").unwrap();
        assert!(db.list_quarantine("root-1", false).unwrap().is_empty());
        assert_eq!(db.list_quarantine("root-1", true).unwrap().len(), 1);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn quarantine_batch_constraint_failure_rolls_back_every_row_and_media_status() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-quarantine-batch-rollback-{}-{}.db",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library", None, Some("/media"))
            .unwrap();
        for id in ["media-1", "media-2"] {
            db.upsert_media_file(&MediaFileInput {
                id: id.into(),
                root_id: "root-1".into(),
                post_id: None,
                relative_path: format!("{id}.jpg"),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 42,
                sha256: None,
                md5: None,
                width: Some(100),
                height: Some(80),
                duration: None,
            })
            .unwrap();
        }

        let entries = vec![
            QuarantineInput {
                id: "q-1".into(),
                root_id: "root-1".into(),
                media_file_id: Some("media-1".into()),
                original_relative_path: "media-1.jpg".into(),
                quarantine_relative_path: ".danbooru-quarantine/batch/media.jpg".into(),
                reason: "duplicate".into(),
                sha256: None,
            },
            QuarantineInput {
                id: "q-2".into(),
                root_id: "root-1".into(),
                media_file_id: Some("media-2".into()),
                original_relative_path: "media-2.jpg".into(),
                quarantine_relative_path: ".danbooru-quarantine/batch/media.jpg".into(),
                reason: "duplicate".into(),
                sha256: None,
            },
        ];

        assert!(db.quarantine_media_batch(&entries).is_err());
        assert!(db.list_quarantine("root-1", true).unwrap().is_empty());
        assert_eq!(
            db.get_media_file("media-1").unwrap().unwrap().status,
            "active"
        );
        assert_eq!(
            db.get_media_file("media-2").unwrap().unwrap().status,
            "active"
        );

        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn resize_database_mutation_does_not_partially_replace_media_on_quarantine_conflict() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-resize-rollback-{}-{}.db",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library", None, Some("/media"))
            .unwrap();
        let original = MediaFileInput {
            id: "media-1".into(),
            root_id: "root-1".into(),
            post_id: None,
            relative_path: "original.png".into(),
            variant: "original".into(),
            mime_type: "image/png".into(),
            byte_size: 100,
            sha256: Some("original-sha".into()),
            md5: None,
            width: Some(100),
            height: Some(80),
            duration: None,
        };
        db.upsert_media_file(&original).unwrap();
        let conflicting_path = ".danbooru-quarantine/resize-1/original.png";
        db.quarantine_media(&QuarantineInput {
            id: "existing".into(),
            root_id: "root-1".into(),
            media_file_id: None,
            original_relative_path: "unrelated.png".into(),
            quarantine_relative_path: conflicting_path.into(),
            reason: "existing".into(),
            sha256: None,
        })
        .unwrap();
        let replacement = MediaFileInput {
            relative_path: "original.jpg".into(),
            mime_type: "image/jpeg".into(),
            byte_size: 80,
            sha256: None,
            width: Some(90),
            height: Some(72),
            ..original
        };

        assert!(db
            .quarantine_and_replace_media(
                &QuarantineInput {
                    id: "new".into(),
                    root_id: "root-1".into(),
                    media_file_id: None,
                    original_relative_path: "original.png".into(),
                    quarantine_relative_path: conflicting_path.into(),
                    reason: "resize_original".into(),
                    sha256: Some("original-sha".into()),
                },
                &replacement,
            )
            .is_err());
        let stored = db.get_media_file("media-1").unwrap().unwrap();
        assert_eq!(stored.relative_path, "original.png");
        assert_eq!(stored.mime_type, "image/png");
        assert_eq!(stored.status, "active");
        assert_eq!(db.list_quarantine("root-1", true).unwrap().len(), 1);

        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn heic_batch_database_failure_rolls_back_every_media_replacement() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-heic-batch-rollback-{}-{}.db",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library", None, Some("/media"))
            .unwrap();
        let originals = ["first", "second"].map(|name| MediaFileInput {
            id: format!("media-{name}"),
            root_id: "root-1".into(),
            post_id: None,
            relative_path: format!("{name}.heic"),
            variant: "original".into(),
            mime_type: "image/heic".into(),
            byte_size: 100,
            sha256: Some(format!("{name}-sha")),
            md5: None,
            width: None,
            height: None,
            duration: None,
        });
        for original in &originals {
            db.upsert_media_file(original).unwrap();
        }
        let conflict_path = ".danbooru-quarantine/heic-task/second.heic";
        db.quarantine_media(&QuarantineInput {
            id: "existing-conflict".into(),
            root_id: "root-1".into(),
            media_file_id: None,
            original_relative_path: "unrelated.heic".into(),
            quarantine_relative_path: conflict_path.into(),
            reason: "existing".into(),
            sha256: None,
        })
        .unwrap();
        let replacements = originals
            .iter()
            .enumerate()
            .map(|(index, original)| {
                let quarantine_path = if index == 0 {
                    ".danbooru-quarantine/heic-task/first.heic"
                } else {
                    conflict_path
                };
                (
                    QuarantineInput {
                        id: format!("heic-{index}"),
                        root_id: "root-1".into(),
                        media_file_id: None,
                        original_relative_path: original.relative_path.clone(),
                        quarantine_relative_path: quarantine_path.into(),
                        reason: "heic_original".into(),
                        sha256: original.sha256.clone(),
                    },
                    MediaFileInput {
                        relative_path: original.relative_path.replace(".heic", ".jpg"),
                        mime_type: "image/jpeg".into(),
                        byte_size: 80,
                        sha256: None,
                        width: Some(20),
                        height: Some(10),
                        ..original.clone()
                    },
                )
            })
            .collect::<Vec<_>>();

        assert!(db
            .quarantine_and_replace_media_batch(&replacements)
            .is_err());
        for original in &originals {
            let persisted = db.get_media_file(&original.id).unwrap().unwrap();
            assert_eq!(persisted.relative_path, original.relative_path);
            assert_eq!(persisted.mime_type, "image/heic");
        }
        let quarantine = db.list_quarantine("root-1", true).unwrap();
        assert_eq!(quarantine.len(), 1);
        assert_eq!(quarantine[0].id, "existing-conflict");

        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn count_media_files_returns_exact_active_count_for_root() {
        let path =
            std::env::temp_dir().join(format!("danbooru-media-count-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library", None, Some("/media"))
            .unwrap();
        for id in ["media-1", "media-2", "media-3"] {
            db.upsert_media_file(&MediaFileInput {
                id: id.into(),
                root_id: "root-1".into(),
                post_id: None,
                relative_path: format!("{id}.jpg"),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 42,
                sha256: None,
                md5: None,
                width: Some(100),
                height: Some(80),
                duration: None,
            })
            .unwrap();
        }
        db.quarantine_media(&QuarantineInput {
            id: "q-1".into(),
            root_id: "root-1".into(),
            media_file_id: Some("media-3".into()),
            original_relative_path: "media-3.jpg".into(),
            quarantine_relative_path: ".danbooru-quarantine/q-1/media-3.jpg".into(),
            reason: "duplicate".into(),
            sha256: None,
        })
        .unwrap();

        assert_eq!(db.count_media_files("root-1").unwrap(), 2);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn directory_media_selection_is_recursive_and_respects_path_boundaries() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-directory-selection-{}-{}.db",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library", None, Some("/media"))
            .unwrap();
        for (id, relative_path) in [
            ("direct", "people/direct.jpg"),
            ("nested", "people/nested/image.png"),
            ("sibling", "people-old/image.jpg"),
        ] {
            db.upsert_media_file(&MediaFileInput {
                id: id.into(),
                root_id: "root-1".into(),
                post_id: None,
                relative_path: relative_path.into(),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 42,
                sha256: None,
                md5: None,
                width: None,
                height: None,
                duration: None,
            })
            .unwrap();
        }

        let selected = db
            .list_active_media_in_directory("root-1", "people", 10)
            .unwrap();

        assert_eq!(
            selected
                .iter()
                .map(|media| media.id.as_str())
                .collect::<Vec<_>>(),
            ["direct", "nested"]
        );
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn source_directory_selection_excludes_internal_augmentation_rows_before_limit() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-source-directory-selection-{}-{}.db",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library", None, Some("/media"))
            .unwrap();
        for (id, relative_path) in [
            (
                "augmentation",
                "people/.augmentation/task/ready/train/portrait/images/a.png",
            ),
            ("source", "people/z-source.png"),
        ] {
            db.upsert_media_file(&MediaFileInput {
                id: id.into(),
                root_id: "root-1".into(),
                post_id: None,
                relative_path: relative_path.into(),
                variant: "original".into(),
                mime_type: "image/png".into(),
                byte_size: 42,
                sha256: None,
                md5: None,
                width: None,
                height: None,
                duration: None,
            })
            .unwrap();
        }

        let selected = db
            .list_active_source_media_in_directory("root-1", "people", 1)
            .unwrap();

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "source");
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn library_empty_query_uses_bounded_cursor_pagination_with_total() {
        let path =
            std::env::temp_dir().join(format!("danbooru-library-page-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library", None, Some("/media"))
            .unwrap();
        for id in ["media-1", "media-2", "media-3", "media-4", "media-5"] {
            db.upsert_media_file(&MediaFileInput {
                id: id.into(),
                root_id: "root-1".into(),
                post_id: None,
                relative_path: format!("{id}.jpg"),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 42,
                sha256: None,
                md5: None,
                width: Some(100),
                height: Some(80),
                duration: None,
            })
            .unwrap();
        }

        let first = db.list_library_media("root-1", None, 2, "").unwrap();
        let second = db
            .list_library_media("root-1", first.next_cursor.as_deref(), 2, "")
            .unwrap();

        assert_eq!(first.total, 5);
        assert_eq!(
            first
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["media-1", "media-2"]
        );
        assert_eq!(first.next_cursor.as_deref(), Some("media-2"));
        assert_eq!(
            second
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["media-3", "media-4"]
        );
        assert_eq!(second.next_cursor.as_deref(), Some("media-4"));
        let previous = db
            .list_library_media_bidirectional(
                "root-1",
                second.previous_cursor.as_deref(),
                true,
                2,
                "",
                &LibraryMediaFilters::default(),
            )
            .unwrap();
        assert_eq!(
            previous
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["media-1", "media-2"]
        );
        assert_eq!(first.catalog_revision, 1);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn library_directory_filter_keeps_root_and_augmentation_subsets_separate() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-library-directory-{}-{}.db",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library", None, Some("/media"))
            .unwrap();
        for (id, relative_path) in [
            ("root", "source.png"),
            (
                "portrait",
                "dataset-expanded/task/derived/portrait/images/one.png",
            ),
            (
                "upper",
                "dataset-expanded/task/derived/upper_body/images/two.png",
            ),
        ] {
            db.upsert_media_file(&MediaFileInput {
                id: id.into(),
                root_id: "root-1".into(),
                post_id: None,
                relative_path: relative_path.into(),
                variant: "dataset_augmentation".into(),
                mime_type: "image/png".into(),
                byte_size: 42,
                sha256: None,
                md5: None,
                width: Some(1024),
                height: Some(1024),
                duration: None,
            })
            .unwrap();
        }

        let root = db
            .list_library_media_by_page(
                "root-1",
                1,
                60,
                "",
                &LibraryMediaFilters {
                    relative_directory: Some(String::new()),
                    ..LibraryMediaFilters::default()
                },
            )
            .unwrap();
        let portrait = db
            .list_library_media_by_page(
                "root-1",
                1,
                60,
                "",
                &LibraryMediaFilters {
                    relative_directory: Some(
                        "dataset-expanded/task/derived/portrait/images".to_string(),
                    ),
                    ..LibraryMediaFilters::default()
                },
            )
            .unwrap();

        assert_eq!(root.items.len(), 1);
        assert_eq!(root.items[0].id, "root");
        assert_eq!(portrait.items.len(), 1);
        assert_eq!(portrait.items[0].id, "portrait");
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn library_page_filters_an_actual_score_interval_and_reports_its_pages() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-library-score-page-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library", None, Some("/media"))
            .unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute_batch("INSERT INTO posts(id, score) VALUES (1, 3), (2, 7), (3, 48);")
                .unwrap();
        }
        for (id, post_id, width, height) in [
            ("media-1", 1, 512, 512),
            ("media-2", 2, 1024, 512),
            ("media-3", 3, 2048, 1024),
        ] {
            db.upsert_media_file(&MediaFileInput {
                id: id.into(),
                root_id: "root-1".into(),
                post_id: Some(post_id),
                relative_path: format!("{id}.jpg"),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 42,
                sha256: None,
                md5: None,
                width: Some(width),
                height: Some(height),
                duration: None,
            })
            .unwrap();
        }

        let page = db
            .list_library_media_by_page(
                "root-1",
                1,
                1,
                "",
                &LibraryMediaFilters {
                    score_min: Some(0),
                    score_max: Some(9),
                    min_resolution: Some(512),
                    max_resolution: None,
                    post_created_from: None,
                    post_created_to: None,
                    relative_directory: None,
                },
            )
            .unwrap();

        assert_eq!(page.total, 2);
        assert_eq!(page.page, 1);
        assert_eq!(page.total_pages, 2);
        assert_eq!(page.items[0].id, "media-1");
        assert_eq!(
            page.score_ranges,
            vec![
                LibraryScoreRange {
                    score_min: 0,
                    score_max: 9,
                    count: 2,
                },
                LibraryScoreRange {
                    score_min: 40,
                    score_max: 49,
                    count: 1,
                },
            ]
        );
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn library_page_filters_an_actual_resolution_interval() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-library-resolution-page-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library", None, Some("/media"))
            .unwrap();
        for (id, width, height) in [
            ("media-512", 768, 512),
            ("media-768", 1024, 768),
            ("media-1536", 2048, 1536),
        ] {
            db.upsert_media_file(&MediaFileInput {
                id: id.into(),
                root_id: "root-1".into(),
                post_id: None,
                relative_path: format!("{id}.jpg"),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 42,
                sha256: None,
                md5: None,
                width: Some(width),
                height: Some(height),
                duration: None,
            })
            .unwrap();
        }

        let page = db
            .list_library_media_by_page(
                "root-1",
                1,
                60,
                "",
                &LibraryMediaFilters {
                    min_resolution: Some(500),
                    max_resolution: Some(1_000),
                    ..LibraryMediaFilters::default()
                },
            )
            .unwrap();

        assert_eq!(
            page.items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["media-512", "media-768"]
        );
        assert!(!page.resolution_ranges.is_empty());
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ten_thousand_item_library_fixture_keeps_pages_and_payloads_bounded() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-library-ten-thousand-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library", None, Some("/media"))
            .unwrap();
        {
            let mut conn = db.conn.lock().unwrap();
            let transaction = conn.transaction().unwrap();
            {
                let mut insert = transaction
                    .prepare(
                        "INSERT INTO media_files(
                            id, root_id, relative_path, variant, mime_type, byte_size,
                            width, height
                         ) VALUES (?1, 'root-1', ?2, 'original', 'image/jpeg', 42, 100, 80)",
                    )
                    .unwrap();
                for index in 0..10_000_u32 {
                    let id = format!("media-{index:05}");
                    insert
                        .execute(rusqlite::params![id, format!("{index:05}.jpg")])
                        .unwrap();
                }
            }
            transaction.commit().unwrap();
        }

        let first = db.list_library_media("root-1", None, 60, "").unwrap();
        let second = db
            .list_library_media("root-1", first.next_cursor.as_deref(), 60, "")
            .unwrap();

        assert_eq!(first.total, 10_000);
        assert_eq!(first.items.len(), 60);
        assert_eq!(second.items.len(), 60);
        assert_eq!(first.items.first().unwrap().id, "media-00000");
        assert_eq!(second.items.first().unwrap().id, "media-00060");
        assert!(serde_json::to_vec(&first).unwrap().len() < 100 * 1024);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn library_tag_query_requires_every_token_as_an_exact_tag() {
        let path =
            std::env::temp_dir().join(format!("danbooru-library-tags-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library", None, Some("/media"))
            .unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute_batch(
                "INSERT INTO posts(id) VALUES (1), (2), (3);
                 INSERT INTO tags(id, name) VALUES
                    (1, 'cat'), (2, 'blue_hair'), (3, 'caterpillar');
                 INSERT INTO post_tags(post_id, tag_id) VALUES
                    (1, 1), (1, 2), (2, 1), (3, 2), (3, 3);",
            )
            .unwrap();
        }
        for (id, post_id) in [("media-1", 1), ("media-2", 2), ("media-3", 3)] {
            db.upsert_media_file(&MediaFileInput {
                id: id.into(),
                root_id: "root-1".into(),
                post_id: Some(post_id),
                relative_path: format!("{id}.jpg"),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 42,
                sha256: None,
                md5: None,
                width: Some(100),
                height: Some(80),
                duration: None,
            })
            .unwrap();
        }

        let both = db
            .list_library_media("root-1", None, 60, "cat blue_hair")
            .unwrap();
        let cat = db.list_library_media("root-1", None, 60, "cat").unwrap();

        assert_eq!(both.total, 1);
        assert_eq!(both.items[0].id, "media-1");
        assert_eq!(cat.total, 2);
        assert_eq!(
            cat.items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["media-1", "media-2"]
        );
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn library_post_created_time_filter_includes_both_boundaries() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-library-post-time-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library", None, Some("/media"))
            .unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute_batch(
                "INSERT INTO posts(id, posted_at) VALUES
                    (1, 1704067200), (2, 1704153600), (3, 1704240000);
                 INSERT INTO media_files(
                    id, root_id, post_id, relative_path, variant, mime_type, byte_size
                 ) VALUES
                    ('media-1', 'root-1', 1, '1.jpg', 'original', 'image/jpeg', 1),
                    ('media-2', 'root-1', 2, '2.jpg', 'original', 'image/jpeg', 1),
                    ('media-3', 'root-1', 3, '3.jpg', 'original', 'image/jpeg', 1);",
            )
            .unwrap();
        }

        let page = db
            .list_library_media_filtered(
                "root-1",
                None,
                60,
                "",
                &LibraryMediaFilters {
                    post_created_from: Some(1_704_153_600),
                    post_created_to: Some(1_704_240_000),
                    ..LibraryMediaFilters::default()
                },
            )
            .unwrap();

        assert_eq!(
            page.items
                .iter()
                .map(|media| media.id.as_str())
                .collect::<Vec<_>>(),
            ["media-2", "media-3"]
        );
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn repeated_post_upsert_replaces_exact_tag_associations() {
        let path =
            std::env::temp_dir().join(format!("danbooru-post-upsert-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library", None, Some("/media"))
            .unwrap();
        let mut post = PostRecordInput {
            id: 42,
            md5: Some("md5".into()),
            rating: "g".into(),
            score: 10,
            fav_count: 3,
            width: 100,
            height: 80,
            file_ext: Some("jpg".into()),
            file_size: Some(42),
            source: Some("https://example.test/source".into()),
            duration: None,
            post_created_at: None,
            status: "available".into(),
            tag_string: "cat blue_hair".into(),
            tag_string_general: "cat blue_hair".into(),
            tag_string_character: String::new(),
            tag_string_copyright: String::new(),
            tag_string_artist: String::new(),
            tag_string_meta: String::new(),
        };
        db.upsert_post_with_tags(
            &post,
            &[
                PostTagInput::new("cat", 0),
                PostTagInput::new("blue_hair", 0),
            ],
        )
        .unwrap();
        post.rating = "s".into();
        post.score = 20;
        post.tag_string = "dog alice".into();
        post.tag_string_general = "dog".into();
        post.tag_string_character = "alice".into();
        db.upsert_post_with_tags(
            &post,
            &[PostTagInput::new("dog", 0), PostTagInput::new("alice", 4)],
        )
        .unwrap();
        db.upsert_media_file(&MediaFileInput {
            id: "media-42".into(),
            root_id: "root-1".into(),
            post_id: Some(42),
            relative_path: "42.jpg".into(),
            variant: "original".into(),
            mime_type: "image/jpeg".into(),
            byte_size: 42,
            sha256: None,
            md5: Some("md5".into()),
            width: Some(100),
            height: Some(80),
            duration: None,
        })
        .unwrap();

        assert_eq!(
            db.list_library_media("root-1", None, 60, "cat")
                .unwrap()
                .total,
            0
        );
        assert_eq!(
            db.list_library_media("root-1", None, 60, "dog alice")
                .unwrap()
                .total,
            1
        );
        let conn = db.conn.lock().unwrap();
        let (rating, score): (String, i64) = conn
            .query_row("SELECT rating, score FROM posts WHERE id=42", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        let alice_category: i64 = conn
            .query_row("SELECT category FROM tags WHERE name='alice'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!((rating.as_str(), score, alice_category), ("s", 20, 4));
        drop(conn);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn post_created_at_is_normalized_and_missing_updates_preserve_it() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-post-created-at-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        let mut post = PostRecordInput {
            id: 710,
            md5: None,
            rating: "g".to_string(),
            score: 0,
            fav_count: 0,
            width: 100,
            height: 100,
            file_ext: Some("jpg".to_string()),
            file_size: Some(1),
            source: None,
            duration: None,
            post_created_at: Some("2024-01-02T11:04:05+08:00".to_string()),
            status: "available".to_string(),
            tag_string: String::new(),
            tag_string_general: String::new(),
            tag_string_character: String::new(),
            tag_string_copyright: String::new(),
            tag_string_artist: String::new(),
            tag_string_meta: String::new(),
        };

        db.upsert_post_with_tags(&post, &[]).unwrap();
        post.post_created_at = None;
        db.upsert_post_with_tags(&post, &[]).unwrap();

        let metadata = db.get_post_library_metadata(post.id).unwrap().unwrap();
        assert_eq!(metadata.post_created_at, Some(1_704_164_645));
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn local_post_insert_preserves_existing_remote_metadata_and_tags() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-local-post-preserve-{}.db",
            uuid::Uuid::new_v4()
        ));
        let db = Database::open(&path).unwrap();
        db.upsert_post_with_tags(
            &PostRecordInput {
                id: 42,
                md5: Some("remote-md5".into()),
                rating: "q".into(),
                score: 73,
                fav_count: 19,
                width: 1920,
                height: 1080,
                file_ext: Some("png".into()),
                file_size: Some(2048),
                source: Some("https://example.test/original".into()),
                duration: None,
                post_created_at: None,
                status: "available".into(),
                tag_string: "remote_tag".into(),
                tag_string_general: "remote_tag".into(),
                tag_string_character: String::new(),
                tag_string_copyright: String::new(),
                tag_string_artist: String::new(),
                tag_string_meta: String::new(),
            },
            &[PostTagInput::new("remote_tag", 0)],
        )
        .unwrap();

        db.insert_local_post_with_tags_if_missing(
            &PostRecordInput {
                id: 42,
                md5: None,
                rating: "unknown".into(),
                score: 9,
                fav_count: 0,
                width: 0,
                height: 0,
                file_ext: Some("jpg".into()),
                file_size: Some(8),
                source: None,
                duration: None,
                post_created_at: None,
                status: "local".into(),
                tag_string: "local_tag".into(),
                tag_string_general: "local_tag".into(),
                tag_string_character: String::new(),
                tag_string_copyright: String::new(),
                tag_string_artist: String::new(),
                tag_string_meta: String::new(),
            },
            &[PostTagInput::new("local_tag", 0)],
        )
        .unwrap();

        let conn = db.conn.lock().unwrap();
        let metadata: (String, i64, i64, Option<String>, String) = conn
            .query_row(
                "SELECT rating, score, fav_count, source, tag_string FROM posts WHERE id=42",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        let tags: Vec<String> = conn
            .prepare(
                "SELECT tags.name FROM tags
                 JOIN post_tags ON post_tags.tag_id=tags.id
                 WHERE post_tags.post_id=42 ORDER BY tags.name",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            metadata,
            (
                "q".into(),
                73,
                19,
                Some("https://example.test/original".into()),
                "remote_tag".into(),
            )
        );
        assert_eq!(tags, ["remote_tag"]);
        drop(conn);
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn indexed_media_batch_registers_multiple_local_posts_and_files_together() {
        let directory = tempfile::tempdir().unwrap();
        let db = Database::open(&directory.path().join("indexed-batch.db")).unwrap();
        db.create_root("root-1", "Library", None, Some("/media"))
            .unwrap();
        let local_posts = vec![
            (
                PostRecordInput {
                    id: 1,
                    md5: None,
                    rating: "unknown".into(),
                    score: 10,
                    fav_count: 0,
                    width: 100,
                    height: 80,
                    file_ext: Some("jpg".into()),
                    file_size: Some(42),
                    source: None,
                    duration: None,
                    post_created_at: None,
                    status: "local".into(),
                    tag_string: "cat".into(),
                    tag_string_general: "cat".into(),
                    tag_string_character: String::new(),
                    tag_string_copyright: String::new(),
                    tag_string_artist: String::new(),
                    tag_string_meta: String::new(),
                },
                vec![PostTagInput::new("cat", 0)],
            ),
            (
                PostRecordInput {
                    id: 2,
                    md5: None,
                    rating: "unknown".into(),
                    score: 20,
                    fav_count: 0,
                    width: 120,
                    height: 90,
                    file_ext: Some("png".into()),
                    file_size: Some(84),
                    source: None,
                    duration: None,
                    post_created_at: None,
                    status: "local".into(),
                    tag_string: "dog".into(),
                    tag_string_general: "dog".into(),
                    tag_string_character: String::new(),
                    tag_string_copyright: String::new(),
                    tag_string_artist: String::new(),
                    tag_string_meta: String::new(),
                },
                vec![PostTagInput::new("dog", 0)],
            ),
        ];
        let media_files = vec![
            MediaFileInput {
                id: "indexed-1".into(),
                root_id: "root-1".into(),
                post_id: Some(1),
                relative_path: "1.jpg".into(),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 42,
                sha256: None,
                md5: None,
                width: Some(100),
                height: Some(80),
                duration: None,
            },
            MediaFileInput {
                id: "indexed-2".into(),
                root_id: "root-1".into(),
                post_id: Some(2),
                relative_path: "2.png".into(),
                variant: "original".into(),
                mime_type: "image/png".into(),
                byte_size: 84,
                sha256: None,
                md5: None,
                width: Some(120),
                height: Some(90),
                duration: None,
            },
        ];

        db.upsert_indexed_media_batch(&local_posts, &media_files)
            .unwrap();

        assert_eq!(db.count_media_files("root-1").unwrap(), 2);
        assert_eq!(
            db.list_library_media("root-1", None, 60, "cat")
                .unwrap()
                .total,
            1
        );
        assert_eq!(
            db.list_library_media("root-1", None, 60, "dog")
                .unwrap()
                .total,
            1
        );
    }

    #[test]
    fn post_library_metadata_returns_rating_and_stably_sorted_tags() {
        let path =
            std::env::temp_dir().join(format!("danbooru-post-metadata-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.upsert_post_with_tags(
            &PostRecordInput {
                id: 42,
                md5: None,
                rating: "q".into(),
                score: 10,
                fav_count: 3,
                width: 100,
                height: 80,
                file_ext: Some("jpg".into()),
                file_size: Some(42),
                source: None,
                duration: None,
                post_created_at: None,
                status: "available".into(),
                tag_string: "zeta alice cat".into(),
                tag_string_general: "zeta cat".into(),
                tag_string_character: "alice".into(),
                tag_string_copyright: String::new(),
                tag_string_artist: String::new(),
                tag_string_meta: String::new(),
            },
            &[
                PostTagInput::new("zeta", 0),
                PostTagInput::new("alice", 4),
                PostTagInput::new("cat", 0),
            ],
        )
        .unwrap();

        let metadata = db.get_post_library_metadata(42).unwrap().unwrap();

        assert_eq!(metadata.rating, "q");
        assert_eq!(
            metadata
                .tags
                .iter()
                .map(|tag| (tag.category, tag.name.as_str()))
                .collect::<Vec<_>>(),
            [(0, "cat"), (0, "zeta"), (4, "alice")]
        );
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn tag_category_lookup_uses_post_metadata_then_persistent_cache() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-tag-category-{}-{}.db",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let db = Database::open(&path).unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO tags(name, category, post_count) VALUES ('alice', 4, 1)",
                [],
            )
            .unwrap();
        }

        assert_eq!(db.find_known_tag_category("alice").unwrap(), Some(Some(4)));
        assert_eq!(db.find_known_tag_category("unknown").unwrap(), None);
        db.set_tag_category("missing_online", None).unwrap();
        assert_eq!(
            db.find_known_tag_category("missing_online").unwrap(),
            Some(None)
        );
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn root_path_lookup_is_scoped_and_returns_only_active_media() {
        let path =
            std::env::temp_dir().join(format!("danbooru-root-path-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library 1", None, Some("/media/one"))
            .unwrap();
        db.create_root("root-2", "Library 2", None, Some("/media/two"))
            .unwrap();
        for (id, root_id) in [("media-1", "root-1"), ("media-2", "root-2")] {
            db.upsert_media_file(&MediaFileInput {
                id: id.into(),
                root_id: root_id.into(),
                post_id: None,
                relative_path: "same/42.jpg".into(),
                variant: "original".into(),
                mime_type: "image/jpeg".into(),
                byte_size: 42,
                sha256: None,
                md5: None,
                width: Some(100),
                height: Some(80),
                duration: None,
            })
            .unwrap();
        }

        assert_eq!(
            db.find_media_by_root_path("root-1", "same/42.jpg")
                .unwrap()
                .unwrap()
                .id,
            "media-1"
        );
        db.quarantine_media(&QuarantineInput {
            id: "q-1".into(),
            root_id: "root-1".into(),
            media_file_id: Some("media-1".into()),
            original_relative_path: "same/42.jpg".into(),
            quarantine_relative_path: ".danbooru-quarantine/q-1/42.jpg".into(),
            reason: "duplicate".into(),
            sha256: None,
        })
        .unwrap();

        assert!(db
            .find_media_by_root_path("root-1", "same/42.jpg")
            .unwrap()
            .is_none());
        assert_eq!(
            db.find_media_by_root_path("root-2", "same/42.jpg")
                .unwrap()
                .unwrap()
                .id,
            "media-2"
        );
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn download_lookup_is_scoped_to_root_and_variant() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-download-lookup-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library 1", None, Some("/media/one"))
            .unwrap();
        db.create_root("root-2", "Library 2", None, Some("/media/two"))
            .unwrap();
        for (id, root_id, variant) in [
            ("root-1-original", "root-1", "original"),
            ("root-2-webm", "root-2", "ugoira_webm"),
        ] {
            db.upsert_media_file(&MediaFileInput {
                id: id.into(),
                root_id: root_id.into(),
                post_id: None,
                relative_path: format!("{id}.bin"),
                variant: variant.into(),
                mime_type: "application/octet-stream".into(),
                byte_size: 42,
                sha256: None,
                md5: Some("same-md5".into()),
                width: None,
                height: None,
                duration: None,
            })
            .unwrap();
        }

        assert!(db
            .find_active_media_for_download("root-1", None, Some("same-md5"), "original")
            .unwrap()
            .is_some());
        assert!(db
            .find_active_media_for_download("root-1", None, Some("same-md5"), "ugoira_webm")
            .unwrap()
            .is_none());
        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn downloaded_post_history_is_scoped_to_the_exact_directory_and_survives_media_deletion() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-directory-download-history-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library 1", None, Some("/media/one"))
            .unwrap();

        assert!(!db
            .was_post_downloaded_in_directory("root-1", "characters/alice", 42)
            .unwrap());
        db.record_downloaded_post_in_directory("root-1", "characters/alice", 42, Some("task-1"))
            .unwrap();

        assert!(db
            .was_post_downloaded_in_directory("root-1", "characters/alice", 42)
            .unwrap());
        assert!(!db
            .was_post_downloaded_in_directory("root-1", "", 42)
            .unwrap());
        assert!(!db
            .was_post_downloaded_in_directory("root-1", "characters/bob", 42)
            .unwrap());

        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn upgrading_backfills_download_history_from_existing_catalogued_media() {
        let path = std::env::temp_dir().join(format!(
            "danbooru-directory-download-history-backfill-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = Database::open(&path).unwrap();
        db.create_root("root-1", "Library 1", None, Some("/media/one"))
            .unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute("INSERT INTO posts(id) VALUES (42)", [])
                .unwrap();
            conn.execute(
                "INSERT INTO media_files(id, root_id, post_id, relative_path, mime_type)
                 VALUES ('media-42', 'root-1', 42, 'characters/alice/42.jpg', 'image/jpeg')",
                [],
            )
            .unwrap();
        }
        drop(db);

        let upgraded = Database::open(&path).unwrap();
        assert!(upgraded
            .was_post_downloaded_in_directory("root-1", "characters/alice", 42)
            .unwrap());

        drop(upgraded);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn database_health_check_executes_a_real_sqlite_probe() {
        let directory = tempfile::tempdir().unwrap();
        let db = Database::open(&directory.path().join("health.db")).unwrap();

        db.health_check().expect("SELECT probe should succeed");
    }

    #[test]
    fn database_pool_keeps_reads_available_when_another_connection_is_checked_out() {
        let directory = tempfile::tempdir().unwrap();
        let db = std::sync::Arc::new(Database::open(&directory.path().join("pool.db")).unwrap());
        let held = db.conn.lock().unwrap();
        let reader = db.clone();

        let probe = std::thread::spawn(move || reader.health_check());

        probe.join().unwrap().expect("a second pooled connection should serve the read");
        drop(held);
        let diagnostics = db.pool_diagnostics();
        assert!(diagnostics.total >= 2);
        assert_eq!(diagnostics.in_use, 0);
    }
}

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UgoiraPolicy {
    #[default]
    WebmAndZip,
    WebmOnly,
    ZipOnly,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(default)]
pub struct StoredSettings {
    pub version: u32,
    pub danbooru_username: String,
    pub vllm_base_url: String,
    pub vllm_allowed_hosts: Vec<String>,
    pub vllm_model: String,
    pub vllm_system_prompt: String,
    pub vllm_tag_mode: crate::services::vllm::TagWriteMode,
    pub vllm_language: crate::services::vllm::VllmLanguage,
    pub vllm_max_tags: usize,
    pub vllm_max_length: usize,
    pub vllm_verify_danbooru: bool,
    pub vllm_reference_existing: bool,
    pub vllm_concurrency: usize,
    pub proxy_url: Option<String>,
    pub download_concurrency: usize,
    pub filename_template: String,
    pub ugoira_policy: UgoiraPolicy,
    pub blur_sensitive_media: bool,
    /// URL path of the uploaded background image, empty when unset.
    pub background_image: String,
    /// Background overlay opacity as a percentage in 0..=100.
    pub background_opacity: u8,
    pub legacy_media_path_suggestion: Option<String>,
    /// Directory containing the local CL Tagger ONNX model and tag mapping.
    pub cl_tagger_model_path: String,
    pub cl_tagger_general_threshold: f32,
    pub cl_tagger_character_threshold: f32,
    pub cl_tagger_copyright_threshold: f32,
    pub cl_tagger_quality_threshold: f32,
    pub cl_tagger_max_tags: usize,
}

impl Default for StoredSettings {
    fn default() -> Self {
        Self {
            version: 6,
            danbooru_username: String::new(),
            vllm_base_url: "http://127.0.0.1:8000/v1".to_string(),
            vllm_allowed_hosts: Vec::new(),
            vllm_model: crate::services::vllm::DEFAULT_MODEL.to_string(),
            vllm_system_prompt: crate::services::vllm::DEFAULT_SYSTEM_PROMPT.to_string(),
            vllm_tag_mode: crate::services::vllm::TagWriteMode::Overwrite,
            vllm_language: crate::services::vllm::VllmLanguage::English,
            vllm_max_tags: 60,
            vllm_max_length: 400,
            vllm_verify_danbooru: true,
            vllm_reference_existing: false,
            vllm_concurrency: 16,
            proxy_url: None,
            download_concurrency: 8,
            filename_template: crate::services::danbooru::DEFAULT_FILENAME_TEMPLATE.to_string(),
            ugoira_policy: UgoiraPolicy::WebmAndZip,
            blur_sensitive_media: true,
            background_image: String::new(),
            background_opacity: 18,
            legacy_media_path_suggestion: None,
            cl_tagger_model_path: String::new(),
            cl_tagger_general_threshold: 0.35,
            cl_tagger_character_threshold: 0.6,
            cl_tagger_copyright_threshold: 0.6,
            cl_tagger_quality_threshold: 0.35,
            cl_tagger_max_tags: 60,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigValidationError {
    pub field: &'static str,
    pub message: String,
}

impl StoredSettings {
    pub fn validate(&self) -> Result<(), ConfigValidationError> {
        if !(1..=32).contains(&self.download_concurrency) {
            return Err(ConfigValidationError {
                field: "download_concurrency",
                message: "下载并发必须在 1..=32 之间".to_string(),
            });
        }
        if !(1..=32).contains(&self.vllm_concurrency) {
            return Err(ConfigValidationError {
                field: "vllm_concurrency",
                message: "vLLM 并发必须在 1..=32 之间".to_string(),
            });
        }
        if !(1..=200).contains(&self.vllm_max_tags) {
            return Err(ConfigValidationError {
                field: "vllm_max_tags",
                message: "vLLM 最大标签数必须在 1..=200 之间".to_string(),
            });
        }
        if !(1..=4_000).contains(&self.vllm_max_length) {
            return Err(ConfigValidationError {
                field: "vllm_max_length",
                message: "vLLM 描述长度必须在 1..=4000 之间".to_string(),
            });
        }
        if self.vllm_model.trim().is_empty() || self.vllm_model.len() > 1_024 {
            return Err(ConfigValidationError {
                field: "vllm_model",
                message: "vLLM 模型名长度必须在 1..=1024 字节之间".to_string(),
            });
        }
        if self.vllm_system_prompt.trim().is_empty() || self.vllm_system_prompt.len() > 64 * 1024 {
            return Err(ConfigValidationError {
                field: "vllm_system_prompt",
                message: "vLLM 系统提示词长度必须在 1..=65536 字节之间".to_string(),
            });
        }
        if crate::services::danbooru::validate_filename_template(&self.filename_template).is_err() {
            return Err(ConfigValidationError {
                field: "filename_template",
                message: "文件名模板无效".to_string(),
            });
        }
        if let Some(proxy) = self.proxy_url.as_deref() {
            let url = url::Url::parse(proxy).map_err(|_| ConfigValidationError {
                field: "proxy_url",
                message: "代理地址无效".to_string(),
            })?;
            if !matches!(
                url.scheme(),
                "http" | "https" | "socks4" | "socks5" | "socks5h"
            ) || url.host_str().is_none()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(ConfigValidationError {
                    field: "proxy_url",
                    message: "代理仅支持 http、https、socks4 或 socks5 URL".to_string(),
                });
            }
            if !url.username().is_empty() || url.password().is_some() {
                return Err(ConfigValidationError {
                    field: "proxy_url",
                    message: "代理凭据不能保存在设置 URL 中".to_string(),
                });
            }
        }
        if self.cl_tagger_model_path.len() > 1_024 {
            return Err(ConfigValidationError {
                field: "cl_tagger_model_path",
                message: "CL Tagger 模型路径过长".to_string(),
            });
        }
        for (field, value) in [
            ("cl_tagger_general_threshold", self.cl_tagger_general_threshold),
            ("cl_tagger_character_threshold", self.cl_tagger_character_threshold),
            ("cl_tagger_copyright_threshold", self.cl_tagger_copyright_threshold),
            ("cl_tagger_quality_threshold", self.cl_tagger_quality_threshold),
        ] {
            if !(0.0..1.0).contains(&value) {
                return Err(ConfigValidationError {
                    field,
                    message: "CL Tagger 阈值必须在 0..1 之间".to_string(),
                });
            }
        }
        if !(1..=500).contains(&self.cl_tagger_max_tags) {
            return Err(ConfigValidationError {
                field: "cl_tagger_max_tags",
                message: "CL Tagger 标签数限制必须在 1..=500 之间".to_string(),
            });
        }
        validate_vllm_endpoint(&self.vllm_base_url, &self.vllm_allowed_hosts)?;
        Ok(())
    }
}

fn validate_vllm_endpoint(
    candidate: &str,
    allowed_hosts: &[String],
) -> Result<(), ConfigValidationError> {
    crate::services::vllm::validate_endpoint(candidate, allowed_hosts)
        .map(|_| ())
        .map_err(|message| ConfigValidationError {
            field: "vllm_base_url",
            message,
        })
}

pub fn load_settings(path: &Path) -> Result<StoredSettings, String> {
    if !path.exists() {
        let settings = StoredSettings::default();
        save_settings(path, &settings)?;
        return Ok(settings);
    }
    let raw = fs::read_to_string(path)
        .map_err(|error| format!("无法读取设置文件 {}: {error}", path.display()))?;
    let settings: StoredSettings =
        serde_json::from_str(&raw).map_err(|error| format!("设置文件格式无效: {error}"))?;
    settings.validate().map_err(|error| error.message)?;
    Ok(settings)
}

pub fn apply_vllm_base_url_override(
    settings: &mut StoredSettings,
    candidate: Option<&str>,
) -> Result<(), ConfigValidationError> {
    let Some(candidate) = candidate else {
        return Ok(());
    };
    let candidate = candidate.trim();
    validate_vllm_endpoint(candidate, &settings.vllm_allowed_hosts)?;
    settings.vllm_base_url = candidate.to_string();
    Ok(())
}

pub fn save_settings(path: &Path, settings: &StoredSettings) -> Result<(), String> {
    settings.validate().map_err(|error| error.message)?;
    let parent: PathBuf = path
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "设置文件缺少父目录".to_string())?;
    fs::create_dir_all(&parent)
        .map_err(|error| format!("无法创建数据目录 {}: {error}", parent.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(&parent)
        .map_err(|error| format!("无法创建设置临时文件: {error}"))?;
    serde_json::to_writer_pretty(&mut temporary, settings)
        .map_err(|error| format!("无法序列化设置: {error}"))?;
    temporary
        .write_all(b"\n")
        .map_err(|error| format!("无法写入设置: {error}"))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("无法同步设置: {error}"))?;
    temporary
        .persist(path)
        .map_err(|error| format!("无法原子替换设置文件: {}", error.error))?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacySettingsMigration {
    UpToDate,
    Migrated,
}

pub fn migrate_legacy_settings(
    settings_path: &Path,
    legacy_config_path: &Path,
    legacy_vllm_path: &Path,
) -> Result<LegacySettingsMigration, String> {
    let mut settings = if settings_path.exists() {
        let raw = fs::read_to_string(settings_path)
            .map_err(|error| format!("无法读取设置文件 {}: {error}", settings_path.display()))?;
        serde_json::from_str::<StoredSettings>(&raw)
            .map_err(|error| format!("设置文件格式无效: {error}"))?
    } else {
        StoredSettings::default()
    };
    let normalized_legacy_suggestion = settings
        .legacy_media_path_suggestion
        .as_deref()
        .map(|suggestion| legacy_media_path(legacy_config_path, suggestion));
    let legacy_suggestion_changed =
        normalized_legacy_suggestion != settings.legacy_media_path_suggestion;
    if settings.version >= 6 && settings_path.exists() && !legacy_suggestion_changed {
        return Ok(LegacySettingsMigration::UpToDate);
    }
    settings.legacy_media_path_suggestion = normalized_legacy_suggestion;

    let defaults = StoredSettings::default();
    if let Ok(raw) = fs::read_to_string(legacy_config_path) {
        if let Ok(document) = serde_json::from_str::<serde_json::Value>(&raw) {
            let download = document
                .get("download")
                .and_then(serde_json::Value::as_object);
            if settings.danbooru_username == defaults.danbooru_username {
                if let Some(username) = download
                    .and_then(|value| value.get("username"))
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                {
                    settings.danbooru_username = username.trim().to_string();
                }
            }
            if settings.download_concurrency == defaults.download_concurrency {
                if let Some(concurrency) = download
                    .and_then(|value| value.get("concurrency"))
                    .and_then(serde_json::Value::as_u64)
                    .filter(|value| (1..=32).contains(value))
                {
                    settings.download_concurrency = concurrency as usize;
                }
            }
            if settings.legacy_media_path_suggestion.is_none() {
                settings.legacy_media_path_suggestion = download
                    .and_then(|value| value.get("save_path"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| legacy_media_path(legacy_config_path, value));
            }
            if settings.proxy_url.is_none() {
                settings.proxy_url = download.and_then(legacy_proxy_url);
            }
        }
    }
    if let Ok(raw) = fs::read_to_string(legacy_vllm_path) {
        if let Ok(document) = serde_json::from_str::<serde_json::Value>(&raw) {
            if settings.vllm_base_url == defaults.vllm_base_url {
                if let Some(endpoint) = document
                    .get("base_url")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .filter(|value| {
                        validate_vllm_endpoint(value, &settings.vllm_allowed_hosts).is_ok()
                    })
                {
                    settings.vllm_base_url = endpoint.to_string();
                }
            }
            if settings.vllm_model == defaults.vllm_model {
                if let Some(model) = document
                    .get("model")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty() && value.len() <= 1_024)
                {
                    settings.vllm_model = model.to_string();
                }
            }
            if settings.vllm_system_prompt == defaults.vllm_system_prompt {
                if let Some(prompt) = document
                    .get("system_prompt")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty() && value.len() <= 64 * 1024)
                {
                    settings.vllm_system_prompt = prompt.to_string();
                }
            }
            if settings.vllm_tag_mode == defaults.vllm_tag_mode {
                if let Some(mode) = document.get("tag_mode").and_then(|value| {
                    serde_json::from_value::<crate::services::vllm::TagWriteMode>(value.clone())
                        .ok()
                }) {
                    settings.vllm_tag_mode = mode;
                }
            }
            if settings.vllm_concurrency == defaults.vllm_concurrency {
                if let Some(concurrency) = document
                    .get("concurrency")
                    .and_then(serde_json::Value::as_u64)
                    .filter(|value| (1..=32).contains(value))
                {
                    settings.vllm_concurrency = concurrency as usize;
                }
            }
            if settings.vllm_language == defaults.vllm_language {
                if let Some(language) = document.get("language").and_then(|value| {
                    serde_json::from_value::<crate::services::vllm::VllmLanguage>(value.clone())
                        .ok()
                }) {
                    settings.vllm_language = language;
                }
            }
            if settings.vllm_max_tags == defaults.vllm_max_tags {
                if let Some(value) = document
                    .get("max_tags")
                    .and_then(serde_json::Value::as_u64)
                    .filter(|value| (1..=200).contains(value))
                {
                    settings.vllm_max_tags = value as usize;
                }
            }
            if settings.vllm_max_length == defaults.vllm_max_length {
                if let Some(value) = document
                    .get("max_length")
                    .and_then(serde_json::Value::as_u64)
                    .filter(|value| (1..=4_000).contains(value))
                {
                    settings.vllm_max_length = value as usize;
                }
            }
            if settings.vllm_verify_danbooru == defaults.vllm_verify_danbooru {
                if let Some(value) = document
                    .get("verify_danbooru")
                    .and_then(serde_json::Value::as_bool)
                {
                    settings.vllm_verify_danbooru = value;
                }
            }
            if settings.vllm_reference_existing == defaults.vllm_reference_existing {
                if let Some(value) = document
                    .get("reference_existing")
                    .and_then(serde_json::Value::as_bool)
                {
                    settings.vllm_reference_existing = value;
                }
            }
        }
    }
    settings.version = 6;
    save_settings(settings_path, &settings)?;
    Ok(LegacySettingsMigration::Migrated)
}

fn legacy_media_path(config_path: &Path, value: &str) -> String {
    let path = Path::new(value);
    let is_portable_absolute = value.starts_with('/') || is_windows_drive_absolute(value);
    let resolved = if is_portable_absolute || value.starts_with("\\\\") {
        path.to_path_buf()
    } else {
        config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(path)
    };
    let rendered = resolved.to_string_lossy().into_owned();
    if let Some(drive_path) = rendered.strip_prefix(r"\\?\") {
        if is_windows_drive_absolute(drive_path) {
            return drive_path.to_string();
        }
    }
    rendered
}

fn is_windows_drive_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

fn legacy_proxy_url(download: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let proxy = download
        .get("proxy")
        .and_then(serde_json::Value::as_str)?
        .trim();
    if proxy.is_empty() {
        return None;
    }
    let port = download
        .get("proxy_port")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|value| *value > 0);
    let mut url = if proxy.contains("://") {
        url::Url::parse(proxy).ok()?
    } else {
        url::Url::parse(&format!("http://{proxy}")).ok()?
    };
    if url.port().is_none() {
        if let Some(port) = port {
            url.set_port(Some(port)).ok()?;
        }
    }
    if !matches!(
        url.scheme(),
        "http" | "https" | "socks4" | "socks5" | "socks5h"
    ) || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some(url.to_string().trim_end_matches('/').to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacySecretMigration {
    NoSecret,
    Migrated,
}

pub fn migrate_legacy_secret(
    path: &Path,
    field_path: &[&str],
    kind: crate::secrets::SecretKind,
    secrets: &crate::secrets::SecretManager,
) -> Result<LegacySecretMigration, String> {
    if !path.exists() || field_path.is_empty() {
        return Ok(LegacySecretMigration::NoSecret);
    }
    let raw = fs::read_to_string(path)
        .map_err(|error| format!("无法读取旧配置 {}: {error}", path.display()))?;
    let mut document: serde_json::Value =
        serde_json::from_str(&raw).map_err(|error| format!("旧配置格式无效: {error}"))?;
    let secret = field_path
        .iter()
        .try_fold(&document, |value, key| value.get(*key))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let Some(secret) = secret else {
        return Ok(LegacySecretMigration::NoSecret);
    };

    secrets
        .set_persistent(kind, &secret)
        .map_err(|_| "系统凭据库不可用或写入后无法验证".to_string())?;

    let mut cursor = &mut document;
    for key in &field_path[..field_path.len() - 1] {
        cursor = cursor
            .get_mut(*key)
            .ok_or_else(|| "旧配置密钥路径在迁移期间发生变化".to_string())?;
    }
    cursor[field_path[field_path.len() - 1]] = serde_json::Value::String(String::new());
    persist_json(path, &document)?;
    Ok(LegacySecretMigration::Migrated)
}

fn persist_json(path: &Path, document: &serde_json::Value) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "配置文件缺少父目录".to_string())?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("无法创建配置临时文件: {error}"))?;
    serde_json::to_writer_pretty(&mut temporary, document)
        .map_err(|error| format!("无法序列化配置: {error}"))?;
    temporary
        .write_all(b"\n")
        .map_err(|error| format!("无法写入配置: {error}"))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("无法同步配置: {error}"))?;
    temporary
        .persist(path)
        .map_err(|error| format!("无法原子替换配置: {}", error.error))?;
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct PublicConfig {
    pub danbooru_username: String,
    pub danbooru_api_key_configured: bool,
    pub vllm_api_key_configured: bool,
    pub vllm_base_url: String,
    pub vllm_allowed_hosts: Vec<String>,
    pub vllm_model: String,
    pub vllm_system_prompt: String,
    pub vllm_tag_mode: crate::services::vllm::TagWriteMode,
    pub vllm_language: crate::services::vllm::VllmLanguage,
    pub vllm_max_tags: usize,
    pub vllm_max_length: usize,
    pub vllm_verify_danbooru: bool,
    pub vllm_reference_existing: bool,
    pub vllm_concurrency: usize,
    pub proxy_url: Option<String>,
    pub download_concurrency: usize,
    pub filename_template: String,
    pub ugoira_policy: UgoiraPolicy,
    pub blur_sensitive_media: bool,
    pub background_image: String,
    pub background_opacity: u8,
    pub cl_tagger_model_path: String,
    pub cl_tagger_general_threshold: f32,
    pub cl_tagger_character_threshold: f32,
    pub cl_tagger_copyright_threshold: f32,
    pub cl_tagger_quality_threshold: f32,
    pub cl_tagger_max_tags: usize,
}

impl PublicConfig {
    pub fn from_settings(
        settings: &StoredSettings,
        danbooru_api_key_configured: bool,
        vllm_api_key_configured: bool,
    ) -> Self {
        Self {
            danbooru_username: settings.danbooru_username.clone(),
            danbooru_api_key_configured,
            vllm_api_key_configured,
            vllm_base_url: settings.vllm_base_url.clone(),
            vllm_allowed_hosts: settings.vllm_allowed_hosts.clone(),
            vllm_model: settings.vllm_model.clone(),
            vllm_system_prompt: settings.vllm_system_prompt.clone(),
            vllm_tag_mode: settings.vllm_tag_mode,
            vllm_language: settings.vllm_language,
            vllm_max_tags: settings.vllm_max_tags,
            vllm_max_length: settings.vllm_max_length,
            vllm_verify_danbooru: settings.vllm_verify_danbooru,
            vllm_reference_existing: settings.vllm_reference_existing,
            vllm_concurrency: settings.vllm_concurrency,
            proxy_url: settings.proxy_url.as_ref().and_then(|proxy| {
                url::Url::parse(proxy).ok().and_then(|url| {
                    (url.username().is_empty() && url.password().is_none()).then(|| proxy.clone())
                })
            }),
            download_concurrency: settings.download_concurrency,
            filename_template: settings.filename_template.clone(),
            ugoira_policy: settings.ugoira_policy,
            blur_sensitive_media: settings.blur_sensitive_media,
            background_image: settings.background_image.clone(),
            background_opacity: settings.background_opacity,
            cl_tagger_model_path: settings.cl_tagger_model_path.clone(),
            cl_tagger_general_threshold: settings.cl_tagger_general_threshold,
            cl_tagger_character_threshold: settings.cl_tagger_character_threshold,
            cl_tagger_copyright_threshold: settings.cl_tagger_copyright_threshold,
            cl_tagger_quality_threshold: settings.cl_tagger_quality_threshold,
            cl_tagger_max_tags: settings.cl_tagger_max_tags,
        }
    }
}

#[cfg(test)]
mod secure_config_tests {
    use super::{
        apply_vllm_base_url_override, load_settings, migrate_legacy_secret,
        migrate_legacy_settings, save_settings, LegacySecretMigration, LegacySettingsMigration,
        PublicConfig, StoredSettings,
    };
    use crate::secrets::{CredentialVault, SecretError, SecretKind, SecretManager};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct TestVault(Mutex<Option<String>>);

    impl CredentialVault for TestVault {
        fn get(&self, _kind: SecretKind) -> Result<Option<String>, SecretError> {
            Ok(self.0.lock().unwrap().clone())
        }

        fn set(&self, _kind: SecretKind, value: &str) -> Result<(), SecretError> {
            *self.0.lock().unwrap() = Some(value.to_string());
            Ok(())
        }

        fn delete(&self, _kind: SecretKind) -> Result<(), SecretError> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
    }

    #[test]
    fn public_config_exposes_only_secret_status() {
        let public = PublicConfig::from_settings(&StoredSettings::default(), true, false);

        let value = serde_json::to_value(public).unwrap();

        assert_eq!(value["danbooru_api_key_configured"], true);
        assert_eq!(value["vllm_api_key_configured"], false);
        assert_eq!(value["blur_sensitive_media"], true);
        assert!(value.get("api_key").is_none());
        assert!(!value.to_string().contains("secret"));
    }

    #[test]
    fn download_concurrency_is_bounded_but_legacy_sixteen_is_valid() {
        let mut settings = StoredSettings {
            download_concurrency: 16,
            ..StoredSettings::default()
        };
        assert!(settings.validate().is_ok());

        settings.download_concurrency = 33;
        assert_eq!(
            settings.validate().unwrap_err().field,
            "download_concurrency"
        );
    }

    #[test]
    fn vllm_concurrency_is_independently_bounded() {
        let mut settings = StoredSettings {
            download_concurrency: 32,
            vllm_concurrency: 1,
            ..StoredSettings::default()
        };
        assert!(settings.validate().is_ok());
        assert_eq!(
            PublicConfig::from_settings(&settings, false, false).vllm_concurrency,
            1
        );

        settings.vllm_concurrency = 0;
        assert_eq!(settings.validate().unwrap_err().field, "vllm_concurrency");
    }

    #[test]
    fn vllm_model_must_be_non_empty_and_bounded() {
        let mut settings = StoredSettings {
            vllm_model: "   ".to_string(),
            ..StoredSettings::default()
        };
        assert_eq!(settings.validate().unwrap_err().field, "vllm_model");

        settings.vllm_model = "m".repeat(1_025);
        assert_eq!(settings.validate().unwrap_err().field, "vllm_model");
    }

    #[test]
    fn vllm_system_prompt_must_be_non_empty_and_bounded() {
        let mut settings = StoredSettings {
            vllm_system_prompt: "\n\t".to_string(),
            ..StoredSettings::default()
        };
        assert_eq!(settings.validate().unwrap_err().field, "vllm_system_prompt");

        settings.vllm_system_prompt = "p".repeat(64 * 1024 + 1);
        assert_eq!(settings.validate().unwrap_err().field, "vllm_system_prompt");
    }

    #[test]
    fn vllm_tag_mode_rejects_unknown_enum_values() {
        let mut document = serde_json::to_value(StoredSettings::default()).unwrap();
        document["vllm_tag_mode"] = serde_json::Value::String("merge".to_string());

        assert!(serde_json::from_value::<StoredSettings>(document).is_err());
    }

    #[test]
    fn settings_reject_filename_templates_that_can_escape_a_root() {
        let settings = StoredSettings {
            filename_template: "../{id}.{ext}".to_string(),
            ..StoredSettings::default()
        };

        assert_eq!(settings.validate().unwrap_err().field, "filename_template");
    }

    #[test]
    fn proxy_credentials_are_rejected_and_never_exposed_by_public_config() {
        let settings = StoredSettings {
            proxy_url: Some("http://proxy-user:proxy-password@127.0.0.1:8080".to_string()),
            ..StoredSettings::default()
        };

        assert_eq!(settings.validate().unwrap_err().field, "proxy_url");
        let public = PublicConfig::from_settings(&settings, false, false);
        assert!(!serde_json::to_string(&public)
            .unwrap()
            .contains("proxy-password"));
    }

    #[test]
    fn vllm_endpoint_is_loopback_unless_host_is_explicitly_allowed() {
        let mut settings = StoredSettings {
            vllm_base_url: "https://vision.example.test/v1".to_string(),
            ..StoredSettings::default()
        };
        assert_eq!(settings.validate().unwrap_err().field, "vllm_base_url");

        settings.vllm_allowed_hosts = vec!["vision.example.test:443".to_string()];
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn runtime_vllm_endpoint_override_is_validated_without_persisting_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("app_settings.json");
        save_settings(&path, &StoredSettings::default()).unwrap();
        let mut settings = load_settings(&path).unwrap();

        apply_vllm_base_url_override(&mut settings, Some("http://127.0.0.1:8001/v1")).unwrap();

        assert_eq!(settings.vllm_base_url, "http://127.0.0.1:8001/v1");
        assert_eq!(
            load_settings(&path).unwrap().vllm_base_url,
            "http://127.0.0.1:8000/v1"
        );
    }

    #[test]
    fn stored_settings_use_the_same_exact_port_allowlist_policy_as_vllm() {
        let mut settings = StoredSettings {
            vllm_base_url: "https://vision.example.test:8443/v1".to_string(),
            vllm_allowed_hosts: vec!["vision.example.test:443".to_string()],
            ..StoredSettings::default()
        };

        assert_eq!(settings.validate().unwrap_err().field, "vllm_base_url");
        settings.vllm_allowed_hosts = vec!["vision.example.test:8443".to_string()];
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn settings_round_trip_through_an_explicit_data_path() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("app_settings.json");
        let expected = StoredSettings {
            download_concurrency: 16,
            danbooru_username: "local-user".to_string(),
            ..StoredSettings::default()
        };

        save_settings(&path, &expected).unwrap();
        let loaded = load_settings(&path).unwrap();

        assert_eq!(loaded, expected);
        assert!(!std::fs::read_to_string(path).unwrap().contains("api_key"));
    }

    #[test]
    fn legacy_non_secret_settings_preserve_a_valid_concurrency_sixteen() {
        let directory = tempfile::tempdir().unwrap();
        let settings_path = directory.path().join("app_settings.json");
        let legacy_path = directory.path().join("config.json");
        let legacy_vllm_path = directory.path().join("vllm_config.json");
        std::fs::write(
            &legacy_path,
            serde_json::json!({
                "download": {
                    "username": "legacy-user",
                    "api_key": "must-never-enter-settings",
                    "concurrency": 16,
                    "save_path": "legacy-media"
                }
            })
            .to_string(),
        )
        .unwrap();

        let result =
            migrate_legacy_settings(&settings_path, &legacy_path, &legacy_vllm_path).unwrap();
        let settings = load_settings(&settings_path).unwrap();
        let stored = std::fs::read_to_string(settings_path).unwrap();

        assert_eq!(result, LegacySettingsMigration::Migrated);
        assert_eq!(settings.download_concurrency, 16);
        assert_eq!(settings.danbooru_username, "legacy-user");
        assert_eq!(
            settings.legacy_media_path_suggestion.as_deref(),
            Some(
                directory
                    .path()
                    .join("legacy-media")
                    .to_string_lossy()
                    .as_ref()
            )
        );
        assert!(!stored.contains("must-never-enter-settings"));
    }

    #[test]
    fn version_four_relative_legacy_suggestion_is_upgraded_to_an_absolute_path() {
        let directory = tempfile::tempdir().unwrap();
        let settings_path = directory.path().join("app_settings.json");
        let legacy_path = directory.path().join("config.json");
        let legacy_vllm_path = directory.path().join("vllm_config.json");
        let existing = StoredSettings {
            version: 4,
            legacy_media_path_suggestion: Some("legacy-media".to_string()),
            ..StoredSettings::default()
        };
        save_settings(&settings_path, &existing).unwrap();

        let result =
            migrate_legacy_settings(&settings_path, &legacy_path, &legacy_vllm_path).unwrap();
        let migrated = load_settings(&settings_path).unwrap();

        assert_eq!(result, LegacySettingsMigration::Migrated);
        assert_eq!(migrated.version, 6);
        assert_eq!(
            migrated.legacy_media_path_suggestion.as_deref(),
            Some(
                directory
                    .path()
                    .join("legacy-media")
                    .to_string_lossy()
                    .as_ref()
            )
        );
    }

    #[cfg(windows)]
    #[test]
    fn extended_windows_config_path_is_not_exposed_in_the_legacy_suggestion() {
        let directory = tempfile::tempdir().unwrap();
        let settings_path = directory.path().join("app_settings.json");
        let regular_legacy_path = directory.path().join("config.json");
        let extended_legacy_path =
            std::path::PathBuf::from(format!(r"\\?\{}", regular_legacy_path.display()));
        let legacy_vllm_path = directory.path().join("vllm_config.json");
        save_settings(
            &settings_path,
            &StoredSettings {
                version: 5,
                legacy_media_path_suggestion: Some("legacy-media".to_string()),
                ..StoredSettings::default()
            },
        )
        .unwrap();

        migrate_legacy_settings(&settings_path, &extended_legacy_path, &legacy_vllm_path).unwrap();
        let migrated = load_settings(&settings_path).unwrap();

        assert_eq!(
            migrated.legacy_media_path_suggestion.as_deref(),
            Some(
                directory
                    .path()
                    .join("legacy-media")
                    .to_string_lossy()
                    .as_ref()
            )
        );
    }

    #[test]
    fn legacy_vllm_execution_settings_are_migrated_without_the_api_key() {
        let directory = tempfile::tempdir().unwrap();
        let settings_path = directory.path().join("app_settings.json");
        let legacy_path = directory.path().join("config.json");
        let legacy_vllm_path = directory.path().join("vllm_config.json");
        std::fs::write(
            &legacy_vllm_path,
            serde_json::json!({
                "base_url": "http://127.0.0.1:9000/v1",
                "model": "local/legacy-vision",
                "system_prompt": "return legacy tags",
                "language": "en",
                "max_tags": 60,
                "max_length": 400,
                "tag_mode": "append",
                "concurrency": 3,
                "verify_danbooru": true,
                "reference_existing": true,
                "api_key": "must-never-enter-settings"
            })
            .to_string(),
        )
        .unwrap();

        migrate_legacy_settings(&settings_path, &legacy_path, &legacy_vllm_path).unwrap();
        let settings = load_settings(&settings_path).unwrap();
        let stored = std::fs::read_to_string(settings_path).unwrap();

        assert_eq!(settings.vllm_base_url, "http://127.0.0.1:9000/v1");
        assert_eq!(settings.vllm_model, "local/legacy-vision");
        assert_eq!(settings.vllm_system_prompt, "return legacy tags");
        assert_eq!(
            settings.vllm_tag_mode,
            crate::services::vllm::TagWriteMode::Append
        );
        assert_eq!(settings.vllm_concurrency, 3);
        assert_eq!(
            settings.vllm_language,
            crate::services::vllm::VllmLanguage::English
        );
        assert_eq!(settings.vllm_max_tags, 60);
        assert_eq!(settings.vllm_max_length, 400);
        assert!(settings.vllm_verify_danbooru);
        assert!(settings.vllm_reference_existing);
        assert!(!stored.contains("must-never-enter-settings"));
    }

    #[test]
    fn legacy_migration_never_overwrites_existing_non_default_settings() {
        let directory = tempfile::tempdir().unwrap();
        let settings_path = directory.path().join("app_settings.json");
        let legacy_path = directory.path().join("config.json");
        let legacy_vllm_path = directory.path().join("vllm_config.json");
        let existing = StoredSettings {
            version: 3,
            danbooru_username: "current-user".to_string(),
            download_concurrency: 12,
            vllm_model: "current/model".to_string(),
            ..StoredSettings::default()
        };
        save_settings(&settings_path, &existing).unwrap();
        std::fs::write(
            &legacy_path,
            serde_json::json!({"download": {"username": "legacy-user", "concurrency": 16}})
                .to_string(),
        )
        .unwrap();
        std::fs::write(
            &legacy_vllm_path,
            serde_json::json!({"model": "legacy/model"}).to_string(),
        )
        .unwrap();

        migrate_legacy_settings(&settings_path, &legacy_path, &legacy_vllm_path).unwrap();
        let migrated = load_settings(&settings_path).unwrap();

        assert_eq!(migrated.version, 6);
        assert_eq!(migrated.danbooru_username, "current-user");
        assert_eq!(migrated.download_concurrency, 12);
        assert_eq!(migrated.vllm_model, "current/model");
    }

    #[test]
    fn legacy_proxy_host_and_port_become_a_valid_credential_free_url() {
        let directory = tempfile::tempdir().unwrap();
        let settings_path = directory.path().join("app_settings.json");
        let legacy_path = directory.path().join("config.json");
        let legacy_vllm_path = directory.path().join("vllm_config.json");
        std::fs::write(
            &legacy_path,
            serde_json::json!({
                "download": {"proxy": "127.0.0.1", "proxy_port": "7890"}
            })
            .to_string(),
        )
        .unwrap();

        migrate_legacy_settings(&settings_path, &legacy_path, &legacy_vllm_path).unwrap();
        let migrated = load_settings(&settings_path).unwrap();

        assert_eq!(migrated.proxy_url.as_deref(), Some("http://127.0.0.1:7890"));
    }

    #[test]
    fn invalid_legacy_network_and_concurrency_values_are_skipped() {
        let directory = tempfile::tempdir().unwrap();
        let settings_path = directory.path().join("app_settings.json");
        let legacy_path = directory.path().join("config.json");
        let legacy_vllm_path = directory.path().join("vllm_config.json");
        std::fs::write(
            &legacy_path,
            serde_json::json!({
                "download": {
                    "concurrency": 99,
                    "proxy": "http://user:password@127.0.0.1:8080"
                }
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            &legacy_vllm_path,
            serde_json::json!({
                "base_url": "http://remote.example.test/v1",
                "concurrency": 0
            })
            .to_string(),
        )
        .unwrap();

        migrate_legacy_settings(&settings_path, &legacy_path, &legacy_vllm_path).unwrap();
        let migrated = load_settings(&settings_path).unwrap();
        let defaults = StoredSettings::default();

        assert_eq!(migrated.download_concurrency, defaults.download_concurrency);
        assert_eq!(migrated.proxy_url, None);
        assert_eq!(migrated.vllm_base_url, defaults.vllm_base_url);
        assert_eq!(migrated.vllm_concurrency, defaults.vllm_concurrency);
    }

    #[test]
    fn stored_and_public_config_preserve_vllm_execution_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("app_settings.json");
        let expected = StoredSettings {
            vllm_model: "local/vision-model".to_string(),
            vllm_system_prompt: "return exact tags".to_string(),
            vllm_tag_mode: crate::services::vllm::TagWriteMode::Append,
            ..StoredSettings::default()
        };

        save_settings(&path, &expected).unwrap();
        let loaded = load_settings(&path).unwrap();
        let public = PublicConfig::from_settings(&loaded, false, true);

        assert_eq!(loaded, expected);
        assert_eq!(public.vllm_model, "local/vision-model");
        assert_eq!(public.vllm_system_prompt, "return exact tags");
        assert_eq!(
            public.vllm_tag_mode,
            crate::services::vllm::TagWriteMode::Append
        );
    }

    #[test]
    fn legacy_secret_is_removed_after_verified_vault_migration() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        std::fs::write(&path, r#"{"download":{"api_key":"legacy-secret"}}"#).unwrap();
        let manager = SecretManager::with_vault(Arc::new(TestVault::default()));

        let outcome = migrate_legacy_secret(
            &path,
            &["download", "api_key"],
            SecretKind::Danbooru,
            &manager,
        )
        .unwrap();

        assert_eq!(outcome, LegacySecretMigration::Migrated);
        let stored: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(stored["download"]["api_key"], "");
        assert_eq!(
            manager.get_for_internal_use(SecretKind::Danbooru).unwrap(),
            Some("legacy-secret".to_string())
        );
    }
}

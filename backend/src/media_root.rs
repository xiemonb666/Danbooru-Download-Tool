use std::path::{Component, Path, PathBuf};

#[cfg(windows)]
pub(crate) fn metadata_is_link_or_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
pub(crate) fn metadata_is_link_or_reparse_point(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

pub(crate) fn validate_root_path(path: &Path) -> Result<(), MediaPathError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| MediaPathError::Io)?;
    if !metadata.is_dir() || metadata_is_link_or_reparse_point(&metadata) {
        return Err(MediaPathError::Io);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaPathError {
    InvalidWindowsPath,
    InvalidRelativePath,
    Io,
    NotRegularFile,
    OutsideRoot,
}

pub fn normalize_windows_path(raw: &str) -> Result<String, MediaPathError> {
    if raw.contains('\0') || raw.is_empty() || raw.starts_with("//") {
        return Err(MediaPathError::InvalidWindowsPath);
    }
    let normalized = raw.replace('/', r"\");
    let drive_path = if let Some(path) = normalized.strip_prefix(r"\\?\") {
        path
    } else {
        if normalized.starts_with(r"\\") {
            return Err(MediaPathError::InvalidWindowsPath);
        }
        normalized.as_str()
    };
    let bytes = drive_path.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' || bytes[2] != b'\\' {
        return Err(MediaPathError::InvalidWindowsPath);
    }
    if drive_path[3..]
        .split('\\')
        .any(|component| matches!(component, "." | "..") || component.contains(':'))
    {
        return Err(MediaPathError::InvalidWindowsPath);
    }
    let mut result = drive_path.to_string();
    result.replace_range(..1, &drive_path[..1].to_ascii_uppercase());
    Ok(result)
}

#[derive(Debug, Clone)]
pub struct MediaRoot {
    pub id: String,
    canonical_path: PathBuf,
}

impl MediaRoot {
    pub fn new(id: impl Into<String>, path: impl AsRef<Path>) -> Result<Self, MediaPathError> {
        let path = path.as_ref();
        validate_root_path(path)?;
        let canonical_path = std::fs::canonicalize(path).map_err(|_| MediaPathError::Io)?;
        Ok(Self {
            id: id.into(),
            canonical_path,
        })
    }

    pub fn resolve_existing_file(&self, relative: &Path) -> Result<PathBuf, MediaPathError> {
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(MediaPathError::InvalidRelativePath);
        }
        let candidate = std::fs::canonicalize(self.canonical_path.join(relative))
            .map_err(|_| MediaPathError::Io)?;
        if !candidate.starts_with(&self.canonical_path) {
            return Err(MediaPathError::OutsideRoot);
        }
        let metadata = std::fs::metadata(&candidate).map_err(|_| MediaPathError::Io)?;
        if !metadata.file_type().is_file() {
            return Err(MediaPathError::NotRegularFile);
        }
        Ok(candidate)
    }

    #[cfg(test)]
    pub fn resolve_for_write(&self, relative: &Path) -> Result<PathBuf, MediaPathError> {
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(MediaPathError::InvalidRelativePath);
        }
        let candidate = self.canonical_path.join(relative);
        let parent = candidate
            .parent()
            .ok_or(MediaPathError::InvalidRelativePath)?;
        let canonical_parent = std::fs::canonicalize(parent).map_err(|_| MediaPathError::Io)?;
        if !canonical_parent.starts_with(&self.canonical_path) {
            return Err(MediaPathError::OutsideRoot);
        }
        let file_name = candidate
            .file_name()
            .ok_or(MediaPathError::InvalidRelativePath)?;
        Ok(canonical_parent.join(file_name))
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_windows_path, MediaRoot};
    use std::fs;
    use std::path::Path;

    #[test]
    fn normalizes_windows_extended_drive_path() {
        assert_eq!(
            normalize_windows_path(r"\\?\C:\Media\Danbooru").unwrap(),
            r"C:\Media\Danbooru"
        );
    }

    #[test]
    fn rejects_unc_and_device_namespaces() {
        for path in [
            r"\\server\share\file.jpg",
            r"//server/share/file.jpg",
            r"//?/C:/Media",
            r"\\?\UNC\server\share",
            r"\\.\PhysicalDrive0",
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\",
        ] {
            assert_eq!(
                normalize_windows_path(path),
                Err(super::MediaPathError::InvalidWindowsPath)
            );
        }
    }

    #[test]
    fn accepts_only_drive_absolute_windows_paths() {
        for path in [".", "Media", r"C:relative", r"\rooted", "/rooted"] {
            assert_eq!(
                normalize_windows_path(path),
                Err(super::MediaPathError::InvalidWindowsPath)
            );
        }
        assert_eq!(normalize_windows_path("c:/Media").unwrap(), r"C:\Media");
    }

    #[test]
    fn rejects_parent_directory_traversal() {
        let root_dir =
            std::env::temp_dir().join(format!("danbooru-media-root-{}", std::process::id()));
        fs::create_dir_all(&root_dir).unwrap();
        let root = MediaRoot::new("root-1", &root_dir).unwrap();

        let result = root.resolve_existing_file(Path::new("../secret.txt"));

        fs::remove_dir_all(&root_dir).unwrap();
        assert_eq!(result, Err(super::MediaPathError::InvalidRelativePath));
    }

    #[test]
    fn only_resolves_regular_files() {
        let root_dir =
            std::env::temp_dir().join(format!("danbooru-media-root-file-{}", std::process::id()));
        fs::create_dir_all(root_dir.join("nested")).unwrap();
        let root = MediaRoot::new("root-1", &root_dir).unwrap();

        let result = root.resolve_existing_file(Path::new("nested"));

        fs::remove_dir_all(&root_dir).unwrap();
        assert_eq!(result, Err(super::MediaPathError::NotRegularFile));
    }

    #[test]
    fn rejects_a_root_path_that_is_itself_a_link() {
        let base = std::env::temp_dir().join(format!(
            "danbooru-media-root-self-link-{}",
            std::process::id()
        ));
        let real_root = base.join("real");
        let linked_root = base.join("linked");
        fs::create_dir_all(&real_root).unwrap();
        #[cfg(windows)]
        assert!(std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(&linked_root)
            .arg(&real_root)
            .status()
            .unwrap()
            .success());
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_root, &linked_root).unwrap();

        let result = MediaRoot::new("root-1", &linked_root);

        #[cfg(windows)]
        fs::remove_dir(&linked_root).unwrap();
        #[cfg(unix)]
        fs::remove_file(&linked_root).unwrap();
        fs::remove_dir_all(&base).unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn rejects_intermediate_symlink_that_escapes_root() {
        let base =
            std::env::temp_dir().join(format!("danbooru-media-root-link-{}", std::process::id()));
        let root_dir = base.join("root");
        let outside_dir = base.join("outside");
        fs::create_dir_all(&root_dir).unwrap();
        fs::create_dir_all(&outside_dir).unwrap();
        fs::write(outside_dir.join("secret.jpg"), b"secret").unwrap();
        #[cfg(windows)]
        assert!(std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(root_dir.join("escape"))
            .arg(&outside_dir)
            .status()
            .unwrap()
            .success());
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside_dir, root_dir.join("escape")).unwrap();
        let root = MediaRoot::new("root-1", &root_dir).unwrap();

        let result = root.resolve_existing_file(Path::new("escape/secret.jpg"));

        fs::remove_dir_all(&base).unwrap();
        assert_eq!(result, Err(super::MediaPathError::OutsideRoot));
    }

    #[test]
    fn rejects_traversal_for_new_files() {
        let root_dir =
            std::env::temp_dir().join(format!("danbooru-media-root-write-{}", std::process::id()));
        fs::create_dir_all(&root_dir).unwrap();
        let root = MediaRoot::new("root-1", &root_dir).unwrap();

        let result = root.resolve_for_write(Path::new("../escape.part"));

        fs::remove_dir_all(&root_dir).unwrap();
        assert_eq!(result, Err(super::MediaPathError::InvalidRelativePath));
    }

    #[test]
    fn rejects_writes_through_directory_link_outside_root() {
        let base = std::env::temp_dir().join(format!(
            "danbooru-media-root-write-link-{}",
            std::process::id()
        ));
        let root_dir = base.join("root");
        let outside_dir = base.join("outside");
        fs::create_dir_all(&root_dir).unwrap();
        fs::create_dir_all(&outside_dir).unwrap();
        #[cfg(windows)]
        assert!(std::process::Command::new("cmd.exe")
            .args(["/C", "mklink", "/J"])
            .arg(root_dir.join("escape"))
            .arg(&outside_dir)
            .status()
            .unwrap()
            .success());
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside_dir, root_dir.join("escape")).unwrap();
        let root = MediaRoot::new("root-1", &root_dir).unwrap();

        let result = root.resolve_for_write(Path::new("escape/new.part"));

        fs::remove_dir_all(&base).unwrap();
        assert_eq!(result, Err(super::MediaPathError::OutsideRoot));
    }
}

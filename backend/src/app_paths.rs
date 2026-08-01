use std::net::IpAddr;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub host: IpAddr,
    pub port: u16,
    pub data_dir: PathBuf,
    pub static_dir: PathBuf,
}

impl AppPaths {
    pub fn from_env() -> Result<Self, String> {
        let host = std::env::var("HOST").ok();
        let port = std::env::var("PORT").ok();
        let mut paths = Self::from_values(host.as_deref(), port.as_deref(), None, None)?;
        if let Some(data_dir) = std::env::var_os("DATA_DIR") {
            paths.data_dir = PathBuf::from(data_dir);
        }
        if let Some(static_dir) = std::env::var_os("STATIC_DIR") {
            paths.static_dir = PathBuf::from(static_dir);
        }
        if !paths.data_dir.is_absolute() || !paths.static_dir.is_absolute() {
            return Err("DATA_DIR and STATIC_DIR must be absolute paths".to_string());
        }
        Ok(paths)
    }

    pub fn from_values(
        host: Option<&str>,
        port: Option<&str>,
        data_dir: Option<&str>,
        static_dir: Option<&str>,
    ) -> Result<Self, String> {
        let manifest_dir = option_env!("CARGO_MANIFEST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|path| path.parent().map(PathBuf::from))
                    .unwrap_or_else(|| PathBuf::from("."))
            });
        let project_dir = manifest_dir
            .parent()
            .map(PathBuf::from)
            .unwrap_or(manifest_dir);

        let host: IpAddr = host
            .unwrap_or("127.0.0.1")
            .parse()
            .map_err(|_| "HOST must be a valid IP address".to_string())?;
        if !host.is_loopback() {
            return Err("HOST must resolve to a loopback address".to_string());
        }

        let port = port
            .unwrap_or("8888")
            .parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| "PORT must be a number between 1 and 65535".to_string())?;
        let data_dir = data_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| project_dir.clone());
        let static_dir = static_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| project_dir.join("frontend").join("dist"));
        if !data_dir.is_absolute() || !static_dir.is_absolute() {
            return Err("DATA_DIR and STATIC_DIR must be absolute paths".to_string());
        }

        Ok(Self {
            host,
            port,
            data_dir,
            static_dir,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::AppPaths;

    #[test]
    fn defaults_are_loopback_and_independent_from_working_directory() {
        let paths = AppPaths::from_values(None, None, None, None).unwrap();

        assert_eq!(paths.host.to_string(), "127.0.0.1");
        assert_eq!(paths.port, 8888);
        assert!(paths.data_dir.is_absolute());
        assert!(paths.static_dir.is_absolute());
    }

    #[test]
    fn rejects_non_loopback_host() {
        let error = AppPaths::from_values(Some("0.0.0.0"), None, None, None).unwrap_err();

        assert_eq!(error, "HOST must resolve to a loopback address");
    }

    #[test]
    fn rejects_zero_port_and_relative_runtime_directories() {
        assert!(AppPaths::from_values(None, Some("0"), None, None).is_err());
        assert!(AppPaths::from_values(None, None, Some("relative-data"), None).is_err());
        assert!(AppPaths::from_values(None, None, None, Some("relative-static")).is_err());
    }

    #[test]
    fn reads_runtime_environment_overrides() {
        let data_dir = std::env::temp_dir().join("danbooru-data");
        let static_dir = std::env::temp_dir().join("danbooru-static");
        std::env::set_var("HOST", "::1");
        std::env::set_var("PORT", "9012");
        std::env::set_var("DATA_DIR", &data_dir);
        std::env::set_var("STATIC_DIR", &static_dir);

        let paths = AppPaths::from_env().unwrap();

        std::env::remove_var("HOST");
        std::env::remove_var("PORT");
        std::env::remove_var("DATA_DIR");
        std::env::remove_var("STATIC_DIR");
        assert_eq!(paths.host.to_string(), "::1");
        assert_eq!(paths.port, 9012);
        assert_eq!(paths.data_dir, data_dir);
        assert_eq!(paths.static_dir, static_dir);
    }
}

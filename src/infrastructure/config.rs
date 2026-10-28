use crate::shared::constants::*;
use crate::shared::errors::ConfigError;
use crate::shared::models::*;
use crate::shared::traits::ConfigRepository;
use crate::shared::validation::validate_config;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

pub struct ConfigStorage {
    path: std::path::PathBuf,
    lock: parking_lot::Mutex<()>,
}

impl ConfigStorage {
    pub fn new(path: std::path::PathBuf) -> Self {
        Self {
            path,
            lock: parking_lot::Mutex::new(()),
        }
    }
}

impl ConfigStorage {
    /// Loads settings; an unreadable or invalid file is kept as `config.invalid-<time>.toml`
    /// and defaults are used, so a bad edit never prevents the app from starting.
    pub fn load_or_recover(&self) -> Result<(AppConfig, Option<String>), ConfigError> {
        match self.load() {
            Ok(mut config) => {
                // Budgets written by earlier versions were too small for real project folders.
                if config.scanner.size_entry_budget == LEGACY_SIZE_ENTRY_BUDGET
                    && config.scanner.size_time_budget_seconds == LEGACY_SIZE_TIME_BUDGET_SECS
                {
                    config.scanner.size_entry_budget = SIZE_ENTRY_BUDGET;
                    config.scanner.size_time_budget_seconds = SIZE_TIME_BUDGET_SECS;
                    let _ = self.save(&config);
                }
                Ok((config, None))
            }
            Err(ConfigError::Write) => Err(ConfigError::Write),
            Err(error) => {
                tracing::warn!(%error, "settings file rejected; using defaults");
                let backup = self.path.with_file_name(format!(
                    "config.invalid-{}.toml",
                    chrono::Utc::now().timestamp()
                ));
                fs::rename(&self.path, &backup).map_err(|_| ConfigError::Write)?;
                let config = AppConfig::default();
                self.save(&config)?;
                Ok((
                    config,
                    Some(format!(
                        "Settings were invalid and have been reset. The old file was kept as {}",
                        backup.display()
                    )),
                ))
            }
        }
    }
}

impl ConfigRepository for ConfigStorage {
    fn load(&self) -> Result<AppConfig, ConfigError> {
        if !self.path.exists() {
            let config = AppConfig::default();
            self.save(&config)?;
            return Ok(config);
        }
        let text = fs::read_to_string(&self.path).map_err(|_| ConfigError::Read)?;
        if text.len() > MAX_DESCRIPTION_BYTES {
            return Err(ConfigError::Parse);
        }
        let config: AppConfig = toml::from_str(&text).map_err(|_| ConfigError::Parse)?;
        validate_config(&config)?;
        Ok(config)
    }

    fn save(&self, config: &AppConfig) -> Result<(), ConfigError> {
        validate_config(config)?;
        let _lock = self.lock.lock();
        let bytes = toml::to_string_pretty(config).map_err(|_| ConfigError::Write)?;
        atomic_write(&self.path, bytes.as_bytes()).map_err(|_| ConfigError::Write)
    }
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let parent = path.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".filemofm-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        // Best effort: FAT/exFAT and some network filesystems have no Unix permissions.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        if let Ok(directory) = fs::File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub fn runtime_paths() -> Result<RuntimePaths, ConfigError> {
    let executable = std::env::current_exe().map_err(|_| ConfigError::Read)?;
    let root = crate::shared::utils::canonicalize(executable.parent().ok_or(ConfigError::Read)?)
        .map_err(|_| ConfigError::Read)?;
    let data = root.join("data");
    let paths = RuntimePaths {
        config: root.join(CONFIG_NAME),
        database: data.join(DATABASE_NAME),
        data,
        logs: root.join("logs"),
        models: root.join("models"),
        runtime: root.join("runtime"),
        trash: root.join("trash"),
        root,
    };
    for directory in [
        &paths.data,
        &paths.logs,
        &paths.models,
        &paths.runtime,
        &paths.trash,
    ] {
        fs::create_dir_all(directory).map_err(|_| ConfigError::Write)?;
        let metadata = fs::symlink_metadata(directory).map_err(|_| ConfigError::Write)?;
        if metadata.file_type().is_symlink() {
            return Err(ConfigError::Write);
        }
        // Best effort, so the portable folder also runs from FAT/exFAT USB drives on Linux.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(directory, fs::Permissions::from_mode(0o700));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const REPARSE_POINT: u32 = 0x400;
            if metadata.file_attributes() & REPARSE_POINT != 0 {
                return Err(ConfigError::Write);
            }
        }
    }
    Ok(paths)
}

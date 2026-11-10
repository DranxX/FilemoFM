use crate::shared::constants::MAX_PATH_BYTES;
use crate::shared::errors::ScanError;
use crate::shared::traits::PathValidator;
use std::path::{Path, PathBuf};

pub struct FsPathValidator {
    excluded_paths: parking_lot::RwLock<Vec<PathBuf>>,
    protected: Vec<PathBuf>,
}
impl FsPathValidator {
    pub fn new(paths: Vec<String>, protected: Vec<PathBuf>) -> Self {
        Self {
            protected,
            excluded_paths: parking_lot::RwLock::new(
                paths
                    .into_iter()
                    .map(|path| {
                        crate::shared::utils::parse_config_path(&path)
                            .map(|path| crate::shared::utils::canonicalize(&path).unwrap_or(path))
                            .unwrap_or_else(|_| PathBuf::from(path))
                    })
                    .collect(),
            ),
        }
    }
}
impl PathValidator for FsPathValidator {
    fn set_exclusions(&self, paths: Vec<String>) {
        *self.excluded_paths.write() = paths
            .into_iter()
            .map(|path| {
                crate::shared::utils::parse_config_path(&path)
                    .map(|path| crate::shared::utils::canonicalize(&path).unwrap_or(path))
                    .unwrap_or_else(|_| PathBuf::from(path))
            })
            .collect();
    }
    fn validate(&self, path: &str) -> Result<PathBuf, ScanError> {
        let path = crate::shared::utils::normalize_input(path);
        if path.is_empty() || path.len() > MAX_PATH_BYTES || path.chars().any(char::is_control) {
            return Err(ScanError::InvalidPath);
        }
        #[cfg(windows)]
        {
            let bytes = path.as_bytes();
            // Device namespace paths and drive-relative paths (`C:folder`) are never guessed.
            if path.starts_with("\\\\.\\")
                || (bytes.get(1) == Some(&b':') && bytes.get(2) != Some(&b'\\'))
            {
                return Err(ScanError::InvalidPath);
            }
        }
        let raw =
            crate::shared::utils::parse_config_path(&path).map_err(|_| ScanError::InvalidPath)?;
        let canonical = crate::shared::utils::canonicalize(&raw).map_err(|error| {
            tracing::warn!(kind = ?error.kind(), "path resolution failed");
            if error.kind() == std::io::ErrorKind::NotFound {
                ScanError::PathNotFound
            } else {
                ScanError::PermissionDenied
            }
        })?;
        if self.is_excluded(&canonical) {
            return Err(ScanError::OutOfScope);
        }
        Ok(canonical)
    }
    fn is_excluded(&self, path: &Path) -> bool {
        self.protected
            .iter()
            .chain(self.excluded_paths.read().iter())
            .any(|excluded| path.starts_with(excluded))
    }
}

pub fn open_file_default(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err("File is unavailable".into());
    }
    open::that(path).map_err(|_| "File could not be opened".into())
}

/// Drives, mounts and WSL distributions as `(label, root)`, labelled like Explorer.
pub fn get_system_drives() -> Vec<(String, PathBuf)> {
    #[cfg(windows)]
    {
        windows_drives()
    }
    #[cfg(not(windows))]
    {
        unix_mounts()
    }
}

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

#[cfg(windows)]
fn windows_drives() -> Vec<(String, PathBuf)> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    };
    use windows_sys::Win32::System::WindowsProgramming::{
        DRIVE_CDROM, DRIVE_REMOTE, DRIVE_REMOVABLE,
    };
    let mask = unsafe { GetLogicalDrives() };
    let mut drives = Vec::new();
    for index in 0..26u32 {
        if mask & (1 << index) == 0 {
            continue;
        }
        let letter = (b'A' + index as u8) as char;
        let root = format!("{letter}:\\");
        let root_wide = wide(&root);
        let kind = unsafe { GetDriveTypeW(root_wide.as_ptr()) };
        // Remote and optical drives can block for seconds when disconnected or empty.
        let volume = if kind == DRIVE_REMOTE || kind == DRIVE_CDROM {
            String::new()
        } else {
            let mut name = [0u16; 261];
            let ok = unsafe {
                GetVolumeInformationW(
                    root_wide.as_ptr(),
                    name.as_mut_ptr(),
                    name.len() as u32,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                )
            } != 0;
            if ok {
                let end = name
                    .iter()
                    .position(|unit| *unit == 0)
                    .unwrap_or(name.len());
                String::from_utf16_lossy(&name[..end])
            } else {
                String::new()
            }
        };
        let fallback = match kind {
            DRIVE_REMOTE => "Network Drive",
            DRIVE_REMOVABLE => "Removable Disk",
            DRIVE_CDROM => "CD Drive",
            _ => "Local Disk",
        };
        let name = if volume.trim().is_empty() {
            fallback.to_string()
        } else {
            volume.trim().to_string()
        };
        drives.push((format!("{name} ({letter}:)"), PathBuf::from(root)));
    }
    drives.extend(wsl_distributions());
    drives
}

/// Installed WSL distributions from the registry; reading them does not start WSL.
#[cfg(windows)]
fn wsl_distributions() -> Vec<(String, PathBuf)> {
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW,
        RegOpenKeyExW,
    };
    let mut distributions = Vec::new();
    let path = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Lxss");
    let mut key: HKEY = std::ptr::null_mut();
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_READ, &mut key) } != 0 {
        return distributions;
    }
    let value = wide("DistributionName");
    for index in 0..64 {
        let mut name = [0u16; 256];
        let mut length = name.len() as u32;
        let status = unsafe {
            RegEnumKeyExW(
                key,
                index,
                name.as_mut_ptr(),
                &mut length,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            break;
        }
        let mut data = [0u16; 256];
        let mut bytes = (data.len() * 2) as u32;
        let ok = unsafe {
            RegGetValueW(
                key,
                name.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                data.as_mut_ptr().cast(),
                &mut bytes,
            )
        } == 0;
        if !ok {
            continue;
        }
        let end = data
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(data.len());
        let distribution = String::from_utf16_lossy(&data[..end]);
        if !distribution.is_empty()
            && distribution
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "-_.".contains(ch))
        {
            distributions.push((
                format!("{distribution} (WSL)"),
                PathBuf::from(format!("\\\\wsl.localhost\\{distribution}\\")),
            ));
        }
    }
    unsafe { RegCloseKey(key) };
    distributions
}

#[cfg(not(windows))]
fn unix_mounts() -> Vec<(String, PathBuf)> {
    let is_wsl = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .is_ok_and(|release| release.to_lowercase().contains("microsoft"));
    let mut roots = vec![("File System".to_string(), PathBuf::from("/"))];
    let Ok(mounts) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return roots;
    };
    // Pseudo/in-memory filesystems are not user storage; WSLg mounts its own
    // system distro under /mnt/wslg, which is not the user's data either.
    const PSEUDO: &[&str] = &[
        "tmpfs",
        "devtmpfs",
        "overlay",
        "proc",
        "sysfs",
        "cgroup",
        "cgroup2",
        "devpts",
        "mqueue",
        "binfmt_misc",
        "autofs",
        "fusectl",
        "tracefs",
        "debugfs",
        "securityfs",
        "pstore",
        "bpf",
        "hugetlbfs",
        "configfs",
        "nsfs",
        "rpc_pipefs",
        "squashfs",
        "efivarfs",
    ];
    for line in mounts.lines() {
        let mut fields = line.split_whitespace();
        let Some(mount) = fields.nth(4) else {
            continue;
        };
        let fs_type = fields.skip_while(|field| *field != "-").nth(1);
        if fs_type.is_some_and(|kind| PSEUDO.contains(&kind)) {
            continue;
        }
        let mount = mount
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\134", "\\");
        let path = PathBuf::from(mount);
        if !["/mnt", "/media", "/run/media"]
            .iter()
            .any(|prefix| path.starts_with(prefix))
            || path.starts_with("/mnt/wslg")
            || !path.is_dir()
        {
            continue;
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let label = if is_wsl
            && path.parent() == Some(Path::new("/mnt"))
            && name.len() == 1
            && name.chars().all(|ch| ch.is_ascii_alphabetic())
        {
            format!("Windows ({}:)", name.to_uppercase())
        } else {
            name
        };
        roots.push((label, path));
    }
    roots.sort_by(|a, b| a.1.cmp(&b.1));
    roots.dedup_by(|a, b| a.1 == b.1);
    roots
}

pub fn get_user_directories() -> Vec<(String, PathBuf)> {
    let Some(user) = directories::UserDirs::new() else {
        return Vec::new();
    };
    let mut paths = vec![("Home".into(), user.home_dir().to_path_buf())];
    for (name, path) in [
        ("Documents", user.document_dir()),
        ("Downloads", user.download_dir()),
        ("Desktop", user.desktop_dir()),
        ("Pictures", user.picture_dir()),
        ("Music", user.audio_dir()),
        ("Videos", user.video_dir()),
    ] {
        if let Some(path) = path {
            paths.push((name.into(), path.into()));
        }
    }
    paths
}

pub fn is_hidden(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        path.symlink_metadata()
            .is_ok_and(|metadata| metadata.file_attributes() & 2 != 0)
    }
    #[cfg(not(windows))]
    {
        path.file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'))
    }
}

pub fn open_terminal(path: &Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err("Folder is unavailable".into());
    }
    #[cfg(windows)]
    let candidates: &[(&str, &[&str])] = &[("wt.exe", &["-d", "."]), ("cmd.exe", &["/K"])];
    #[cfg(not(windows))]
    let candidates: &[(&str, &[&str])] = &[
        ("x-terminal-emulator", &[]),
        ("gnome-terminal", &[]),
        ("konsole", &[]),
        ("xfce4-terminal", &[]),
        ("kitty", &[]),
        ("alacritty", &[]),
        ("xterm", &[]),
    ];
    for (program, arguments) in candidates {
        let mut command = std::process::Command::new(program);
        command.args(*arguments).current_dir(path);
        if command.spawn().is_ok() {
            return Ok(());
        }
    }
    Err("Terminal could not be opened".into())
}

pub struct SystemFileInspector;
impl crate::shared::traits::FileInspector for SystemFileInspector {
    fn inspect(&self, path: &Path) -> Option<crate::shared::models::FileItem> {
        // A link to a folder behaves like a folder for browsing; the link itself is what
        // file operations act on.
        let link = path.symlink_metadata().ok()?;
        let metadata = if link.file_type().is_symlink() {
            path.metadata().unwrap_or(link)
        } else {
            link
        };
        let timestamp = |time: Result<std::time::SystemTime, std::io::Error>| {
            time.ok()
                .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |value| value.as_secs().min(i64::MAX as u64) as i64)
        };
        Some(crate::shared::models::FileItem {
            id: uuid::Uuid::new_v4().to_string(),
            path: path.into(),
            name: path.file_name()?.to_string_lossy().into_owned(),
            extension: path
                .extension()
                .map(|value| value.to_string_lossy().to_lowercase())
                .unwrap_or_default(),
            size: if metadata.is_dir() { 0 } else { metadata.len() },
            is_directory: metadata.is_dir(),
            created_at: timestamp(metadata.created()),
            modified_at: timestamp(metadata.modified()),
            updated_at: chrono::Utc::now().timestamp(),
            revision: 1,
            native_id: crate::shared::utils::file_identity(path),
            is_hidden: is_hidden(path),
            is_missing: false,
            tags: Vec::new(),
            description: String::new(),
        })
    }
}

pub struct SystemPlatform {
    paths: crate::shared::models::RuntimePaths,
}
impl SystemPlatform {
    pub fn new(paths: crate::shared::models::RuntimePaths) -> Self {
        Self { paths }
    }
}
impl crate::shared::traits::PlatformIntegration for SystemPlatform {
    fn locations(&self) -> Vec<(String, PathBuf)> {
        get_user_directories()
    }
    fn drives(&self) -> Vec<(String, PathBuf)> {
        get_system_drives()
    }
    fn open(&self, path: &Path) -> Result<(), String> {
        open_file_default(path)
    }
    fn terminal(&self, path: &Path) -> Result<(), String> {
        open_terminal(path)
    }
    fn register_runtime(&self, path: &Path, kind: &str, version: &str) -> Result<(), String> {
        super::runtime::register_runtime(&self.paths, path, kind, version)
    }
}

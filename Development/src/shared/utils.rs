use pulldown_cmark::{Event, Parser};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

pub fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

pub fn path_key(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect()
    }
}

pub fn path_from_key(bytes: Vec<u8>) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(bytes).into()
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        let units: Vec<_> = bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        std::ffi::OsString::from_wide(&units).into()
    }
}

pub fn natural_compare(left: &str, right: &str) -> Ordering {
    let left = left.to_lowercase();
    let right = right.to_lowercase();
    let mut a = left.chars().peekable();
    let mut b = right.chars().peekable();
    while let (Some(x), Some(y)) = (a.peek().copied(), b.peek().copied()) {
        if x.is_ascii_digit() && y.is_ascii_digit() {
            let x: String = std::iter::from_fn(|| a.next_if(char::is_ascii_digit)).collect();
            let y: String = std::iter::from_fn(|| b.next_if(char::is_ascii_digit)).collect();
            let x_number = x.trim_start_matches('0');
            let y_number = y.trim_start_matches('0');
            let order = x_number
                .len()
                .cmp(&y_number.len())
                .then_with(|| x_number.cmp(y_number))
                .then_with(|| x.len().cmp(&y.len()));
            if order != Ordering::Equal {
                return order;
            }
            continue;
        }
        let order = x.cmp(&y);
        if order != Ordering::Equal {
            return order;
        }
        a.next();
        b.next();
    }
    a.next().is_some().cmp(&b.next().is_some())
}

pub fn reference_nodes(source: &str) -> Vec<(std::ops::Range<usize>, String, Option<String>)> {
    let mut nodes = Vec::new();
    let regions = markdown_text_regions(source);
    for (start, _) in source.match_indices("@{file:") {
        let Some(end) = source[start..].find('}') else {
            continue;
        };
        let end = start + end + 1;
        if !is_markdown_text(source, &(start..end), &regions) {
            continue;
        }
        let content = &source[start + 7..end - 1];
        let (id, label) = content
            .split_once('|')
            .map_or((content, None), |(id, label)| (id, Some(label.to_string())));
        if uuid::Uuid::parse_str(id).is_ok()
            && label.as_ref().is_none_or(|s| {
                s.len() <= crate::shared::constants::MAX_REFERENCE_LABEL_BYTES
                    && !s.chars().any(char::is_control)
            })
        {
            nodes.push((start..end, id.to_string(), label));
        }
    }
    nodes
}

type MarkdownTextRegions = (Vec<std::ops::Range<usize>>, Vec<std::ops::Range<usize>>);

fn markdown_text_regions(source: &str) -> MarkdownTextRegions {
    let mut text = Vec::new();
    let mut protected = Vec::new();
    let mut code_start = None;
    for (event, range) in Parser::new(source).into_offset_iter() {
        match event {
            Event::Start(pulldown_cmark::Tag::CodeBlock(_)) => code_start = Some(range.start),
            Event::End(pulldown_cmark::TagEnd::CodeBlock) => {
                if let Some(start) = code_start.take() {
                    protected.push(start..range.end);
                }
            }
            Event::Code(_) | Event::Html(_) | Event::InlineHtml(_) => protected.push(range),
            Event::Text(_) if code_start.is_none() => text.push(range),
            _ => {}
        }
    }
    if let Some(start) = code_start {
        protected.push(start..source.len());
    }
    (text, protected)
}

fn is_markdown_text(
    source: &str,
    range: &std::ops::Range<usize>,
    regions: &MarkdownTextRegions,
) -> bool {
    regions.0.iter().any(|text| text.contains(&range.start))
        && !regions
            .1
            .iter()
            .any(|protected| protected.start < range.end && range.start < protected.end)
        && !source[range.clone()].contains('\n')
        && source[..range.start]
            .chars()
            .rev()
            .take_while(|ch| *ch == '\\')
            .count()
            % 2
            == 0
}

pub fn validate_component(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Enter a name".into());
    }
    if matches!(name, "." | "..") {
        return Err("“.” and “..” are reserved names".into());
    }
    if name.len() > 255 {
        return Err("Name is too long".into());
    }
    if name.contains(['/', '\0']) {
        return Err("Names cannot contain /".into());
    }
    if name.chars().any(char::is_control) {
        return Err("Name contains control characters".into());
    }
    #[cfg(windows)]
    {
        if name.encode_utf16().count() > 255 {
            return Err("Name is too long".into());
        }
        if name.chars().any(|ch| "\\<>:\"|?*".contains(ch)) {
            return Err(
                "Name contains characters Windows does not allow: \\ / : * ? \" < > |".into(),
            );
        }
        if name.ends_with([' ', '.']) {
            return Err("Windows names cannot end with a space or a dot".into());
        }
        let stem = name
            .split('.')
            .next()
            .unwrap_or_default()
            .trim_end()
            .to_uppercase();
        let reserved = matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|n| {
                matches!(
                    n,
                    "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        });
        if reserved {
            return Err("Name is reserved by Windows".into());
        }
    }
    Ok(())
}

pub fn file_identity(path: &std::path::Path) -> Option<String> {
    file_id::get_file_id(path).ok().map(|id| format!("{id:?}"))
}

pub fn config_path(path: &Path) -> String {
    path.to_str().map(str::to_string).unwrap_or_else(|| {
        reqwest::Url::from_file_path(path)
            .map(|url| url.to_string())
            .unwrap_or_default()
    })
}
pub fn parse_config_path(value: &str) -> Result<PathBuf, String> {
    let path = if value.starts_with("file:") {
        reqwest::Url::parse(value)
            .map_err(|_| "File URI is invalid")?
            .to_file_path()
            .map_err(|_| "File URI is invalid")?
    } else {
        PathBuf::from(value)
    };
    if !path.is_absolute()
        || value.len() > crate::shared::constants::MAX_PATH_BYTES
        || value.contains('\0')
    {
        return Err("Path must be absolute".into());
    }
    Ok(path)
}

pub fn group_label(
    file: &crate::shared::models::FileItem,
    sizes: &std::collections::HashMap<PathBuf, crate::shared::models::FolderSize>,
    group: crate::shared::models::GroupColumn,
) -> String {
    use crate::shared::models::{GroupColumn, SizeState};
    const SMALL_BYTES: u64 = 1024 * 1024;
    const LARGE_BYTES: u64 = 100 * 1024 * 1024;
    match group {
        GroupColumn::None => String::new(),
        GroupColumn::Name => file
            .name
            .chars()
            .next()
            .map(|ch| ch.to_uppercase().to_string())
            .unwrap_or_else(|| "Unknown".into()),
        GroupColumn::Extension => {
            if file.is_directory {
                "Folders".into()
            } else if file.extension.is_empty() {
                "No extension".into()
            } else {
                file.extension.to_uppercase()
            }
        }
        GroupColumn::Tags => {
            let mut tags = file
                .tags
                .iter()
                .map(|tag| tag.name.as_str())
                .collect::<Vec<_>>();
            tags.sort();
            if tags.is_empty() {
                "No tags".into()
            } else {
                tags.join(", ")
            }
        }
        GroupColumn::Size => {
            let bytes = if file.is_directory {
                sizes
                    .get(&file.path)
                    .filter(|size| size.state == SizeState::Complete)
                    .map(|size| size.bytes)
            } else {
                Some(file.size)
            };
            match bytes {
                None => "Unknown".into(),
                Some(0) => "Empty".into(),
                Some(bytes) if bytes < SMALL_BYTES => "Under 1 MiB".into(),
                Some(bytes) if bytes < LARGE_BYTES => "1–100 MiB".into(),
                Some(_) => "100 MiB or larger".into(),
            }
        }
        GroupColumn::Modified | GroupColumn::Created => {
            let timestamp = if group == GroupColumn::Created {
                file.created_at
            } else {
                file.modified_at
            };
            if timestamp == 0 {
                return "Unknown".into();
            }
            chrono::DateTime::from_timestamp(timestamp, 0)
                .map(|date| {
                    date.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d")
                        .to_string()
                })
                .unwrap_or_else(|| "Unknown".into())
        }
    }
}

pub fn serialize_native_path<S: serde::Serializer>(
    path: &Path,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&config_path(path))
}
pub fn deserialize_native_path<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<PathBuf, D::Error> {
    let value = <String as serde::Deserialize>::deserialize(deserializer)?;
    parse_config_path(&value).map_err(serde::de::Error::custom)
}

pub fn open_verified_file(path: &Path, expected: Option<&str>) -> Result<std::fs::File, String> {
    let canonical = canonicalize(path).map_err(|_| "File cannot be accessed")?;
    let before = file_identity(&canonical).ok_or("File identity is unavailable")?;
    if expected.is_some_and(|expected| expected != before) {
        return Err("File was replaced while opening".into());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options
        .open(&canonical)
        .map_err(|_| "File could not be opened")?;
    if !file
        .metadata()
        .map_err(|_| "File metadata is unavailable")?
        .is_file()
    {
        return Err("Target is not a regular file".into());
    }
    let identity = opened_file_identity(&file)?;
    if identity != before
        || file_identity(&canonical).as_ref() != Some(&identity)
        || canonicalize(path).ok().as_ref() != Some(&canonical)
    {
        return Err("File was replaced while opening".into());
    }
    Ok(file)
}
fn opened_file_identity(file: &std::fs::File) -> Result<String, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file
            .metadata()
            .map_err(|_| "File identity is unavailable")?;
        Ok(format!(
            "{:?}",
            file_id::FileId::new_inode(metadata.dev(), metadata.ino())
        ))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_ID_INFO, FileIdInfo, GetFileInformationByHandle,
            GetFileInformationByHandleEx,
        };
        let mut info: FILE_ID_INFO = unsafe { std::mem::zeroed() };
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileIdInfo,
                &mut info as *mut FILE_ID_INFO as _,
                std::mem::size_of::<FILE_ID_INFO>() as u32,
            )
        } != 0
        {
            return Ok(format!(
                "{:?}",
                file_id::FileId::new_high_res(
                    info.VolumeSerialNumber,
                    u128::from_le_bytes(info.FileId.Identifier)
                )
            ));
        }
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err("File identity is unavailable".into());
        }
        Ok(format!(
            "{:?}",
            file_id::FileId::new_low_res(
                info.dwVolumeSerialNumber,
                (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)
            )
        ))
    }
}

pub fn replace_marked_text(source: &str, replacements: &[(String, String)]) -> String {
    let mut ranges = Vec::new();
    let regions = markdown_text_regions(source);
    for (marker, replacement) in replacements {
        if marker.is_empty() {
            continue;
        }
        for (start, _) in source.match_indices(marker) {
            let range = start..start + marker.len();
            if is_markdown_text(source, &range, &regions) {
                ranges.push((range, replacement.clone()));
            }
        }
    }
    ranges.sort_by_key(|(range, _)| (range.start, std::cmp::Reverse(range.len())));
    let mut end = 0;
    ranges.retain(|(range, _)| {
        if range.start < end {
            return false;
        }
        end = range.end;
        true
    });
    let mut output = source.to_string();
    for (range, replacement) in ranges.into_iter().rev() {
        output.replace_range(range, &replacement);
    }
    output
}

/// `fs::canonicalize` followed by [`simplify_path`], so stored and displayed paths never carry
/// the Windows `\\?\` prefix when a normal path names the same file.
pub fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path).map(simplify_path)
}

/// Converts `\\?\C:\x` to `C:\x` and `\\?\UNC\server\share\x` to `\\server\share\x` when the
/// short form is unambiguous (no reserved names, trailing dots/spaces, or MAX_PATH overflow).
/// Paths are returned unchanged on other platforms.
pub fn simplify_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        let mut components = path.components();
        let Some(Component::Prefix(prefix)) = components.next() else {
            return path;
        };
        let rest: PathBuf = components.clone().collect();
        let base = match prefix.kind() {
            Prefix::VerbatimDisk(letter) => format!("{}:", letter as char),
            Prefix::VerbatimUNC(server, share) => format!(
                r"\\{}\{}",
                server.to_string_lossy(),
                share.to_string_lossy()
            ),
            _ => return path,
        };
        let is_safe = components.all(|component| match component {
            Component::RootDir => true,
            Component::Normal(name) => name
                .to_str()
                .is_some_and(|name| validate_component(name).is_ok()),
            _ => false,
        });
        let simplified = PathBuf::from(base).join(rest);
        if is_safe && simplified.as_os_str().len() < 260 {
            return simplified;
        }
        path
    }
    #[cfg(not(windows))]
    {
        path
    }
}

/// Normalises text typed or pasted into the address bar before validation: trims whitespace and
/// surrounding quotes (Explorer's "Copy as path"), expands `~`, accepts `/` or `\` on Windows,
/// turns a bare drive `D:` into `D:\`, and on WSL maps `C:\Users` to `/mnt/c/Users`.
pub fn normalize_input(input: &str) -> String {
    let mut text = input.trim();
    for quote in ['"', '\''] {
        if text.len() >= 2 && text.starts_with(quote) && text.ends_with(quote) {
            text = text[1..text.len() - 1].trim();
        }
    }
    let mut text = text.to_string();
    if (text == "~" || text.starts_with("~/") || text.starts_with("~\\"))
        && let Some(home) = directories::UserDirs::new()
    {
        text = format!("{}{}", home.home_dir().display(), &text[1..]);
    }
    let bytes = text.as_bytes();
    let is_drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    #[cfg(windows)]
    {
        if !text.starts_with("file:") {
            text = text.replace('/', "\\");
        }
        if is_drive && text.len() == 2 {
            text.push('\\');
        }
    }
    #[cfg(not(windows))]
    {
        if is_drive && (text.len() == 2 || matches!(bytes[2], b'\\' | b'/')) {
            let mount = PathBuf::from(format!("/mnt/{}", (bytes[0] as char).to_ascii_lowercase()));
            if mount.is_dir() {
                let rest = text[2..].replace('\\', "/");
                text = format!("{}{}", mount.display(), rest.trim_end_matches('/'));
            }
        }
    }
    text
}

const WINDOWS_FORBIDDEN: &str = "\\/<>:\"|?*";

/// A cleaned-up name for the rename dialog's "Use suggested name": invalid characters become
/// `_`, control characters are dropped, and trailing dots/spaces are trimmed.
pub fn suggest_component(name: &str) -> String {
    let forbidden = if cfg!(windows) {
        WINDOWS_FORBIDDEN
    } else {
        "/"
    };
    let mut cleaned: String = name
        .chars()
        .filter(|ch| !ch.is_control())
        .map(|ch| if forbidden.contains(ch) { '_' } else { ch })
        .collect();
    if cfg!(windows) {
        cleaned = cleaned.trim_end_matches([' ', '.']).to_string();
    }
    let cleaned = cleaned.trim().to_string();
    if validate_component(&cleaned).is_ok() {
        cleaned
    } else if cleaned.is_empty() || matches!(cleaned.as_str(), "." | "..") {
        "New item".into()
    } else {
        format!("_{cleaned}")
    }
}

/// Non-blocking warning for names that are valid here but break on Windows or FAT/exFAT drives.
pub fn portability_warning(name: &str) -> Option<&'static str> {
    if cfg!(windows) {
        return None;
    }
    if name
        .chars()
        .any(|ch| WINDOWS_FORBIDDEN.contains(ch) && ch != '/')
    {
        Some("Contains \\ : * ? \" < > | — the name cannot be copied to Windows or USB drives")
    } else if name.ends_with([' ', '.']) {
        Some("Ends with a space or dot — Windows will not keep this name")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_validated_and_suggested() {
        assert!(validate_component("report.pdf").is_ok());
        assert!(validate_component("").is_err());
        assert!(validate_component("..").is_err());
        assert!(validate_component("a/b").is_err());
        assert!(validate_component("   ").is_err());
        assert!(validate_component("line\nbreak").is_err());
        assert!(validate_component(&"x".repeat(256)).is_err());
        assert_eq!(suggest_component("a/b"), "a_b");
        assert!(validate_component(&suggest_component("CON")).is_ok());
        assert!(portability_warning("a:b").is_some() || cfg!(windows));
        assert!(portability_warning("plain.txt").is_none());
    }

    #[test]
    fn natural_order() {
        let mut names = vec!["file10", "file2", "File1", "file02"];
        names.sort_by(|a, b| natural_compare(a, b));
        assert_eq!(names, vec!["File1", "file2", "file02", "file10"]);
    }

    #[test]
    fn address_input_is_normalized() {
        assert_eq!(normalize_input("  \"/tmp\"  "), "/tmp");
        assert_eq!(normalize_input("'/tmp'"), "/tmp");
    }
}

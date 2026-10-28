use crate::shared::constants::{CHANNEL_CAPACITY, MAX_OPERATION_ITEMS};
use crate::shared::models::*;
use crate::shared::traits::MetadataRepository;
use crate::shared::utils::{canonicalize, path_from_key, path_key, validate_component};
use crossbeam_channel::{Sender, bounded};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

const COPY_BUFFER_BYTES: usize = 1024 * 1024;
const COLLISION_ATTEMPTS: usize = 10000;
const MAX_TRASH_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
const TEMPORARY_PREFIX: &str = ".filemofm-copy-";
const REPLACED_PREFIX: &str = ".filemofm-replaced-";

#[derive(serde::Serialize, serde::Deserialize)]
struct TrashManifest {
    source: Vec<u8>,
    stored: Vec<u8>,
    moved_at: i64,
    digest: Vec<u8>,
    files: Vec<(String, Vec<u8>)>,
}

/// One entry of a tree snapshot: relative path, kind, length and modification time. Comparing
/// snapshots detects a source that changed during a copy without re-reading file contents.
type Snapshot = Vec<(Vec<u8>, u8, u64, i128)>;

pub struct OperationService {
    requests: Sender<(OperationRequest, u64)>,
    cancel: Arc<AtomicBool>,
    epoch: Arc<AtomicU64>,
    shutdown: Arc<AtomicBool>,
    worker: parking_lot::Mutex<Option<std::thread::JoinHandle<()>>>,
}
impl OperationService {
    pub fn new(
        repository: Arc<dyn MetadataRepository>,
        paths: RuntimePaths,
        sender: EventSink,
    ) -> Self {
        let (requests, receiver) = bounded::<(OperationRequest, u64)>(CHANNEL_CAPACITY);
        let cancel = Arc::new(AtomicBool::new(false));
        let epoch = Arc::new(AtomicU64::new(0));
        let current = epoch.clone();
        let cancellation = cancel.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let stopping = shutdown.clone();
        let worker = std::thread::spawn(move || {
            recover(repository.as_ref());
            while !stopping.load(Ordering::Acquire) {
                let (mut request, generation) =
                    match receiver.recv_timeout(std::time::Duration::from_millis(250)) {
                        Ok(request) => request,
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    };
                cancellation.store(false, Ordering::Release);
                let mut report = OperationReport {
                    id: request.id.clone(),
                    done: 0,
                    total: request.sources.len(),
                    failures: Vec::new(),
                    is_cancelled: false,
                };
                if generation != current.load(Ordering::Acquire) {
                    report.is_cancelled = true;
                    let _ = sender.send(BackgroundMessage::OperationComplete(report));
                    continue;
                }
                // A folder and something inside it: the folder already covers the child.
                request
                    .sources
                    .sort_by_key(|path| path.components().count());
                let mut sources: Vec<PathBuf> = Vec::new();
                for source in request.sources {
                    if !sources.iter().any(|parent| source.starts_with(parent)) {
                        sources.push(source);
                    }
                }
                request.sources = sources;
                report.total = request.sources.len();
                let stale = stale_sources(&request, repository.as_ref());
                for source in &request.sources {
                    if cancellation.load(Ordering::Acquire) {
                        report.is_cancelled = true;
                        break;
                    }
                    let result = match stale.get(source) {
                        Some(reason) => Err(reason.clone()),
                        None => execute_item(
                            &request,
                            source,
                            &paths,
                            repository.as_ref(),
                            &cancellation,
                        ),
                    };
                    match result {
                        Ok(()) => {
                            report.done += 1;
                            if request.kind == OperationKind::Relocate {
                                let _ = sender.send(BackgroundMessage::RootRelocated {
                                    old: source.clone(),
                                    new: request.destination.clone(),
                                });
                            }
                        }
                        Err(error) => report.failures.push(format!(
                            "{}: {error}",
                            source.file_name().unwrap_or_default().to_string_lossy()
                        )),
                    }
                    let _ = sender.send(BackgroundMessage::OperationProgress(report.clone()));
                }
                report.is_cancelled |= cancellation.load(Ordering::Acquire);
                let _ = repository.finish_operation(
                    &request.id,
                    if report.is_cancelled {
                        "cancelled"
                    } else {
                        "failed"
                    },
                );
                let _ = sender.send(BackgroundMessage::OperationComplete(report));
            }
        });
        Self {
            requests,
            cancel,
            epoch,
            shutdown,
            worker: parking_lot::Mutex::new(Some(worker)),
        }
    }
    pub fn submit(&self, request: OperationRequest) -> Result<(), String> {
        if self.shutdown.load(Ordering::Acquire) {
            return Err("Operation queue stopped".into());
        }
        if request.sources.is_empty() || request.sources.len() > MAX_OPERATION_ITEMS {
            return Err("Select files for the operation".into());
        }
        self.requests
            .try_send((request, self.epoch.load(Ordering::Acquire)))
            .map_err(|_| "Operation queue is full".into())
    }
    pub fn cancel(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.cancel.store(true, Ordering::Release);
    }
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.cancel();
        if let Some(worker) = self.worker.lock().take() {
            let _ = worker.join();
        }
    }
}
impl Drop for OperationService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Finishes or undoes journal entries left by a crash.
fn recover(repository: &dyn MetadataRepository) {
    let Ok(entries) = repository.unfinished_operations() else {
        return;
    };
    for (id, source, target, phase) in entries {
        match phase.as_str() {
            // The filesystem move happened; only the metadata update is missing.
            "move:committed" => {
                if !source.exists() && target.exists() {
                    let _ = repository.update_path(&source, &target);
                }
            }
            // Crashed between the two renames of a case-only rename: put the name back.
            "rename:intermediate" => {
                if target.symlink_metadata().is_ok() && source.symlink_metadata().is_err() {
                    let _ = publish(&target, &source);
                }
            }
            // `target` is the backup of a replaced item.
            "replace:backup" => {
                if target.symlink_metadata().is_ok() {
                    if source.symlink_metadata().is_err() {
                        let _ = publish(&target, &source);
                    } else {
                        let _ = remove_tree(&target, &AtomicBool::new(false));
                    }
                }
            }
            _ => {
                if let Some(parent) = target.parent() {
                    let temporary = parent.join(temporary_name(&id, &source));
                    if temporary.symlink_metadata().is_ok() {
                        let _ = remove_tree(&temporary, &AtomicBool::new(false));
                    }
                }
            }
        }
        let _ = repository.journal(&id, &source, &target, "failed");
    }
}

/// Sources whose metadata changed after they were selected. Checked once per request instead of
/// once per item, so large selections stay linear.
fn stale_sources(
    request: &OperationRequest,
    repository: &dyn MetadataRepository,
) -> HashMap<PathBuf, String> {
    let mut stale = HashMap::new();
    if request.kind == OperationKind::Relocate {
        return stale;
    }
    for (id, revision) in &request.revisions {
        let Ok(Some(file)) = repository.get_file(id) else {
            continue;
        };
        if file.revision != *revision || file.is_missing {
            stale.insert(
                file.path,
                "Target changed since it was selected; review the operation again".to_string(),
            );
        }
    }
    stale
}

fn temporary_name(id: &str, source: &Path) -> String {
    let digest = Sha256::digest([id.as_bytes(), &path_key(source)].concat());
    format!(
        "{TEMPORARY_PREFIX}{}",
        digest[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

/// Resolves the parent folder but keeps the final component, so an operation on a link acts on
/// the link itself and never on whatever it points to.
fn resolve_entry(path: &Path) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .ok_or("Drive and filesystem roots cannot be changed")?;
    let parent = path.parent().ok_or("Parent folder is unavailable")?;
    let parent = canonicalize(parent).map_err(|_| "Source is unavailable")?;
    let entry = parent.join(name);
    entry
        .symlink_metadata()
        .map_err(|_| "Source is unavailable")?;
    Ok(entry)
}

fn is_protected(path: &Path, paths: &RuntimePaths) -> bool {
    path.starts_with(&paths.root) || paths.root.starts_with(path)
}

fn execute_item(
    request: &OperationRequest,
    source: &Path,
    paths: &RuntimePaths,
    repository: &dyn MetadataRepository,
    cancel: &AtomicBool,
) -> Result<(), String> {
    if request.kind == OperationKind::Relocate {
        if !source.is_absolute() || source.exists() || request.revisions.is_empty() {
            return Err("The old root must be unavailable and have metadata".into());
        }
        reject_links(&request.destination)?;
        let target =
            canonicalize(&request.destination).map_err(|_| "Replacement root is unavailable")?;
        if !target.is_dir() || is_protected(&target, paths) {
            return Err("Replacement root is not allowed".into());
        }
        repository
            .journal(&request.id, source, &target, "relocate:prepared")
            .map_err(|error| error.to_string())?;
        repository
            .relocate(source, &target, &request.revisions)
            .map_err(|error| error.to_string())?;
        repository
            .journal(&request.id, source, &target, "complete")
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    if matches!(
        request.kind,
        OperationKind::CreateFile | OperationKind::CreateFolder
    ) {
        validate_component(&request.name)?;
        reject_links(&request.destination)?;
        let destination =
            canonicalize(&request.destination).map_err(|_| "Destination folder is unavailable")?;
        if is_protected(&destination, paths) {
            return Err("Application folders are protected".into());
        }
        let target = destination.join(&request.name);
        repository
            .journal(&request.id, &target, &target, "create:prepared")
            .map_err(|error| error.to_string())?;
        let created = if request.kind == OperationKind::CreateFolder {
            fs::create_dir(&target)
        } else {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map(|_| ())
        };
        created.map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                "An item with this name already exists".to_string()
            } else if request.kind == OperationKind::CreateFolder {
                "Folder could not be created".to_string()
            } else {
                "File could not be created".to_string()
            }
        })?;
        repository
            .invalidate_sizes(&target)
            .map_err(|error| error.to_string())?;
        repository
            .journal(&request.id, &target, &target, "complete")
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    if request.kind == OperationKind::Restore {
        return restore_item(request, source, paths, repository, cancel);
    }
    if let Some((_, expected)) = request.identities.iter().find(|(path, _)| path == source)
        && crate::shared::utils::file_identity(source).as_ref() != Some(expected)
    {
        return Err("Source was replaced; select the item again before retrying".into());
    }
    let source = resolve_entry(source)?;
    let source_parent = source.parent().ok_or("Parent folder is unavailable")?;
    reject_links(source_parent)?;
    if is_protected(&source, paths) {
        return Err("Application system locations cannot be changed".into());
    }
    if request.kind == OperationKind::Delete {
        if !request.is_permanent_confirmed {
            return Err("Permanent deletion requires confirmation".into());
        }
        repository
            .journal(&request.id, &source, &source, "delete:prepared")
            .map_err(|error| error.to_string())?;
        remove_tree(&source, cancel)?;
        repository
            .mark_missing(&source)
            .map_err(|error| error.to_string())?;
        repository
            .journal(&request.id, &source, &source, "complete")
            .map_err(|error| error.to_string())?;
        repository
            .invalidate_sizes(&source)
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    if request.kind == OperationKind::Trash {
        repository
            .journal(&request.id, &source, &source, "trash:prepared")
            .map_err(|error| error.to_string())?;
        let trashed = os_trash_supported(&source) && trash::delete(&source).is_ok();
        if !trashed {
            if !request.allow_trash_fallback {
                return Err("This location has no system trash (network, removable or WSL drive). Delete permanently, or enable the app trash fallback in Settings › Files".into());
            }
            application_trash(&source, paths, repository, &request.id, cancel)?;
        }
        repository
            .mark_missing(&source)
            .map_err(|error| error.to_string())?;
        repository
            .invalidate_sizes(&source)
            .map_err(|error| error.to_string())?;
        repository
            .journal(&request.id, &source, &source, "complete")
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    let parent = if request.kind == OperationKind::Rename {
        source_parent.to_path_buf()
    } else {
        canonicalize(&request.destination).map_err(|_| "Destination folder is unavailable")?
    };
    reject_links(&parent)?;
    if !parent.is_dir() {
        return Err("Destination must be a folder".into());
    }
    let name = if request.kind == OperationKind::Rename {
        validate_component(&request.name)?;
        std::ffi::OsString::from(&request.name)
    } else {
        source
            .file_name()
            .ok_or("Source name is unavailable")?
            .to_os_string()
    };
    let mut target = parent.join(&name);
    if is_protected(&target, paths) {
        return Err("Application destination is protected".into());
    }
    let is_folder = source
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.is_dir());
    if target == source {
        match request.kind {
            // Cut and paste into the same folder, or renaming to the same name: nothing to do.
            OperationKind::Move | OperationKind::Rename => return Ok(()),
            // Copy and paste into the same folder makes "name - Copy", like Explorer.
            _ => target = free_name(&parent, &name, " - Copy")?,
        }
    } else if is_folder && target.starts_with(&source) {
        return Err("A folder cannot be copied or moved into itself".into());
    } else if target.symlink_metadata().is_ok() {
        let same = crate::shared::utils::file_identity(&source)
            .zip(crate::shared::utils::file_identity(&target))
            .is_some_and(|(a, b)| a == b);
        if same {
            return rename_alias(request, &source, &target, &parent, repository);
        }
        match request.collision {
            CollisionPolicy::Merge if request.kind != OperationKind::Rename => {
                return merge(request, &source, &target, paths, repository, cancel);
            }
            CollisionPolicy::Replace if request.kind != OperationKind::Rename => {
                return replace(request, &source, &target, &parent, repository, cancel);
            }
            CollisionPolicy::RenameCopy if request.kind != OperationKind::Rename => {
                target = free_name(&parent, &name, "")?;
            }
            _ => return Err("Target already exists; it was not overwritten".into()),
        }
    }
    repository
        .journal(&request.id, &source, &target, "transfer:prepared")
        .map_err(|error| error.to_string())?;
    let is_move = matches!(request.kind, OperationKind::Move | OperationKind::Rename);
    transfer(
        &source,
        &target,
        is_move,
        cancel,
        &temporary_name(&request.id, &source),
    )?;
    repository
        .journal(
            &request.id,
            &source,
            &target,
            if is_move {
                "move:committed"
            } else {
                "copy:committed"
            },
        )
        .map_err(|error| error.to_string())?;
    if is_move {
        repository
            .update_path(&source, &target)
            .map_err(|error| error.to_string())?;
    }
    repository
        .invalidate_sizes(&source)
        .and_then(|_| repository.invalidate_sizes(&target))
        .map_err(|error| error.to_string())?;
    repository
        .journal(&request.id, &source, &target, "complete")
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// The target is the same file under another spelling. Only a case-only rename is meaningful;
/// it goes through an intermediate name because case-insensitive filesystems treat it as a no-op.
fn rename_alias(
    request: &OperationRequest,
    source: &Path,
    target: &Path,
    parent: &Path,
    repository: &dyn MetadataRepository,
) -> Result<(), String> {
    let lower = |path: &Path| {
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
    };
    if request.kind != OperationKind::Rename || source.file_name() == target.file_name() {
        return Err("Source and target are the same file".into());
    }
    if lower(source) != lower(target) {
        return Err("Rename to an alias of the source was rejected".into());
    }
    let intermediate = parent.join(format!(".filemofm-rename-{}", request.id));
    repository
        .journal(&request.id, source, &intermediate, "rename:intermediate")
        .map_err(|error| error.to_string())?;
    publish(source, &intermediate).map_err(|_| "Temporary rename failed")?;
    if publish(&intermediate, target).is_err() {
        let _ = publish(&intermediate, source);
        return Err("Final rename failed; check the operation journal".into());
    }
    repository
        .update_path(source, target)
        .map_err(|error| error.to_string())?;
    repository
        .journal(&request.id, source, target, "complete")
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Merges two folders: subfolders present on both sides are merged recursively, files that
/// already exist are skipped and reported.
fn merge(
    request: &OperationRequest,
    source: &Path,
    target: &Path,
    paths: &RuntimePaths,
    repository: &dyn MetadataRepository,
    cancel: &AtomicBool,
) -> Result<(), String> {
    if !source.is_dir() || !target.is_dir() {
        return Err("Merge only applies to two folders".into());
    }
    let mut failures = 0usize;
    for entry in fs::read_dir(source).map_err(|_| "Source folder could not be read")? {
        if cancel.load(Ordering::Acquire) {
            return Err("Merge cancelled; completed items are kept".into());
        }
        let entry = entry.map_err(|_| "Folder contents changed")?;
        let mut child = request.clone();
        child.kind = if request.kind == OperationKind::Move {
            OperationKind::Move
        } else {
            OperationKind::Copy
        };
        child.destination = target.to_path_buf();
        child.collision = if entry.file_type().is_ok_and(|kind| kind.is_dir())
            && target.join(entry.file_name()).is_dir()
        {
            CollisionPolicy::Merge
        } else {
            CollisionPolicy::Skip
        };
        child.sources = vec![entry.path()];
        child.identities.clear();
        if execute_item(&child, &entry.path(), paths, repository, cancel).is_err() {
            failures += 1;
        }
    }
    if failures > 0 {
        return Err(format!(
            "Merge partly finished: {failures} item(s) skipped or failed"
        ));
    }
    if request.kind == OperationKind::Move {
        fs::remove_dir(source).map_err(|_| "Source folder changed; new contents were kept")?;
        repository
            .mark_missing(source)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Overwrites `target`. The old item is renamed to a sibling backup first, so a failed
/// transfer restores it; after success it is removed, matching Explorer's Replace.
fn replace(
    request: &OperationRequest,
    source: &Path,
    target: &Path,
    parent: &Path,
    repository: &dyn MetadataRepository,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let backup = parent.join(format!(
        "{REPLACED_PREFIX}{}",
        &temporary_name(&request.id, source)[TEMPORARY_PREFIX.len()..]
    ));
    repository
        .journal(&request.id, target, &backup, "replace:backup")
        .map_err(|error| error.to_string())?;
    publish(target, &backup).map_err(|_| "The existing item could not be set aside")?;
    // Journaled so startup recovery removes the temporary copy if the app stops mid-transfer.
    repository
        .journal(&request.id, source, target, "transfer:prepared")
        .map_err(|error| error.to_string())?;
    let is_move = request.kind == OperationKind::Move;
    if let Err(error) = transfer(
        source,
        target,
        is_move,
        cancel,
        &temporary_name(&request.id, source),
    ) {
        let _ = publish(&backup, target);
        return Err(error);
    }
    repository
        .mark_missing(target)
        .map_err(|error| error.to_string())?;
    if is_move {
        repository
            .update_path(source, target)
            .map_err(|error| error.to_string())?;
    }
    if let Err(error) = remove_tree(&backup, &AtomicBool::new(false)) {
        tracing::warn!(%error, "replaced item backup was not removed");
    }
    repository
        .journal(&request.id, target, &backup, "complete")
        .and_then(|_| repository.journal(&request.id, source, target, "complete"))
        .map_err(|error| error.to_string())?;
    repository
        .invalidate_sizes(source)
        .and_then(|_| repository.invalidate_sizes(target))
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// First free name: `stem{suffix}.ext`, then `stem{suffix} (2).ext`, … or, with an empty
/// suffix, `stem (1).ext`, `stem (2).ext`, …
fn free_name(parent: &Path, name: &std::ffi::OsStr, suffix: &str) -> Result<PathBuf, String> {
    let path = Path::new(name);
    let is_folder = parent.join(name).is_dir();
    let (stem, extension) = if is_folder {
        (name.to_string_lossy().into_owned(), String::new())
    } else {
        (
            path.file_stem()
                .unwrap_or(name)
                .to_string_lossy()
                .into_owned(),
            path.extension()
                .map(|extension| format!(".{}", extension.to_string_lossy()))
                .unwrap_or_default(),
        )
    };
    let first = if suffix.is_empty() { 1 } else { 0 };
    for number in first..=COLLISION_ATTEMPTS {
        let candidate = match (number, suffix.is_empty()) {
            (0, _) => format!("{stem}{suffix}{extension}"),
            (1, false) => continue,
            (number, _) => format!("{stem}{suffix} ({number}){extension}"),
        };
        let candidate = parent.join(candidate);
        if candidate.symlink_metadata().is_err() {
            return Ok(candidate);
        }
    }
    Err("No free name for the copy".into())
}

#[cfg(windows)]
fn os_trash_supported(path: &Path) -> bool {
    use std::path::{Component, Prefix};
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    use windows_sys::Win32::System::WindowsProgramming::DRIVE_FIXED;
    // Only fixed disks have a Recycle Bin. Elsewhere the shell can delete permanently
    // without asking, so those locations are treated as having no system trash.
    match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                let root: Vec<u16> = format!("{}:\\", letter as char)
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
                unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_FIXED }
            }
            _ => false,
        },
        _ => false,
    }
}

#[cfg(not(windows))]
fn os_trash_supported(_path: &Path) -> bool {
    true
}

fn application_trash(
    source: &Path,
    paths: &RuntimePaths,
    repository: &dyn MetadataRepository,
    id: &str,
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    let directory = paths.trash.join(uuid::Uuid::new_v4().to_string());
    fs::create_dir(&directory).map_err(|_| "App trash could not be created")?;
    let target = directory.join(source.file_name().ok_or("Source name is invalid")?);
    let files = repository
        .files_under(source)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|(id, path)| (id, path_key(&path)))
        .collect();
    let manifest = TrashManifest {
        source: path_key(source),
        stored: path_key(&target),
        moved_at: chrono::Utc::now().timestamp(),
        digest: tree_digest(source, cancel)?,
        files,
    };
    let manifest_path = directory.join("restore.json");
    let bytes = serde_json::to_vec(&manifest).map_err(|_| "Trash manifest failed")?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&manifest_path)
        .map_err(|_| "Trash manifest could not be created")?;
    output
        .write_all(&bytes)
        .and_then(|_| output.sync_all())
        .map_err(|_| "Trash manifest could not be saved")?;
    repository
        .journal(id, source, &target, "trash:copying")
        .map_err(|error| error.to_string())?;
    transfer(source, &target, true, cancel, &temporary_name(id, source))?;
    repository
        .mark_missing(source)
        .map_err(|error| error.to_string())?;
    Ok(manifest_path)
}

fn restore_item(
    request: &OperationRequest,
    manifest_path: &Path,
    paths: &RuntimePaths,
    repository: &dyn MetadataRepository,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let canonical = canonicalize(manifest_path).map_err(|_| "Restore manifest is unavailable")?;
    if !canonical.starts_with(&paths.trash)
        || canonical.file_name() != Some(std::ffi::OsStr::new("restore.json"))
    {
        return Err("Manifest must come from the app trash".into());
    }
    reject_links(&canonical)?;
    let mut bytes = Vec::new();
    File::open(&canonical)
        .map_err(|_| "Restore manifest is unavailable")?
        .take(MAX_TRASH_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Restore manifest could not be read")?;
    if bytes.len() as u64 > MAX_TRASH_MANIFEST_BYTES {
        return Err("Restore manifest is too large".into());
    }
    let manifest: TrashManifest =
        serde_json::from_slice(&bytes).map_err(|_| "Restore manifest is invalid")?;
    let source = path_from_key(manifest.stored);
    let original = path_from_key(manifest.source);
    let parent = canonical.parent().ok_or("Restore manifest is invalid")?;
    if source.parent() != Some(parent)
        || source.file_name() == Some(std::ffi::OsStr::new("restore.json"))
        || !original.is_absolute()
        || is_protected(&original, paths)
    {
        return Err("Restore target is invalid".into());
    }
    reject_links(&source)?;
    reject_links(original.parent().ok_or("Original folder is unavailable")?)?;
    if original.symlink_metadata().is_ok() {
        return Err("Restore target already exists; move it away before restoring".into());
    }
    let mut ids = std::collections::HashSet::new();
    let files = manifest
        .files
        .into_iter()
        .map(|(id, path)| (id, path_from_key(path)))
        .collect::<Vec<_>>();
    if files.iter().any(|(id, path)| {
        uuid::Uuid::parse_str(id).is_err()
            || !ids.insert(id.clone())
            || !path.starts_with(&original)
    }) {
        return Err("Restore manifest metadata is invalid".into());
    }
    repository
        .journal(&request.id, &source, &original, "restore:prepared")
        .map_err(|error| error.to_string())?;
    if tree_digest(&source, cancel)? != manifest.digest {
        return Err("Trash contents changed; restore cancelled".into());
    }
    transfer(
        &source,
        &original,
        true,
        cancel,
        &temporary_name(&request.id, &source),
    )?;
    repository
        .restore_files(&files)
        .map_err(|error| error.to_string())?;
    repository
        .journal(&request.id, &source, &original, "complete")
        .map_err(|error| error.to_string())?;
    fs::remove_file(manifest_path)
        .map_err(|_| "Restore finished; the manifest could not be cleaned up")?;
    let _ = fs::remove_dir(parent);
    Ok(())
}

/// Rejects symbolic links and reparse points in `path` and all of its ancestors.
fn reject_links(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        let metadata = ancestor
            .symlink_metadata()
            .map_err(|_| "Location cannot be accessed")?;
        if metadata.file_type().is_symlink() {
            return Err("Operations through symbolic links are not allowed".into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            // Drive roots of mounted volumes report a reparse point; only folders below them matter.
            if ancestor.parent().is_some()
                && metadata.file_attributes()
                    & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                    != 0
            {
                return Err("Operations through reparse points are not allowed".into());
            }
        }
    }
    Ok(())
}

/// Renames without ever replacing an existing target.
fn publish(source: &Path, target: &Path) -> Result<(), std::io::Error> {
    #[cfg(target_os = "linux")]
    {
        match rustix::fs::renameat_with(
            rustix::fs::CWD,
            source,
            rustix::fs::CWD,
            target,
            rustix::fs::RenameFlags::NOREPLACE,
        ) {
            Ok(()) => Ok(()),
            // 9p (WSL /mnt/c), CIFS and older NFS do not support RENAME_NOREPLACE.
            Err(error)
                if error == rustix::io::Errno::INVAL
                    || error == rustix::io::Errno::NOSYS
                    || error == rustix::io::Errno::OPNOTSUPP =>
            {
                if target.symlink_metadata().is_ok() {
                    return Err(std::io::ErrorKind::AlreadyExists.into());
                }
                fs::rename(source, target)
            }
            Err(error) => Err(std::io::Error::from(error)),
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let source = source
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let target = target
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        // No MOVEFILE_REPLACE_EXISTING: an existing target fails with ERROR_ALREADY_EXISTS.
        if unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                source.as_ptr(),
                target.as_ptr(),
                0,
            )
        } == 0
        {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

fn is_cross_device(error: &std::io::Error) -> bool {
    // ERROR_NOT_SAME_DEVICE (17) on Windows, EXDEV on Unix.
    error.kind() == std::io::ErrorKind::CrossesDevices
        || (cfg!(windows) && error.raw_os_error() == Some(17))
}

fn transfer(
    source: &Path,
    target: &Path,
    is_move: bool,
    cancel: &AtomicBool,
    temporary: &str,
) -> Result<(), String> {
    if is_move {
        match publish(source, target) {
            Ok(()) => return Ok(()),
            Err(error) if is_cross_device(&error) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err("Target already exists; it was not overwritten".into());
            }
            Err(_) => return Err("Move was refused; the source was kept".into()),
        }
    }
    let parent = target.parent().ok_or("Destination folder is invalid")?;
    let temporary = parent.join(temporary);
    let result = (|| {
        let before = copy_tree(source, &temporary, cancel, is_move)?;
        if cancel.load(Ordering::Acquire) {
            return Err("Operation cancelled".into());
        }
        if snapshot(source)? != before {
            return Err("Source changed during transfer; the copy was cancelled".into());
        }
        publish(&temporary, target).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                "Target already exists; it was not overwritten".to_string()
            } else {
                "Target changed or is not writable".to_string()
            }
        })?;
        if is_move {
            if snapshot(source)? != before {
                return Err("Copy finished; the source changed and was kept".into());
            }
            remove_tree(source, cancel)
                .map_err(|_| "Copy finished; the source could not be removed")?;
        }
        Ok(())
    })();
    if temporary.symlink_metadata().is_ok() {
        let _ = remove_tree(&temporary, &AtomicBool::new(false));
    }
    result
}

fn snapshot_entry(root: &Path, path: &Path, metadata: &fs::Metadata) -> (Vec<u8>, u8, u64, i128) {
    let relative = path_key(path.strip_prefix(root).unwrap_or(path));
    let kind = if metadata.file_type().is_symlink() {
        2
    } else if metadata.is_dir() {
        1
    } else {
        0
    };
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |time| time.as_nanos() as i128);
    let length = if kind == 0 { metadata.len() } else { 0 };
    // Folder times change when entries are added, so only files and links compare times.
    (relative, kind, length, if kind == 1 { 0 } else { modified })
}

/// Size/mtime listing of a tree, without following links or reading file contents.
fn snapshot(root: &Path) -> Result<Snapshot, String> {
    let mut entries = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let metadata = path.symlink_metadata().map_err(|_| "Source changed")?;
        entries.push(snapshot_entry(root, &path, &metadata));
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            for entry in fs::read_dir(&path).map_err(|_| "Source changed")? {
                stack.push(entry.map_err(|_| "Source changed")?.path());
            }
        }
    }
    entries.sort();
    Ok(entries)
}

/// Copies a tree, recreating links instead of following them, and returns the snapshot of the
/// source as it was read. Files are synced to disk when the source will be deleted afterwards.
fn copy_tree(
    source: &Path,
    target: &Path,
    cancel: &AtomicBool,
    durable: bool,
) -> Result<Snapshot, String> {
    let mut entries = Vec::new();
    let mut stack = vec![(source.to_path_buf(), target.to_path_buf())];
    let mut buffer = vec![0u8; COPY_BUFFER_BYTES];
    while let Some((from, to)) = stack.pop() {
        if cancel.load(Ordering::Acquire) {
            return Err("Operation cancelled".into());
        }
        let metadata = from
            .symlink_metadata()
            .map_err(|_| "Source could not be read")?;
        entries.push(snapshot_entry(source, &from, &metadata));
        if metadata.file_type().is_symlink() {
            copy_link(&from, &to, &metadata)?;
        } else if metadata.is_dir() {
            fs::create_dir(&to).map_err(|_| "Destination folder could not be created")?;
            for entry in fs::read_dir(&from).map_err(|_| "Source folder could not be read")? {
                let entry = entry.map_err(|_| "Folder contents changed")?;
                stack.push((entry.path(), to.join(entry.file_name())));
            }
        } else if metadata.is_file() {
            copy_file(&from, &to, &metadata, &mut buffer, cancel, durable)?;
        } else {
            return Err("Special file types are not supported".into());
        }
    }
    entries.sort();
    Ok(entries)
}

fn open_no_follow(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
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
    options.open(path)
}

fn copy_file(
    from: &Path,
    to: &Path,
    metadata: &fs::Metadata,
    buffer: &mut [u8],
    cancel: &AtomicBool,
    durable: bool,
) -> Result<(), String> {
    let mut input = open_no_follow(from).map_err(|_| "Source could not be read")?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .map_err(|_| "Destination file could not be created")?;
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err("Operation cancelled".into());
        }
        let count = input
            .read(buffer)
            .map_err(|_| "Reading the source failed")?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(|_| "Write failed; check free disk space")?;
    }
    if durable {
        output
            .sync_all()
            .map_err(|_| "Copy was not flushed to disk")?;
    }
    // Best effort: FAT/exFAT and network shares may not keep times or permissions.
    if let Ok(modified) = metadata.modified() {
        let _ = output.set_modified(modified);
    }
    drop(output);
    let _ = fs::set_permissions(to, metadata.permissions());
    Ok(())
}

fn copy_link(from: &Path, to: &Path, metadata: &fs::Metadata) -> Result<(), String> {
    let link = fs::read_link(from).map_err(|_| "Symbolic link could not be read")?;
    #[cfg(unix)]
    let result = {
        let _ = metadata;
        std::os::unix::fs::symlink(&link, to)
    };
    #[cfg(windows)]
    let result = {
        use std::os::windows::fs::FileTypeExt;
        if metadata.file_type().is_symlink_dir() {
            std::os::windows::fs::symlink_dir(&link, to)
        } else {
            std::os::windows::fs::symlink_file(&link, to)
        }
    };
    result.map_err(|_| {
        if cfg!(windows) {
            "Symbolic links could not be copied (Windows requires Developer Mode or administrator rights)".to_string()
        } else {
            "Symbolic link could not be copied".to_string()
        }
    })
}

/// Content digest of a tree, used to verify app-trash entries before restoring them.
fn tree_digest(path: &Path, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
    let mut digest = Sha256::new();
    let mut stack = vec![path.to_path_buf()];
    let mut buffer = vec![0; COPY_BUFFER_BYTES];
    while let Some(current) = stack.pop() {
        if cancel.load(Ordering::Acquire) {
            return Err("Operation cancelled".into());
        }
        let metadata = current.symlink_metadata().map_err(|_| "Source changed")?;
        let relative = current
            .strip_prefix(path)
            .map_err(|_| "Verification path is invalid")?;
        let key = path_key(relative);
        digest.update((key.len() as u64).to_le_bytes());
        digest.update(key);
        if metadata.file_type().is_symlink() {
            digest.update([2]);
            let link = fs::read_link(&current).map_err(|_| "Source changed")?;
            digest.update(path_key(&link));
        } else if metadata.is_dir() {
            digest.update([1]);
            let mut children = fs::read_dir(current)
                .map_err(|_| "Folder verification failed")?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "Folder verification failed")?;
            children.sort();
            children.reverse();
            stack.extend(children);
        } else if metadata.is_file() {
            digest.update([0]);
            let mut file = open_no_follow(&current).map_err(|_| "File verification failed")?;
            loop {
                let count = file
                    .read(&mut buffer)
                    .map_err(|_| "File verification failed")?;
                if count == 0 {
                    break;
                }
                digest.update(&buffer[..count]);
            }
        } else {
            return Err("Special files cannot be verified".into());
        }
    }
    Ok(digest.finalize().to_vec())
}

/// Clears the read-only attribute and retries, like Explorer does (e.g. `.git/objects`).
fn retry_writable(
    path: &Path,
    remove: impl Fn(&Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    match remove(path) {
        #[cfg(windows)]
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            let metadata = path.symlink_metadata()?;
            let mut permissions = metadata.permissions();
            if !permissions.readonly() {
                return Err(error);
            }
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            fs::set_permissions(path, permissions)?;
            remove(path)
        }
        result => result,
    }
}

/// Deletes a tree without following links; a link to a folder is removed, not its contents.
fn remove_tree(path: &Path, cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        return Err("Operation cancelled".into());
    }
    let metadata = path
        .symlink_metadata()
        .map_err(|_| "Source is unavailable")?;
    if metadata.file_type().is_symlink() {
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileTypeExt;
            if metadata.file_type().is_symlink_dir() {
                return retry_writable(path, |path| fs::remove_dir(path))
                    .map_err(|_| "Link could not be deleted".into());
            }
        }
        return retry_writable(path, |path| fs::remove_file(path))
            .map_err(|_| "Link could not be deleted".into());
    }
    if metadata.is_file() {
        return retry_writable(path, |path| fs::remove_file(path))
            .map_err(|_| "File could not be deleted".into());
    }
    if !metadata.is_dir() {
        return Err("Special files cannot be deleted".into());
    }
    for entry in fs::read_dir(path).map_err(|_| "Folder could not be read")? {
        remove_tree(&entry.map_err(|_| "Folder changed")?.path(), cancel)?;
    }
    retry_writable(path, |path| fs::remove_dir(path))
        .map_err(|_| "Folder could not be deleted".into())
}

impl crate::shared::traits::FileOperationQueue for OperationService {
    fn submit(&self, request: OperationRequest) -> Result<(), String> {
        OperationService::submit(self, request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let root = std::env::temp_dir().join(format!("filemofm-ops-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn copy_tree_keeps_links_and_contents() {
        let root = scratch();
        let source = root.join("project");
        fs::create_dir_all(source.join("src")).unwrap();
        fs::write(source.join("src/main.rs"), b"fn main() {}").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("src/main.rs", source.join("link")).unwrap();
        let target = root.join("copy");
        transfer(
            &source,
            &target,
            false,
            &AtomicBool::new(false),
            ".tmp-test",
        )
        .unwrap();
        assert_eq!(
            fs::read(target.join("src/main.rs")).unwrap(),
            b"fn main() {}"
        );
        #[cfg(unix)]
        assert!(
            target
                .join("link")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(source.exists());
        assert!(!root.join(".tmp-test").exists());
        // Removing a link never touches its target.
        #[cfg(unix)]
        {
            remove_tree(&target.join("link"), &AtomicBool::new(false)).unwrap();
            assert!(target.join("src/main.rs").exists());
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn free_names_follow_explorer_conventions() {
        let root = scratch();
        fs::write(root.join("report.pdf"), b"x").unwrap();
        let copy = free_name(&root, std::ffi::OsStr::new("report.pdf"), " - Copy").unwrap();
        assert_eq!(copy, root.join("report - Copy.pdf"));
        fs::write(&copy, b"x").unwrap();
        let second = free_name(&root, std::ffi::OsStr::new("report.pdf"), " - Copy").unwrap();
        assert_eq!(second, root.join("report - Copy (2).pdf"));
        let numbered = free_name(&root, std::ffi::OsStr::new("report.pdf"), "").unwrap();
        assert_eq!(numbered, root.join("report (1).pdf"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn publish_never_overwrites() {
        let root = scratch();
        fs::write(root.join("a"), b"a").unwrap();
        fs::write(root.join("b"), b"b").unwrap();
        assert!(publish(&root.join("a"), &root.join("b")).is_err());
        assert_eq!(fs::read(root.join("b")).unwrap(), b"b");
        let _ = fs::remove_dir_all(root);
    }
}

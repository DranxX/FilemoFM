use crate::shared::constants::*;
use crate::shared::models::*;
use crate::shared::traits::{FileInspector, MetadataRepository, PathValidator};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

pub struct FileScannerService {
    validator: Arc<dyn PathValidator>,
    inspector: Arc<dyn FileInspector>,
    repository: Arc<dyn MetadataRepository>,
    sender: EventSink,
    generation: Arc<AtomicU64>,
    /// Serialises scans so a superseded scan can never run its "mark missing" phase after a
    /// newer scan of the same folder has already recorded freshly created files.
    lock: Arc<parking_lot::Mutex<()>>,
}
impl FileScannerService {
    pub fn new(
        validator: Arc<dyn PathValidator>,
        inspector: Arc<dyn FileInspector>,
        repository: Arc<dyn MetadataRepository>,
        sender: EventSink,
    ) -> Self {
        Self {
            validator,
            inspector,
            repository,
            sender,
            generation: Arc::new(AtomicU64::new(0)),
            lock: Arc::new(parking_lot::Mutex::new(())),
        }
    }
    pub fn scan_directory(&self, path: PathBuf) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let current = self.generation.clone();
        let validator = self.validator.clone();
        let inspector = self.inspector.clone();
        let repository = self.repository.clone();
        let sender = self.sender.clone();
        let lock = self.lock.clone();
        rayon::spawn(move || {
            let is_current = || current.load(Ordering::Acquire) == generation;
            let _guard = lock.lock();
            if !is_current() {
                return;
            }
            let result = (|| -> Result<(), String> {
                let path = if let Some(text) = path.to_str() {
                    validator
                        .validate(text)
                        .map_err(|error| error.to_string())?
                } else {
                    let canonical = crate::shared::utils::canonicalize(&path)
                        .map_err(|_| "Folder cannot be accessed")?;
                    if validator.is_excluded(&canonical) {
                        return Err("Location is outside the allowed scope".into());
                    }
                    canonical
                };
                let mut batch = Vec::new();
                let known = repository
                    .get_files_by_directory(&path)
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .map(|file| (file.path, (file.size, file.modified_at)))
                    .collect::<std::collections::HashMap<_, _>>();
                let mut seen = HashSet::new();
                let mut has_errors = false;
                for entry in std::fs::read_dir(&path).map_err(|_| "Folder cannot be accessed")? {
                    if !is_current() {
                        return Ok(());
                    }
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(_) => {
                            has_errors = true;
                            continue;
                        }
                    };
                    let path = entry.path();
                    if validator.is_excluded(&path) {
                        continue;
                    }
                    seen.insert(path.clone());
                    let Some(file) = inspector.inspect(&path) else {
                        has_errors = true;
                        continue;
                    };
                    if known.get(&file.path) != Some(&(file.size, file.modified_at)) {
                        repository
                            .invalidate_sizes(&file.path)
                            .map_err(|error| error.to_string())?;
                    }
                    batch.push(file);
                    if batch.len() >= SCAN_BATCH_SIZE {
                        repository
                            .upsert_files(&mut batch)
                            .map_err(|error| error.to_string())?;
                        if sender
                            .send(BackgroundMessage::ScanBatch {
                                generation,
                                files: std::mem::take(&mut batch),
                                is_complete: false,
                            })
                            .is_err()
                        {
                            return Ok(());
                        }
                    }
                }
                if !is_current() {
                    return Ok(());
                }
                repository
                    .upsert_files(&mut batch)
                    .map_err(|error| error.to_string())?;
                if !has_errors {
                    for file in repository
                        .get_files_by_directory(&path)
                        .map_err(|error| error.to_string())?
                    {
                        if !is_current() {
                            return Ok(());
                        }
                        if !seen.contains(&file.path) {
                            repository
                                .mark_missing(&file.path)
                                .map_err(|error| error.to_string())?;
                            repository
                                .invalidate_sizes(&file.path)
                                .map_err(|error| error.to_string())?;
                        }
                    }
                }
                let _ = sender.send(BackgroundMessage::ScanBatch {
                    generation,
                    files: batch,
                    is_complete: true,
                });
                if has_errors {
                    return Err("Some items could not be read".into());
                }
                Ok(())
            })();
            if let Err(error) = result {
                let _ = sender.send(BackgroundMessage::ScanError { generation, error });
            }
        });
        generation
    }
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}
impl Drop for FileScannerService {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn start_file_watcher(path: PathBuf, sender: EventSink) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher;
    let directory = path.clone();
    let revision = Arc::new(AtomicU64::new(0));
    let pending = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut watcher =
        notify::recommended_watcher(move |result: Result<notify::Event, notify::Error>| {
            if result
                .as_ref()
                .is_ok_and(|event| matches!(event.kind, notify::EventKind::Access(_)))
            {
                return;
            }
            revision.fetch_add(1, Ordering::AcqRel);
            if pending.swap(true, Ordering::AcqRel) {
                return;
            }
            let revision = revision.clone();
            let pending = pending.clone();
            let sender = sender.clone();
            let directory = directory.clone();
            std::thread::spawn(move || {
                let started = Instant::now();
                loop {
                    let before = revision.load(Ordering::Acquire);
                    std::thread::sleep(Duration::from_millis(WATCHER_DEBOUNCE_MS));
                    if revision.load(Ordering::Acquire) != before
                        && started.elapsed() < Duration::from_millis(WATCHER_DEBOUNCE_MS * 4)
                    {
                        continue;
                    }
                    pending.store(false, Ordering::Release);
                    let _ = sender.try_send(BackgroundMessage::DirectoryChanged(directory));
                    break;
                }
            });
        })
        .ok()?;
    watcher
        .watch(&path, notify::RecursiveMode::NonRecursive)
        .ok()?;
    Some(watcher)
}

/// Background folder sizes. Several workers share one queue so the visible folders are
/// measured in parallel; each walk is bounded by the entry and time budgets in Settings.
pub struct FolderSizeService {
    requests: crossbeam_channel::Sender<(PathBuf, u64)>,
    generation: Arc<AtomicU64>,
}
impl FolderSizeService {
    pub fn new(
        repository: Arc<dyn MetadataRepository>,
        sender: EventSink,
        config: ScannerConfig,
        validator: Arc<dyn PathValidator>,
    ) -> Self {
        let (requests, receiver) = crossbeam_channel::bounded::<(PathBuf, u64)>(CHANNEL_CAPACITY);
        let generation = Arc::new(AtomicU64::new(0));
        let workers = std::thread::available_parallelism()
            .map_or(2, |count| count.get())
            .clamp(1, SIZE_WORKERS);
        for _ in 0..workers {
            let receiver = receiver.clone();
            let current = generation.clone();
            let repository = repository.clone();
            let sender = sender.clone();
            let config = config.clone();
            let validator = validator.clone();
            std::thread::spawn(move || {
                while let Ok((path, generation)) = receiver.recv() {
                    if current.load(Ordering::Acquire) != generation {
                        continue;
                    }
                    if let Ok(Some(size)) = repository.get_size(&path) {
                        let is_complete = size.state == SizeState::Complete;
                        let _ =
                            sender.send(BackgroundMessage::FolderSizeReady { generation, size });
                        if is_complete {
                            continue;
                        }
                    }
                    let is_current = || current.load(Ordering::Acquire) == generation;
                    let size = measure(&path, &config, validator.as_ref(), &is_current);
                    if is_current() {
                        let _ = repository.save_size(&size);
                        let _ =
                            sender.send(BackgroundMessage::FolderSizeReady { generation, size });
                    }
                }
            });
        }
        Self {
            requests,
            generation,
        }
    }
    pub fn reset(&self, generation: u64) {
        self.generation.store(generation, Ordering::Release);
    }
    pub fn request(&self, path: PathBuf, generation: u64) -> bool {
        self.requests.try_send((path, generation)).is_ok()
    }
}
impl Drop for FolderSizeService {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}

/// Depth-first logical size of `root`. Links, reparse points, other filesystems and excluded
/// paths are not entered; skipping any of them, hitting a budget or an unreadable folder makes
/// the result Partial rather than silently wrong.
fn measure(
    root: &Path,
    config: &ScannerConfig,
    validator: &dyn PathValidator,
    is_current: &dyn Fn() -> bool,
) -> FolderSize {
    let mut size = FolderSize {
        path: root.to_path_buf(),
        bytes: 0,
        state: SizeState::Complete,
        measured_at: chrono::Utc::now().timestamp(),
    };
    let started = Instant::now();
    let budget = Duration::from_secs(config.size_time_budget_seconds);
    #[cfg(unix)]
    let device = {
        use std::os::unix::fs::MetadataExt;
        root.metadata().map(|metadata| metadata.dev()).ok()
    };
    let mut stack = vec![root.to_path_buf()];
    let mut entries = 0usize;
    while let Some(directory) = stack.pop() {
        if !is_current() {
            break;
        }
        let Ok(children) = std::fs::read_dir(&directory) else {
            size.state = SizeState::Partial;
            continue;
        };
        for child in children {
            entries += 1;
            if entries > config.size_entry_budget
                || (entries.is_multiple_of(4096) && started.elapsed() > budget)
            {
                size.state = SizeState::Partial;
                return size;
            }
            // On Windows this metadata comes from the directory listing itself (no extra
            // syscall); on Unix it is a single lstat, never following links.
            let Some((child, metadata)) = child
                .ok()
                .and_then(|entry| entry.metadata().ok().map(|metadata| (entry, metadata)))
            else {
                size.state = SizeState::Partial;
                continue;
            };
            if metadata.file_type().is_symlink() {
                continue;
            }
            if !metadata.is_dir() {
                size.bytes = size.bytes.saturating_add(metadata.len());
                continue;
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                const REPARSE_POINT: u32 = 0x400;
                // Junctions and volume mount points are reparse points.
                if metadata.file_attributes() & REPARSE_POINT != 0 {
                    size.state = SizeState::Partial;
                    continue;
                }
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if Some(metadata.dev()) != device {
                    size.state = SizeState::Partial;
                    continue;
                }
            }
            let child = child.path();
            if validator.is_excluded(&child) {
                size.state = SizeState::Partial;
                continue;
            }
            stack.push(child);
        }
    }
    if !is_current() {
        size.state = SizeState::Partial;
    }
    size
}

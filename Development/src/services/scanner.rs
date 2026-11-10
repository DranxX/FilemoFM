use crate::shared::constants::*;
use crate::shared::models::*;
use crate::shared::traits::{FileInspector, MetadataRepository, PathValidator};
use crossbeam_deque::{Steal, Stealer, Worker};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
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

/// Logical size of `root`. The walk is a parallel work-stealing traversal: per-worker LIFO
/// deques with batch stealing keep depth memory-bounded while wide subtrees spread across
/// workers. Links, reparse points, other filesystems and excluded paths are not entered;
/// skipping any of them, hitting a budget, an unreadable folder or losing the active
/// generation makes the result Partial rather than silently wrong.
fn measure(
    root: &Path,
    config: &ScannerConfig,
    validator: &dyn PathValidator,
    is_current: &(dyn Fn() -> bool + Send + Sync),
) -> FolderSize {
    let threads = std::thread::available_parallelism()
        .map_or(1, |count| count.get())
        .clamp(1, SIZE_WALK_THREADS);
    measure_with(root, config, validator, is_current, threads)
}

fn measure_with(
    root: &Path,
    config: &ScannerConfig,
    validator: &dyn PathValidator,
    is_current: &(dyn Fn() -> bool + Send + Sync),
    threads: usize,
) -> FolderSize {
    let mut size = FolderSize {
        path: root.to_path_buf(),
        bytes: 0,
        state: SizeState::Complete,
        measured_at: chrono::Utc::now().timestamp(),
    };
    #[cfg(unix)]
    let context = WalkContext {
        config,
        validator,
        budget: Duration::from_secs(config.size_time_budget_seconds),
        is_current,
        device: root.metadata().ok().map(|metadata| {
            use std::os::unix::fs::MetadataExt;
            metadata.dev()
        }),
    };
    #[cfg(not(unix))]
    let context = WalkContext {
        config,
        validator,
        budget: Duration::from_secs(config.size_time_budget_seconds),
        is_current,
    };
    let shared = Arc::new(WalkShared {
        bytes: AtomicU64::new(0),
        pending: AtomicUsize::new(1),
        entries: AtomicU64::new(0),
        partial: AtomicBool::new(false),
        done: AtomicBool::new(false),
        started: Instant::now(),
    });
    let deques: Vec<Worker<PathBuf>> = (0..threads).map(|_| Worker::new_lifo()).collect();
    let stealers: Vec<Stealer<PathBuf>> = deques.iter().map(Worker::stealer).collect();
    deques[0].push(root.to_path_buf());
    std::thread::scope(|scope| {
        let context = &context;
        for (index, deque) in deques.into_iter().enumerate() {
            let shared = shared.clone();
            let stealers = &stealers;
            scope.spawn(move || {
                while !shared.done.load(Ordering::Acquire) {
                    match take_work(&deque, stealers, index) {
                        Some(directory) => process_dir(&directory, &shared, &deque, context),
                        None => std::thread::sleep(Duration::from_millis(1)),
                    }
                }
            });
        }
    });
    size.bytes = shared.bytes.load(Ordering::Relaxed);
    if shared.partial.load(Ordering::Relaxed) || !is_current() {
        size.state = SizeState::Partial;
    }
    size
}

/// Shared accumulators for one parallel folder-size walk.
struct WalkShared {
    bytes: AtomicU64,
    /// Live work items, whether queued in a deque or held by a worker. Reaching zero proves
    /// no directory anywhere is unprocessed, so termination never drops queued work.
    pending: AtomicUsize,
    entries: AtomicU64,
    partial: AtomicBool,
    done: AtomicBool,
    started: Instant,
}
impl WalkShared {
    /// Accounts one child against the entry and time budgets and the active generation.
    /// Returns false when the whole walk must stop; the result is Partial either way.
    fn charge(
        &self,
        entry_budget: usize,
        time_budget: Duration,
        is_current: &(dyn Fn() -> bool + Send + Sync),
    ) -> bool {
        let entries = self.entries.fetch_add(1, Ordering::Relaxed) + 1;
        if entries > entry_budget as u64
            || (entries.is_multiple_of(4096) && self.started.elapsed() > time_budget)
            || !is_current()
        {
            self.partial.store(true, Ordering::Release);
            self.done.store(true, Ordering::Release);
            return false;
        }
        true
    }
}

/// Pops the local LIFO deque first, then batch-steals from the other workers in round-robin
/// order starting at the next index; moving half a victim's queue amortizes steal contention.
fn take_work(
    deque: &Worker<PathBuf>,
    stealers: &[Stealer<PathBuf>],
    index: usize,
) -> Option<PathBuf> {
    if let Some(directory) = deque.pop() {
        return Some(directory);
    }
    for offset in 1..stealers.len() {
        let stealer = &stealers[(index + offset) % stealers.len()];
        loop {
            match stealer.steal_batch_and_pop(deque) {
                Steal::Success(directory) => return Some(directory),
                Steal::Empty => break,
                Steal::Retry => continue,
            }
        }
    }
    None
}

/// Immutable per-walk parameters shared by every worker thread.
struct WalkContext<'a> {
    config: &'a ScannerConfig,
    validator: &'a dyn PathValidator,
    budget: Duration,
    is_current: &'a (dyn Fn() -> bool + Send + Sync),
    #[cfg(unix)]
    device: Option<u64>,
}

/// Processes one popped work item. Every exit path releases the item from `pending`, so the
/// counter reaching zero always proves every discovered directory was handled.
fn process_dir(
    directory: &Path,
    shared: &WalkShared,
    deque: &Worker<PathBuf>,
    context: &WalkContext,
) {
    if (context.is_current)() {
        walk_dir(directory, shared, deque, context);
    } else {
        shared.partial.store(true, Ordering::Release);
        shared.done.store(true, Ordering::Release);
    }
    if shared.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
        shared.done.store(true, Ordering::Release);
    }
}

fn walk_dir(directory: &Path, shared: &WalkShared, deque: &Worker<PathBuf>, context: &WalkContext) {
    let Ok(children) = std::fs::read_dir(directory) else {
        shared.partial.store(true, Ordering::Release);
        return;
    };
    for child in children {
        if !shared.charge(
            context.config.size_entry_budget,
            context.budget,
            context.is_current,
        ) {
            break;
        }
        // On Windows this metadata comes from the directory listing itself (no extra
        // syscall); on Unix it is a single lstat, never following links.
        let Some((child, metadata)) = child
            .ok()
            .and_then(|entry| entry.metadata().ok().map(|metadata| (entry, metadata)))
        else {
            shared.partial.store(true, Ordering::Release);
            continue;
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if !metadata.is_dir() {
            shared.bytes.fetch_add(metadata.len(), Ordering::Relaxed);
            continue;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const REPARSE_POINT: u32 = 0x400;
            // Junctions and volume mount points are reparse points.
            if metadata.file_attributes() & REPARSE_POINT != 0 {
                shared.partial.store(true, Ordering::Release);
                continue;
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if Some(metadata.dev()) != context.device {
                shared.partial.store(true, Ordering::Release);
                continue;
            }
        }
        let child = child.path();
        if context.validator.is_excluded(&child) {
            shared.partial.store(true, Ordering::Release);
            continue;
        }
        // Children are counted before the parent is released, so `pending` can never
        // transiently reach zero while a discovered subtree is still queued.
        shared.pending.fetch_add(1, Ordering::AcqRel);
        deque.push(child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::errors::ScanError;
    use std::fs;

    struct AllowAll;
    impl PathValidator for AllowAll {
        fn set_exclusions(&self, _paths: Vec<String>) {}
        fn validate(&self, path: &str) -> Result<PathBuf, ScanError> {
            Ok(PathBuf::from(path))
        }
        fn is_excluded(&self, _path: &Path) -> bool {
            false
        }
    }

    fn config(entry_budget: usize, seconds: u64) -> ScannerConfig {
        ScannerConfig {
            watch_enabled: false,
            excluded_paths: Vec::new(),
            size_entry_budget: entry_budget,
            size_time_budget_seconds: seconds,
        }
    }

    fn temp_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("filemofm-size-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    /// a/1.bin(100) a/b/2.bin(250) a/b/c/3.bin(50) top.bin(1000) empty/
    fn sample_tree(root: &Path) {
        fs::create_dir_all(root.join("a/b/c")).unwrap();
        fs::create_dir_all(root.join("empty")).unwrap();
        fs::write(root.join("a/1.bin"), vec![0u8; 100]).unwrap();
        fs::write(root.join("a/b/2.bin"), vec![0u8; 250]).unwrap();
        fs::write(root.join("a/b/c/3.bin"), vec![0u8; 50]).unwrap();
        fs::write(root.join("top.bin"), vec![0u8; 1000]).unwrap();
    }

    #[test]
    fn parallel_walk_counts_every_file() {
        let root = temp_root();
        sample_tree(&root);
        for threads in [1usize, 2, 4, 8] {
            let size = measure_with(&root, &config(1_000_000, 60), &AllowAll, &|| true, threads);
            assert_eq!(size.bytes, 1400, "threads={threads}");
            assert!(
                matches!(size.state, SizeState::Complete),
                "threads={threads}"
            );
        }
        fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_never_followed() {
        let root = temp_root();
        sample_tree(&root);
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
        let size = measure_with(&root, &config(1_000_000, 60), &AllowAll, &|| true, 4);
        assert_eq!(size.bytes, 1400);
        assert!(matches!(size.state, SizeState::Complete));
        fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_folder_is_partial() {
        let root = temp_root();
        sample_tree(&root);
        let secret = root.join("secret");
        fs::create_dir_all(&secret).unwrap();
        fs::write(secret.join("huge.bin"), vec![0u8; 4096]).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read_dir(&secret).is_ok() {
            // Privileges ignore the permission bits; the partial path cannot be exercised.
            fs::set_permissions(&secret, fs::Permissions::from_mode(0o755)).unwrap();
            fs::remove_dir_all(&root).ok();
            return;
        }
        let size = measure_with(&root, &config(1_000_000, 60), &AllowAll, &|| true, 4);
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(size.state, SizeState::Partial));
        assert_eq!(size.bytes, 1400);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn entry_budget_stops_the_walk_as_partial() {
        let root = temp_root();
        sample_tree(&root);
        let size = measure_with(&root, &config(2, 60), &AllowAll, &|| true, 4);
        assert!(matches!(size.state, SizeState::Partial));
        assert!(size.bytes < 1400);
        fs::remove_dir_all(&root).ok();
    }
}

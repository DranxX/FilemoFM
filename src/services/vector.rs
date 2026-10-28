use crate::shared::constants::*;
use crate::shared::models::*;
use crate::shared::traits::*;
use crossbeam_channel::{Sender, bounded};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

pub enum VectorCommand {
    Configure(AppConfig),
    Synchronize,
    Load,
    Unload,
    Rebuild(EmbeddingConfig, VerifiedModel),
    Rollback,
    Search(SearchQuery, Sender<Result<Vec<SearchResult>, String>>),
    Shutdown,
}

pub struct VectorService {
    commands: Sender<VectorCommand>,
    /// Set while a rebuild owns the worker, so searches fail fast instead of waiting minutes.
    busy: Arc<AtomicBool>,
    cancellation: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    worker: parking_lot::Mutex<Option<std::thread::JoinHandle<()>>>,
}
struct VectorWorker {
    repository: Arc<dyn MetadataRepository>,
    models: Arc<dyn ModelRepository>,
    encoders: Arc<dyn EncoderFactory>,
    backends: Arc<dyn VectorBackendFactory>,
    config: AppConfig,
    sender: EventSink,
    cancellation: Arc<AtomicBool>,
    backend: Option<Box<dyn VectorDataset>>,
    encoder: Option<Box<dyn TextEncoder>>,
    manifest: Option<DatasetManifest>,
    last_inference: Instant,
    last_backend_attempt: Instant,
    failures: HashMap<String, usize>,
    error: Option<String>,
    is_paused: bool,
    is_manual_sync: bool,
}
impl VectorService {
    pub fn new(
        repository: Arc<dyn MetadataRepository>,
        models: Arc<dyn ModelRepository>,
        encoders: Arc<dyn EncoderFactory>,
        backends: Arc<dyn VectorBackendFactory>,
        config: AppConfig,
        sender: EventSink,
    ) -> Self {
        let (commands, receiver) = bounded(CHANNEL_CAPACITY);
        let cancellation = Arc::new(AtomicBool::new(false));
        let cancel = cancellation.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let stopping = shutdown.clone();
        let busy = Arc::new(AtomicBool::new(false));
        let rebuilding = busy.clone();
        let worker = std::thread::spawn(move || {
            let manifest = repository.dataset().ok().flatten();
            let mut worker = VectorWorker {
                repository,
                models,
                encoders,
                backends,
                config,
                sender,
                cancellation: cancel,
                backend: None,
                encoder: None,
                manifest,
                last_inference: Instant::now(),
                last_backend_attempt: Instant::now() - Duration::from_secs(60),
                failures: HashMap::new(),
                error: None,
                is_paused: false,
                is_manual_sync: false,
            };
            if let Some(manifest) = &worker.manifest {
                let mut active = worker.config.embedding.clone();
                active.dimension = manifest.dimension;
                if let Some(model) = &manifest.model {
                    active.repo_url = format!("https://huggingface.co/{}", model.artifact.repo);
                }
                worker.config.embedding = active.clone();
                let _ = worker
                    .sender
                    .send(BackgroundMessage::DatasetActivated { config: active });
            }
            let mut last_status = Instant::now() - Duration::from_secs(60);
            let mut has_backlog = false;
            loop {
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                // With a sync backlog, only poll for commands between batches.
                let command = receiver.recv_timeout(if has_backlog {
                    Duration::ZERO
                } else {
                    Duration::from_millis(250)
                });
                match command {
                    Ok(VectorCommand::Shutdown)
                    | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    Ok(VectorCommand::Configure(config)) => {
                        worker.config = config;
                        if !worker.config.qdrant.enabled {
                            worker.encoder = None;
                            worker.backend = None;
                        }
                        worker.last_backend_attempt = Instant::now() - Duration::from_secs(60);
                    }
                    Ok(VectorCommand::Synchronize) => {
                        worker.is_manual_sync = true;
                        worker.failures.clear();
                        worker.is_paused = false;
                        worker.error = None;
                        worker.cancellation.store(false, Ordering::Release);
                    }
                    Ok(VectorCommand::Load) => {
                        worker.cancellation.store(false, Ordering::Release);
                        if let Err(error) = worker.load_encoder() {
                            worker.error = Some(error);
                        }
                    }
                    Ok(VectorCommand::Unload) => {
                        worker.encoder = None;
                        worker.is_paused = true;
                    }
                    Ok(VectorCommand::Rebuild(config, model)) => {
                        worker.cancellation.store(false, Ordering::Release);
                        rebuilding.store(true, Ordering::Release);
                        if let Err(error) = worker.rebuild(config, model) {
                            worker.error = Some(error);
                        }
                        rebuilding.store(false, Ordering::Release);
                    }
                    Ok(VectorCommand::Rollback) => {
                        worker.cancellation.store(false, Ordering::Release);
                        let previous = worker
                            .manifest
                            .as_ref()
                            .and_then(|manifest| manifest.rollback.clone());
                        rebuilding.store(true, Ordering::Release);
                        let result = (|| {
                            let previous = previous.ok_or("No rollback generation is stored")?;
                            let model = previous
                                .model
                                .ok_or("Rollback model artifact is unavailable")?;
                            let mut config = worker.config.embedding.clone();
                            config.dimension = previous.dimension;
                            config.repo_url =
                                format!("https://huggingface.co/{}", model.artifact.repo);
                            worker.rebuild(config, model)
                        })();
                        rebuilding.store(false, Ordering::Release);
                        if let Err(error) = result {
                            worker.error = Some(error);
                        }
                    }
                    Ok(VectorCommand::Search(query, response)) => {
                        worker.cancellation.store(false, Ordering::Release);
                        let result = worker.search(query);
                        let _ = response.send(result);
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                }
                if worker.cancellation.load(Ordering::Acquire) {
                    worker.is_paused = true;
                }
                if worker
                    .backend
                    .as_mut()
                    .is_some_and(|backend| !backend.is_available())
                {
                    worker.backend = None;
                    worker.encoder = None;
                    worker.error = Some("Qdrant is offline; keyword search still works".into());
                }
                if worker.config.qdrant.enabled
                    && worker.backend.is_none()
                    && worker.last_backend_attempt.elapsed() > Duration::from_secs(30)
                {
                    worker.last_backend_attempt = Instant::now();
                    match worker.backends.start() {
                        Ok(backend) => {
                            worker.backend = Some(backend);
                            worker.error = None;
                        }
                        Err(error) => worker.error = Some(error),
                    }
                }
                has_backlog = false;
                if worker.config.qdrant.enabled
                    && (worker.config.qdrant.auto_sync || worker.is_manual_sync)
                    && !worker.is_paused
                    && worker.backend.is_some()
                    && worker.manifest.is_some()
                {
                    // Work in short time slices so commands (search, cancel) stay responsive.
                    const SLICE: Duration = Duration::from_millis(200);
                    const BATCH: usize = 64;
                    if let Ok(items) = worker.repository.pending(BATCH) {
                        if items.is_empty() {
                            worker.is_manual_sync = false;
                        }
                        let started = Instant::now();
                        let mut eligible = 0usize;
                        for item in items {
                            if worker.failures.get(&item.id).copied().unwrap_or(0) >= 3 {
                                continue;
                            }
                            eligible += 1;
                            if started.elapsed() > SLICE
                                || worker.cancellation.load(Ordering::Acquire)
                            {
                                break;
                            }
                            if let Err(error) = worker.sync_item(&item) {
                                *worker.failures.entry(item.id).or_default() += 1;
                                worker.error = Some(error);
                            }
                        }
                        has_backlog = eligible > 0;
                    }
                }
                if worker.encoder.is_some()
                    && worker.last_inference.elapsed()
                        > Duration::from_secs(worker.config.embedding.idle_seconds)
                {
                    worker.encoder = None;
                }
                if last_status.elapsed() > Duration::from_secs(1) {
                    worker.status();
                    last_status = Instant::now();
                }
            }
        });
        Self {
            commands,
            busy,
            cancellation,
            shutdown,
            worker: parking_lot::Mutex::new(Some(worker)),
        }
    }
    pub fn command(&self, command: VectorCommand) -> Result<(), String> {
        if self.shutdown.load(Ordering::Acquire) {
            return Err("Vector worker stopped".into());
        }
        self.commands
            .try_send(command)
            .map_err(|_| "Vector queue is full".into())
    }
    pub fn cancel(&self) {
        self.cancellation.store(true, Ordering::Release);
    }
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.cancel();
        let _ = self.commands.try_send(VectorCommand::Shutdown);
        if let Some(worker) = self.worker.lock().take() {
            let _ = worker.join();
        }
    }
}
impl VectorSearch for VectorService {
    fn search(&self, query: SearchQuery) -> Result<Vec<SearchResult>, String> {
        if self.busy.load(Ordering::Acquire) {
            return Err("The vector index is being rebuilt; semantic search is paused".into());
        }
        let (sender, receiver) = bounded(1);
        self.command(VectorCommand::Search(query, sender))?;
        receiver
            .recv_timeout(Duration::from_secs(180))
            .map_err(|_| "Semantic search timed out")?
    }
}
impl Drop for VectorService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl VectorWorker {
    fn status(&self) {
        let pending = self.repository.pending_count().unwrap_or(0);
        let model = if self.encoder.is_some() {
            "Idle".to_string()
        } else if self.models.active().ok().flatten().is_some() {
            "Sleeping".into()
        } else {
            "Not downloaded".into()
        };
        let model = if let Some(manifest) = &self.manifest {
            format!("{model} · {} dimensions", manifest.dimension)
        } else {
            format!("{model} · index not built")
        };
        let _ = self.sender.try_send(BackgroundMessage::VectorStatus {
            backend: if !self.config.qdrant.enabled {
                ServiceStatus::Disabled
            } else if self.backend.is_some() {
                ServiceStatus::Online
            } else {
                ServiceStatus::Offline
            },
            model,
            pending,
            error: self.error.clone(),
        });
    }
    fn load_encoder(&mut self) -> Result<(), String> {
        if !self.config.qdrant.enabled {
            return Err("Vector search is disabled".into());
        }
        if self.encoder.is_some() {
            return Ok(());
        }
        let model = self
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.model.clone())
            .or(self.models.active()?)
            .ok_or("Download and verify a model in Settings › Index & vector")?;
        let mut config = self.config.embedding.clone();
        if let Some(manifest) = &self.manifest {
            config.dimension = manifest.dimension;
        }
        let _ = self.sender.send(BackgroundMessage::VectorStatus {
            backend: if self.backend.is_some() {
                ServiceStatus::Online
            } else {
                ServiceStatus::Offline
            },
            model: "Loading".into(),
            pending: 0,
            error: None,
        });
        let encoder = self.encoders.load(&model, &config)?;
        if self
            .manifest
            .as_ref()
            .is_some_and(|manifest| manifest.fingerprint != encoder.fingerprint())
        {
            return Err("Model does not match the active dataset; review a rebuild".into());
        }
        self.encoder = Some(encoder);
        self.last_inference = Instant::now();
        Ok(())
    }
    fn normalized(vector: Vec<f32>, dimension: usize) -> Result<Vec<f32>, String> {
        if vector.len() != DEFAULT_DIMENSIONS
            || !(MIN_QWEN_DIMENSIONS..=DEFAULT_DIMENSIONS).contains(&dimension)
        {
            return Err("Embedding dimension is invalid".into());
        }
        let mut vector = vector[..dimension].to_vec();
        let norm = vector
            .iter()
            .map(|value| (*value as f64).powi(2))
            .sum::<f64>()
            .sqrt();
        if !norm.is_finite() || norm <= f64::EPSILON {
            return Err("Embedding norm is invalid".into());
        }
        for value in &mut vector {
            *value = (*value as f64 / norm) as f32;
        }
        Ok(vector)
    }
    fn document(&self, file: &FileItem) -> Result<String, String> {
        let mut tags = file
            .tags
            .iter()
            .map(|tag| tag.name.as_str())
            .collect::<Vec<_>>();
        tags.sort();
        let description = self
            .repository
            .resolve_description(&file.description)
            .map_err(|error| error.to_string())?;
        let plain = pulldown_cmark::Parser::new(&description)
            .filter_map(|event| match event {
                pulldown_cmark::Event::Text(text) | pulldown_cmark::Event::Code(text) => {
                    Some(text.into_string())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        Ok(format!(
            "{}\n{}\n{}\n{}",
            file.name,
            file.path.display(),
            tags.join(", "),
            plain
        ))
    }
    fn chunks(text: &str, config: &EmbeddingConfig) -> Vec<String> {
        let size = (config.context_tokens / 4).clamp(64, 512);
        let chars = text.chars().collect::<Vec<_>>();
        chars
            .chunks(size)
            .map(|chunk| chunk.iter().collect())
            .collect()
    }
    fn write_file(
        &mut self,
        file: &FileItem,
        name: &str,
        dimension: usize,
    ) -> Result<usize, String> {
        self.load_encoder()?;
        let document = self.document(file)?;
        let hash = format!("{:x}", Sha256::digest(document.as_bytes()));
        let chunks = Self::chunks(&document, &self.config.embedding);
        self.repository
            .clear_projection(name, Some(&file.id))
            .map_err(|error| error.to_string())?;
        self.backend
            .as_mut()
            .ok_or("Qdrant is unavailable")?
            .delete(name, &file.id)?;
        for (index, text) in chunks.iter().enumerate() {
            if self.cancellation.load(Ordering::Acquire) {
                return Err("Synchronization paused".into());
            }
            let vector = self
                .encoder
                .as_mut()
                .ok_or("Model is not loaded")?
                .embed(text, false)?;
            let vector = Self::normalized(vector, dimension)?;
            self.last_inference = Instant::now();
            if self
                .repository
                .get_file(&file.id)
                .map_err(|error| error.to_string())?
                .is_none_or(|current| current.revision != file.revision || current.is_missing)
            {
                return Err("Metadata changed; the job was rescheduled".into());
            }
            self.backend
                .as_mut()
                .ok_or("Qdrant is unavailable")?
                .upsert(name, &file.id, index, vector, file.revision)?;
        }
        self.repository
            .save_projection(name, &file.id, &hash)
            .map_err(|error| error.to_string())?;
        Ok(chunks.len())
    }
    fn sync_item(&mut self, item: &OutboxItem) -> Result<(), String> {
        let manifest = self.manifest.clone().ok_or("Index is not built")?;
        if self
            .backend
            .as_mut()
            .ok_or("Qdrant is unavailable")?
            .ensure(&manifest.active, manifest.dimension)?
        {
            self.repository
                .clear_projection(&manifest.active, None)
                .map_err(|error| error.to_string())?;
            self.repository
                .enqueue_all()
                .map_err(|error| error.to_string())?;
        }
        if item.is_deleted {
            self.repository
                .clear_projection(&manifest.active, Some(&item.id))
                .map_err(|error| error.to_string())?;
            self.backend
                .as_mut()
                .ok_or("Qdrant is unavailable")?
                .delete(&manifest.active, &item.id)?;
        } else {
            let file = self
                .repository
                .get_file(&item.id)
                .map_err(|error| error.to_string())?
                .ok_or("Metadata is not available yet")?;
            if file.revision != item.revision {
                // Counted as a retry so a stale row is parked instead of spinning the worker.
                return Err("Metadata changed; the job was rescheduled".into());
            }
            let hash = format!("{:x}", Sha256::digest(self.document(&file)?.as_bytes()));
            if self
                .repository
                .projection(&manifest.active, &file.id)
                .map_err(|error| error.to_string())?
                .as_ref()
                == Some(&hash)
            {
                self.backend
                    .as_mut()
                    .ok_or("Qdrant is unavailable")?
                    .update_revision(&manifest.active, &file.id, file.revision)?;
            } else {
                self.write_file(&file, &manifest.active, manifest.dimension)?;
            }
        }
        self.repository
            .acknowledge(&item.id, item.revision)
            .map_err(|error| error.to_string())?;
        Ok(())
    }
    fn search(&mut self, query: SearchQuery) -> Result<Vec<SearchResult>, String> {
        let manifest = self
            .manifest
            .clone()
            .ok_or("Semantic index is not built; open Settings › Index & vector")?;
        if self.backend.is_none() {
            return Err("Qdrant is unavailable".into());
        }
        self.load_encoder()?;
        let vector = self
            .encoder
            .as_mut()
            .ok_or("Model is not loaded")?
            .embed(&query.text, true)?;
        self.last_inference = Instant::now();
        let vector = Self::normalized(vector, manifest.dimension)?;
        let mut filter = query.clone();
        filter.text.clear();
        filter.limit = MAX_RESULTS;
        filter.offset = 0;
        let mut candidates = Vec::new();
        loop {
            let batch = self
                .repository
                .search_keyword(&filter)
                .map_err(|error| error.to_string())?;
            let count = batch.len();
            candidates.extend(batch.into_iter().map(|hit| hit.file));
            if count < MAX_RESULTS {
                break;
            }
            filter.offset += MAX_RESULTS;
            if candidates.len() > 100000 {
                return Err("Narrow the semantic scope to a smaller root".into());
            }
        }
        let mut hits = Vec::new();
        for batch in candidates.chunks(MAX_RESULTS) {
            let ids = batch.iter().map(|file| file.id.clone()).collect::<Vec<_>>();
            hits.extend(
                self.backend
                    .as_mut()
                    .ok_or("Qdrant is unavailable")?
                    .search(&manifest.active, vector.clone(), &ids, MAX_RESULTS)?,
            );
        }
        hits.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut seen = HashSet::new();
        let mut results = Vec::new();
        for (id, score, revision) in hits {
            if seen.contains(&id) {
                continue;
            }
            let Some(file) = self
                .repository
                .get_file(&id)
                .map_err(|error| error.to_string())?
            else {
                continue;
            };
            if file.is_missing || file.revision != revision || !file.path.exists() {
                continue;
            }
            seen.insert(id);
            results.push(SearchResult { file, score });
            if results.len() >= query.limit {
                break;
            }
        }
        Ok(results)
    }
    fn rebuild(&mut self, config: EmbeddingConfig, model: VerifiedModel) -> Result<(), String> {
        if !(MIN_QWEN_DIMENSIONS..=DEFAULT_DIMENSIONS).contains(&config.dimension) {
            return Err("Model dimension is invalid".into());
        }
        if self.backend.is_none() {
            self.backend = Some(self.backends.start()?);
        }
        let old_manifest = self.manifest.clone();
        let old_config = self.config.embedding.clone();
        let old_model = old_manifest
            .as_ref()
            .and_then(|manifest| manifest.model.clone())
            .or(self.models.active()?);
        self.encoder = None;
        let result = (|| {
            let mut encoder = self.encoders.load(&model, &config)?;
            let fingerprint = encoder.fingerprint();
            Self::normalized(
                encoder.embed("FilemoFM retrieval capability verification", true)?,
                config.dimension,
            )?;
            let saved = self
                .repository
                .migration()
                .map_err(|error| error.to_string())?;
            let mut checkpoint = match saved {
                Some(checkpoint)
                    if checkpoint.fingerprint == fingerprint
                        && checkpoint.model.artifact.sha256 == model.artifact.sha256 =>
                {
                    checkpoint
                }
                _ => MigrationCheckpoint {
                    target: format!("filemofm_{}", uuid::Uuid::new_v4().simple()),
                    fingerprint: fingerprint.clone(),
                    config: config.clone(),
                    model: model.clone(),
                    previous: old_manifest.clone(),
                    phase: "backfill".into(),
                    completed: Default::default(),
                },
            };
            let created = self
                .backend
                .as_mut()
                .ok_or("Qdrant is unavailable")?
                .ensure(&checkpoint.target, config.dimension)?;
            if created {
                checkpoint.completed.clear();
            }
            self.repository
                .save_migration(&checkpoint)
                .map_err(|error| error.to_string())?;
            self.encoder = Some(encoder);
            self.config.embedding = config.clone();
            self.manifest = None;
            const CATCHUP_PASSES: usize = 8;
            let mut is_consistent = false;
            for _ in 0..CATCHUP_PASSES {
                let records = self
                    .repository
                    .all_files()
                    .map_err(|error| error.to_string())?;
                let live = records
                    .iter()
                    .filter(|file| !file.is_missing)
                    .collect::<Vec<_>>();
                let retired = checkpoint
                    .completed
                    .keys()
                    .filter(|id| !live.iter().any(|file| &file.id == *id))
                    .cloned()
                    .collect::<Vec<_>>();
                for id in retired {
                    self.backend
                        .as_mut()
                        .ok_or("Qdrant is unavailable")?
                        .delete(&checkpoint.target, &id)?;
                    checkpoint.completed.remove(&id);
                }
                for (index, file) in live.iter().enumerate() {
                    if self.cancellation.load(Ordering::Acquire) {
                        return Err("Rebuild paused; the checkpoint was saved and the previous index stays active. Start the rebuild again to continue".into());
                    }
                    if checkpoint
                        .completed
                        .get(&file.id)
                        .is_some_and(|(revision, _)| *revision == file.revision)
                    {
                        continue;
                    }
                    let chunks = match self.write_file(file, &checkpoint.target, config.dimension) {
                        Ok(chunks) => chunks,
                        Err(error)
                            if self
                                .repository
                                .get_file(&file.id)
                                .map_err(|error| error.to_string())?
                                .is_none_or(|current| {
                                    current.revision != file.revision || current.is_missing
                                }) =>
                        {
                            checkpoint.completed.remove(&file.id);
                            let _ = error;
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    checkpoint
                        .completed
                        .insert(file.id.clone(), (file.revision, chunks));
                    self.repository
                        .save_migration(&checkpoint)
                        .map_err(|error| error.to_string())?;
                    let _ = self.sender.try_send(BackgroundMessage::VectorStatus {
                        backend: ServiceStatus::Online,
                        model: format!("Rebuild {}/{}", index + 1, live.len()),
                        pending: live.len().saturating_sub(index + 1),
                        error: None,
                    });
                }
                let current = self
                    .repository
                    .all_files()
                    .map_err(|error| error.to_string())?;
                let current = current
                    .iter()
                    .filter(|file| !file.is_missing)
                    .collect::<Vec<_>>();
                if current.len() == checkpoint.completed.len()
                    && current.iter().all(|file| {
                        checkpoint
                            .completed
                            .get(&file.id)
                            .is_some_and(|(revision, _)| *revision == file.revision)
                    })
                {
                    is_consistent = true;
                    break;
                }
            }
            if !is_consistent {
                return Err("Metadata keeps changing; the checkpoint was saved. Continue when changes settle".into());
            }
            checkpoint.phase = "verifying".into();
            self.repository
                .save_migration(&checkpoint)
                .map_err(|error| error.to_string())?;
            let expected = checkpoint
                .completed
                .values()
                .map(|(_, chunks)| *chunks)
                .sum::<usize>();
            if self
                .backend
                .as_mut()
                .ok_or("Qdrant is unavailable")?
                .count(&checkpoint.target)?
                != expected
            {
                checkpoint.completed.clear();
                self.repository
                    .save_migration(&checkpoint)
                    .map_err(|error| error.to_string())?;
                return Err(
                    "Chunk count verification failed; retrying rebuilds the candidate".into(),
                );
            }
            if let Some((id, (revision, _))) = checkpoint.completed.first_key_value() {
                let vector = Self::normalized(
                    self.encoder
                        .as_mut()
                        .ok_or("Model is not loaded")?
                        .embed("file", true)?,
                    config.dimension,
                )?;
                if !self
                    .backend
                    .as_mut()
                    .ok_or("Qdrant is unavailable")?
                    .search(&checkpoint.target, vector, std::slice::from_ref(id), 1)?
                    .iter()
                    .any(|(found, _, found_revision)| found == id && found_revision == revision)
                {
                    return Err("Filtered retrieval verification failed".into());
                }
            }
            if self.cancellation.load(Ordering::Acquire) {
                return Err("Rebuild paused before cutover".into());
            }
            checkpoint.phase = "cutover".into();
            self.repository
                .save_migration(&checkpoint)
                .map_err(|error| error.to_string())?;
            let manifest = DatasetManifest {
                rollback: old_manifest.clone().map(|mut manifest| {
                    manifest.rollback = None;
                    Box::new(manifest)
                }),
                active: checkpoint.target,
                previous: old_manifest
                    .as_ref()
                    .map(|manifest| manifest.active.clone()),
                dimension: config.dimension,
                fingerprint,
                model: Some(model.clone()),
            };
            self.models.activate(&model)?;
            self.repository
                .save_dataset(&manifest)
                .map_err(|error| error.to_string())?;
            self.manifest = Some(manifest);
            self.error = self
                .repository
                .enqueue_all()
                .err()
                .map(|error| error.to_string());
            self.failures.clear();
            self.is_paused = false;
            self.last_inference = Instant::now();
            let _ = self.sender.send(BackgroundMessage::DatasetActivated {
                config: config.clone(),
            });
            Ok(())
        })();
        if result.is_err() {
            self.encoder = None;
            self.manifest = old_manifest;
            self.config.embedding = old_config;
            if let Some(model) = old_model {
                let _ = self.models.activate(&model);
            }
        }
        result
    }
}

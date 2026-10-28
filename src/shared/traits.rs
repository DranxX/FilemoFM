use crate::shared::errors::{ConfigError, ScanError, StorageError};
use crate::shared::models::*;
use std::path::{Path, PathBuf};

pub trait FileRepository: Send + Sync {
    fn upsert_files(&self, files: &mut [FileItem]) -> Result<(), StorageError>;
    fn get_files_by_directory(&self, directory: &Path) -> Result<Vec<FileItem>, StorageError>;
    fn get_file(&self, id: &str) -> Result<Option<FileItem>, StorageError>;
    fn get_file_by_path(&self, path: &Path) -> Result<Option<FileItem>, StorageError>;
    fn update_path(&self, old: &Path, new: &Path) -> Result<(), StorageError>;
    /// Live records at or below `root`, as `(id, path)`.
    fn files_under(&self, root: &Path) -> Result<Vec<(String, PathBuf)>, StorageError>;
    fn relocate(
        &self,
        old: &Path,
        new: &Path,
        revisions: &[(String, i64)],
    ) -> Result<(), StorageError>;
    fn mark_missing(&self, path: &Path) -> Result<(), StorageError>;
    fn restore_files(&self, files: &[(String, std::path::PathBuf)]) -> Result<(), StorageError>;
    fn search_keyword(&self, query: &SearchQuery) -> Result<Vec<SearchResult>, StorageError>;
    fn update_description(
        &self,
        id: &str,
        revision: i64,
        description: &str,
    ) -> Result<(), StorageError>;
    fn resolve_description(&self, description: &str) -> Result<String, StorageError>;
}

pub trait TagRepository: Send + Sync {
    fn create_tag(&self, name: &str, h: f32, s: f32, l: f32) -> Result<Tag, StorageError>;
    fn delete_tag(&self, id: i64) -> Result<(), StorageError>;
    fn get_all_tags(&self) -> Result<Vec<Tag>, StorageError>;
    fn assign_tag(&self, file_id: &str, tag_id: i64) -> Result<(), StorageError>;
    fn remove_tag_from_file(&self, file_id: &str, tag_id: i64) -> Result<(), StorageError>;
    fn set_tags(&self, id: &str, revision: i64, tags: &[i64]) -> Result<(), StorageError>;
}

pub trait HistoryRepository: Send + Sync {
    fn record_visit(&self, path: &Path) -> Result<(), StorageError>;
    fn get_frequent(&self, limit: usize) -> Result<Vec<PathBuf>, StorageError>;
}

pub trait SyncRepository: Send + Sync {
    fn pending(&self, limit: usize) -> Result<Vec<OutboxItem>, StorageError>;
    fn pending_count(&self) -> Result<usize, StorageError>;
    fn acknowledge(&self, id: &str, revision: i64) -> Result<(), StorageError>;
    fn enqueue_all(&self) -> Result<(), StorageError>;
    fn dataset(&self) -> Result<Option<DatasetManifest>, StorageError>;
    fn save_dataset(&self, manifest: &DatasetManifest) -> Result<(), StorageError>;
    fn all_files(&self) -> Result<Vec<FileItem>, StorageError>;
    fn migration(&self) -> Result<Option<MigrationCheckpoint>, StorageError>;
    fn save_migration(&self, checkpoint: &MigrationCheckpoint) -> Result<(), StorageError>;
    fn projection(&self, collection: &str, id: &str) -> Result<Option<String>, StorageError>;
    fn clear_projection(&self, collection: &str, id: Option<&str>) -> Result<(), StorageError>;
    fn save_projection(&self, collection: &str, id: &str, hash: &str) -> Result<(), StorageError>;
}

pub trait CacheRepository: Send + Sync {
    fn get_size(&self, path: &Path) -> Result<Option<FolderSize>, StorageError>;
    fn save_size(&self, size: &FolderSize) -> Result<(), StorageError>;
    fn invalidate_sizes(&self, path: &Path) -> Result<(), StorageError>;
    fn clear_caches(&self) -> Result<(), StorageError>;
    fn backup(&self, destination: &Path) -> Result<(), StorageError>;
    fn restore(&self, source: &Path) -> Result<(), StorageError>;
}

pub trait OperationRepository: Send + Sync {
    fn journal(
        &self,
        id: &str,
        source: &Path,
        target: &Path,
        phase: &str,
    ) -> Result<(), StorageError>;
    /// Closes journal rows of an operation that did not reach a committed phase.
    fn finish_operation(&self, id: &str, phase: &str) -> Result<(), StorageError>;
    fn unfinished_operations(
        &self,
    ) -> Result<Vec<(String, PathBuf, PathBuf, String)>, StorageError>;
}

pub trait MetadataRepository:
    FileRepository
    + TagRepository
    + HistoryRepository
    + SyncRepository
    + CacheRepository
    + OperationRepository
{
}
impl<
    T: FileRepository
        + TagRepository
        + HistoryRepository
        + SyncRepository
        + CacheRepository
        + OperationRepository,
> MetadataRepository for T
{
}

pub trait PathValidator: Send + Sync {
    fn set_exclusions(&self, paths: Vec<String>);
    fn validate(&self, path: &str) -> Result<PathBuf, ScanError>;
    fn is_excluded(&self, path: &Path) -> bool;
}

pub trait ConfigRepository: Send + Sync {
    fn load(&self) -> Result<AppConfig, ConfigError>;
    fn save(&self, config: &AppConfig) -> Result<(), ConfigError>;
}

pub trait NameIndex: Send + Sync {
    fn search(&self, query: &SearchQuery) -> Result<Vec<PathBuf>, String>;
    fn reset(&self);
}

pub trait VectorSearch: Send + Sync {
    fn search(&self, query: SearchQuery) -> Result<Vec<SearchResult>, String>;
}

pub trait FileInspector: Send + Sync {
    fn inspect(&self, path: &Path) -> Option<FileItem>;
}

pub trait ModelRepository: Send + Sync {
    fn inspect(&self, repo: &str) -> Result<ModelArtifact, String>;
    fn download(
        &self,
        artifact: &ModelArtifact,
        cancel: &std::sync::atomic::AtomicBool,
        sender: &EventSink,
    ) -> Result<VerifiedModel, String>;
    fn active(&self) -> Result<Option<VerifiedModel>, String>;
    fn activate(&self, model: &VerifiedModel) -> Result<(), String>;
}

pub trait TextEncoder: Send {
    fn embed(&mut self, text: &str, is_query: bool) -> Result<Vec<f32>, String>;
    fn fingerprint(&self) -> String;
}

pub trait EncoderFactory: Send + Sync {
    fn load(
        &self,
        model: &VerifiedModel,
        config: &EmbeddingConfig,
    ) -> Result<Box<dyn TextEncoder>, String>;
}

pub trait VectorDataset: Send {
    fn ensure(&mut self, name: &str, dimensions: usize) -> Result<bool, String>;
    fn upsert(
        &mut self,
        name: &str,
        id: &str,
        chunk: usize,
        vector: Vec<f32>,
        revision: i64,
    ) -> Result<(), String>;
    fn delete(&mut self, name: &str, id: &str) -> Result<(), String>;
    fn search(
        &mut self,
        name: &str,
        vector: Vec<f32>,
        ids: &[String],
        limit: usize,
    ) -> Result<Vec<(String, f32, i64)>, String>;
    fn count(&mut self, name: &str) -> Result<usize, String>;
    fn is_available(&mut self) -> bool;
    fn update_revision(&mut self, name: &str, id: &str, revision: i64) -> Result<(), String>;
}

pub trait VectorBackendFactory: Send + Sync {
    fn start(&self) -> Result<Box<dyn VectorDataset>, String>;
}

pub trait ToolDispatcher: Send + Sync {
    fn dispatch(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, String>;
}

pub trait PermissionPolicy: Send + Sync {
    fn get(&self) -> AgentConfig;
    fn set(&self, config: AgentConfig);
    fn authorize(&self, path: &Path, capability: &str) -> Result<(), String>;
}

pub trait ClipboardRepository: Send + Sync {
    fn refresh(&self);
    fn write(&self, payload: ClipboardPayload) -> Result<(), String>;
    fn clear(&self);
}

pub trait PlatformIntegration: Send + Sync {
    fn locations(&self) -> Vec<(String, PathBuf)>;
    fn drives(&self) -> Vec<(String, PathBuf)>;
    fn open(&self, path: &Path) -> Result<(), String>;
    fn terminal(&self, path: &Path) -> Result<(), String>;
    fn register_runtime(&self, path: &Path, kind: &str, version: &str) -> Result<(), String>;
}

pub trait MutationApprover: Send + Sync {
    fn approve(&self, intent: MutationIntent) -> Result<(), String>;
}
pub trait FileOperationQueue: Send + Sync {
    fn submit(&self, request: OperationRequest) -> Result<(), String>;
}

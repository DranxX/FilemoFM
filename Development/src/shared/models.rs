use crate::shared::constants::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct RuntimePaths {
    pub root: PathBuf,
    pub config: PathBuf,
    pub data: PathBuf,
    pub database: PathBuf,
    pub logs: PathBuf,
    pub models: PathBuf,
    pub runtime: PathBuf,
    pub trash: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileItem {
    pub id: String,
    #[serde(
        serialize_with = "crate::shared::utils::serialize_native_path",
        deserialize_with = "crate::shared::utils::deserialize_native_path"
    )]
    pub path: PathBuf,
    pub name: String,
    pub extension: String,
    pub size: u64,
    pub is_directory: bool,
    pub created_at: i64,
    pub modified_at: i64,
    pub updated_at: i64,
    pub revision: i64,
    pub native_id: Option<String>,
    pub is_hidden: bool,
    pub is_missing: bool,
    pub tags: Vec<Tag>,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub color_h: f32,
    pub color_s: f32,
    pub color_l: f32,
}

#[derive(Debug, Clone)]
pub enum FilePreview {
    Image {
        width: u32,
        height: u32,
        rgba_data: Vec<u8>,
    },
    Text(String),
    None,
    TooLarge,
    UnsupportedEncoding,
    DecodeFailed(String),
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub file: FileItem,
    pub score: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SearchMode {
    #[default]
    Keyword,
    Semantic,
    Hybrid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SearchScope {
    #[default]
    CurrentFolder,
    IndexedFiles,
    SelectedRoots,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceStatus {
    Online,
    Offline,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SortColumn {
    #[default]
    Name,
    Size,
    Modified,
    Extension,
    Created,
    Tags,
    Relevance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ViewMode {
    #[default]
    Details,
    List,
    SmallIcons,
    MediumIcons,
    LargeIcons,
    ExtraLargeIcons,
    Tiles,
    Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum GroupColumn {
    #[default]
    None,
    Name,
    Modified,
    Extension,
    Size,
    Created,
    Tags,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SizeState {
    Complete,
    Partial,
    Stale,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderSize {
    pub path: PathBuf,
    pub bytes: u64,
    pub state: SizeState,
    pub measured_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SearchFilter {
    Title(String),
    Extension(String),
    Category(String),
    Tag(String),
    Date(i64, i64),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub text: String,
    pub filters: Vec<SearchFilter>,
    pub ids: Vec<String>,
    pub scope: SearchScope,
    pub directory: PathBuf,
    pub roots: Vec<PathBuf>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationKind {
    Relocate,
    Copy,
    Move,
    Rename,
    Trash,
    Delete,
    Restore,
    CreateFile,
    CreateFolder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CollisionPolicy {
    #[default]
    Skip,
    RenameCopy,
    Replace,
    Merge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationRequest {
    pub id: String,
    pub kind: OperationKind,
    pub sources: Vec<PathBuf>,
    pub identities: Vec<(PathBuf, String)>,
    pub revisions: Vec<(String, i64)>,
    pub destination: PathBuf,
    pub name: String,
    pub collision: CollisionPolicy,
    pub is_permanent_confirmed: bool,
    pub allow_trash_fallback: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationReport {
    pub id: String,
    pub done: usize,
    pub total: usize,
    pub failures: Vec<String>,
    pub is_cancelled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardPayload {
    pub paths: Vec<PathBuf>,
    pub is_cut: bool,
    #[serde(default)]
    pub identities: Vec<(PathBuf, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxItem {
    pub id: String,
    pub revision: i64,
    pub is_deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetManifest {
    #[serde(default)]
    pub rollback: Option<Box<DatasetManifest>>,
    #[serde(default)]
    pub model: Option<VerifiedModel>,
    pub active: String,
    pub previous: Option<String>,
    pub dimension: usize,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationCheckpoint {
    pub target: String,
    pub fingerprint: String,
    pub config: EmbeddingConfig,
    pub model: VerifiedModel,
    pub previous: Option<DatasetManifest>,
    pub phase: String,
    pub completed: std::collections::BTreeMap<String, (i64, usize)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelArtifact {
    pub repo: String,
    pub revision: String,
    pub filename: String,
    pub bytes: u64,
    pub sha256: String,
    pub license: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifiedModel {
    pub artifact: ModelArtifact,
    #[serde(
        serialize_with = "crate::shared::utils::serialize_native_path",
        deserialize_with = "crate::shared::utils::deserialize_native_path"
    )]
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeCompanion {
    pub filename: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeArtifact {
    #[serde(default)]
    pub companions: Vec<RuntimeCompanion>,
    pub filename: String,
    pub sha256: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub enum MutationIntent {
    Description {
        id: String,
        revision: i64,
        source: String,
    },
    Tags {
        id: String,
        revision: i64,
        tags: Vec<i64>,
    },
    Files(OperationRequest),
}

#[derive(Debug, Clone)]
pub enum BackgroundMessage {
    FrequentFolders(Vec<PathBuf>),
    RootRelocated {
        old: PathBuf,
        new: PathBuf,
    },
    CanvasReady {
        generation: u64,
        files: Vec<FileItem>,
    },
    ScanBatch {
        generation: u64,
        files: Vec<FileItem>,
        is_complete: bool,
    },
    ScanError {
        generation: u64,
        error: String,
    },
    DirectoryChanged(PathBuf),
    SearchResults {
        generation: u64,
        results: Result<Vec<SearchResult>, String>,
    },
    FolderSizeReady {
        generation: u64,
        size: FolderSize,
    },
    PreviewReady {
        generation: u64,
        file_id: String,
        preview: FilePreview,
    },
    PreviewFailed {
        generation: u64,
        file_id: String,
        error: String,
    },
    MetadataChanged(Result<(), String>),
    TagsLoaded(Vec<Tag>),
    OperationProgress(OperationReport),
    OperationComplete(OperationReport),
    VectorStatus {
        backend: ServiceStatus,
        model: String,
        pending: usize,
        error: Option<String>,
    },
    ModelInspected(Result<ModelArtifact, String>),
    DownloadProgress {
        done: u64,
        total: u64,
    },
    ModelDownloaded(Result<VerifiedModel, String>),
    ClipboardReady(Result<ClipboardPayload, String>),
    AgentDelta {
        generation: u64,
        text: String,
    },
    AgentComplete {
        generation: u64,
        result: Result<(), String>,
    },
    ConnectionTest(Result<(), String>),
    Diagnostic(Result<String, String>),
    LocationResolved {
        generation: u64,
        result: Result<(PathBuf, Option<PathBuf>), String>,
    },
    NavigationChildren {
        path: PathBuf,
        result: Result<Vec<PathBuf>, String>,
    },
    EditorLoaded {
        generation: u64,
        result: Result<(FileItem, Vec<FileItem>), String>,
    },
    DescriptionSaved {
        id: String,
        result: Result<(), String>,
    },
    ReferenceCandidates {
        generation: u64,
        result: Result<Vec<FileItem>, String>,
    },
    ConfigurationSaved {
        config: Box<AppConfig>,
        result: Result<(), String>,
    },
    DatasetActivated {
        config: EmbeddingConfig,
    },
    StartupState {
        tags: Vec<Tag>,
        frequent: Vec<PathBuf>,
        recovery: Vec<(String, PathBuf, PathBuf, String)>,
    },
    PlatformReady {
        locations: Vec<(String, PathBuf)>,
        drives: Vec<(String, PathBuf)>,
    },
    RestoreScheduled {
        path: PathBuf,
        result: Result<(), String>,
    },
    PickedLocation(PathBuf),
    DiskSpace {
        path: PathBuf,
        free: u64,
        total: u64,
    },
    OperationReview(OperationRequest),
    MutationApproval {
        intent: MutationIntent,
        response: crossbeam_channel::Sender<bool>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub version: u32,
    pub general: GeneralConfig,
    pub sidebar: SidebarConfig,
    pub files: FileConfig,
    pub search: SearchConfig,
    pub qdrant: QdrantConfig,
    pub embedding: EmbeddingConfig,
    pub scanner: ScannerConfig,
    pub agent: AgentConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub language: String,
    pub inspector_tab: InspectorTab,
    pub theme: String,
    pub font_size: f32,
    pub ui_scale: f32,
    pub last_window_width: u32,
    pub last_window_height: u32,
    pub maximized: bool,
    pub last_path: String,
    pub pending_restore: Option<String>,
    pub restore_session: bool,
    pub show_sidebar: bool,
    pub show_inspector: bool,
    pub sidebar_width: f32,
    pub inspector_width: f32,
    pub log_retention_days: usize,
}
impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            language: crate::shared::i18n::DEFAULT_LANGUAGE.into(),
            inspector_tab: InspectorTab::Preview,
            theme: "dark".into(),
            font_size: 13.0,
            ui_scale: 1.0,
            // Fits a 1080p screen at 150% scaling with room for the taskbar.
            last_window_width: 1100,
            last_window_height: 660,
            maximized: false,
            last_path: directories::UserDirs::new()
                .map(|d| crate::shared::utils::config_path(d.home_dir()))
                .unwrap_or_else(|| std::path::MAIN_SEPARATOR.to_string()),
            pending_restore: None,
            restore_session: true,
            show_sidebar: true,
            show_inspector: true,
            sidebar_width: SIDEBAR_WIDTH,
            inspector_width: PREVIEW_PANEL_WIDTH,
            log_retention_days: 7,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SidebarConfig {
    pub closed_sections: std::collections::BTreeSet<String>,
    pub favorites: Vec<String>,
    pub shares: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FileConfig {
    pub folders_first: bool,
    pub show_extensions: bool,
    pub show_hidden: bool,
    pub compact: bool,
    pub checkboxes: bool,
    pub view: ViewMode,
    pub sort: SortColumn,
    pub ascending: bool,
    pub group: GroupColumn,
    pub collision: CollisionPolicy,
    pub trash_fallback: bool,
}
impl Default for FileConfig {
    fn default() -> Self {
        Self {
            folders_first: true,
            show_extensions: true,
            show_hidden: false,
            compact: true,
            checkboxes: false,
            view: ViewMode::Details,
            sort: SortColumn::Name,
            ascending: true,
            group: GroupColumn::None,
            collision: CollisionPolicy::Skip,
            trash_fallback: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    pub scope: SearchScope,
    pub mode: SearchMode,
    pub debounce_ms: u64,
    pub limit: usize,
    pub roots: Vec<String>,
}
impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            scope: SearchScope::CurrentFolder,
            mode: SearchMode::Keyword,
            debounce_ms: SEARCH_DEBOUNCE_MS,
            limit: 100,
            roots: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QdrantConfig {
    pub enabled: bool,
    pub auto_sync: bool,
}
impl Default for QdrantConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_sync: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingConfig {
    pub repo_url: String,
    pub dimension: usize,
    pub idle_seconds: u64,
    pub threads: usize,
    pub context_tokens: usize,
}
impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            repo_url: DEFAULT_MODEL_REPO.into(),
            dimension: DEFAULT_DIMENSIONS,
            idle_seconds: 300,
            threads: 4,
            context_tokens: 2048,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScannerConfig {
    pub watch_enabled: bool,
    pub excluded_paths: Vec<String>,
    pub size_entry_budget: usize,
    pub size_time_budget_seconds: u64,
}
impl Default for ScannerConfig {
    fn default() -> Self {
        Self {
            watch_enabled: true,
            excluded_paths: Vec::new(),
            size_entry_budget: SIZE_ENTRY_BUDGET,
            size_time_budget_seconds: SIZE_TIME_BUDGET_SECS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EndpointType {
    #[default]
    ChatCompatible,
    Ollama,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentEndpoint {
    pub name: String,
    pub base_url: String,
    pub endpoint_type: EndpointType,
    pub model: String,
    pub credential_env: String,
    pub allow_local_unauthenticated: bool,
    pub timeout_seconds: u64,
}
impl Default for AgentEndpoint {
    fn default() -> Self {
        Self {
            name: "Local Ollama".into(),
            base_url: "http://127.0.0.1:11434".into(),
            endpoint_type: EndpointType::Ollama,
            model: String::new(),
            credential_env: String::new(),
            // The default endpoint is a loopback Ollama, which has no authentication.
            allow_local_unauthenticated: true,
            timeout_seconds: 120,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub enabled: bool,
    pub endpoints: Vec<AgentEndpoint>,
    pub allowed_roots: Vec<String>,
    pub metadata_read: bool,
    pub content_read: bool,
    pub semantic_search: bool,
    pub metadata_write: bool,
    pub filesystem_write: bool,
    pub network: bool,
    pub mcp_stdio: bool,
}
impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoints: Vec::new(),
            allowed_roots: Vec::new(),
            metadata_read: true,
            content_read: false,
            semantic_search: false,
            metadata_write: false,
            filesystem_write: false,
            network: false,
            mcp_stdio: false,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            general: GeneralConfig::default(),
            sidebar: SidebarConfig::default(),
            files: FileConfig::default(),
            search: SearchConfig::default(),
            qdrant: QdrantConfig::default(),
            embedding: EmbeddingConfig::default(),
            scanner: ScannerConfig::default(),
            agent: AgentConfig::default(),
        }
    }
}

#[derive(Clone)]
pub struct EventSink {
    sender: crossbeam_channel::Sender<BackgroundMessage>,
    wake: std::sync::Arc<dyn Fn() + Send + Sync>,
    closed: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl EventSink {
    pub fn new(
        sender: crossbeam_channel::Sender<BackgroundMessage>,
        wake: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            sender,
            wake,
            closed: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
    pub fn send(
        &self,
        mut message: BackgroundMessage,
    ) -> Result<(), crossbeam_channel::SendError<BackgroundMessage>> {
        loop {
            if self.closed.load(std::sync::atomic::Ordering::Acquire) {
                return Err(crossbeam_channel::SendError(message));
            }
            match self
                .sender
                .send_timeout(message, std::time::Duration::from_millis(50))
            {
                Ok(()) => {
                    (self.wake)();
                    return Ok(());
                }
                Err(crossbeam_channel::SendTimeoutError::Timeout(value)) => message = value,
                Err(crossbeam_channel::SendTimeoutError::Disconnected(value)) => {
                    return Err(crossbeam_channel::SendError(value));
                }
            }
        }
    }
    pub fn try_send(
        &self,
        message: BackgroundMessage,
    ) -> Result<(), crossbeam_channel::TrySendError<BackgroundMessage>> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(crossbeam_channel::TrySendError::Disconnected(message));
        }
        let result = self.sender.try_send(message);
        (self.wake)();
        result
    }
    pub fn close(&self) {
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InspectorTab {
    Preview,
    Details,
    Agent,
}

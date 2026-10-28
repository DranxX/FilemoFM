use crate::services::{
    agent::AgentService,
    operations::OperationService,
    preview::request_preview,
    scanner::{FileScannerService, FolderSizeService, start_file_watcher},
    search::SearchService,
    tools::{MetadataTools, tool_schemas},
    vector::{VectorCommand, VectorService},
};
use crate::shared::i18n::{n_, tr};
use crate::shared::{
    constants::*,
    models::*,
    query::parse_query,
    traits::*,
    utils::{natural_compare, reference_nodes, validate_component},
};
use crate::ui::{
    breadcrumb::*, context_menu::*, preview_panel::*, search_bar::*, settings::*, sidebar::*,
    table_view::*, theme::FilemoTheme,
};
use crossbeam_channel::Receiver;
use egui_phosphor::regular::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

pub struct AppDependencies {
    pub paths: RuntimePaths,
    pub repository: Arc<dyn MetadataRepository>,
    pub configuration: Arc<dyn ConfigRepository>,
    pub models: Arc<dyn ModelRepository>,
    pub names: Arc<dyn NameIndex>,
    pub inspector: Arc<dyn FileInspector>,
    pub validator: Arc<dyn PathValidator>,
    pub platform: Arc<dyn PlatformIntegration>,
    pub encoders: Arc<dyn EncoderFactory>,
    pub backends: Arc<dyn VectorBackendFactory>,
    pub clipboard: Arc<dyn ClipboardRepository>,
    pub policy: Arc<dyn PermissionPolicy>,
    pub sender: EventSink,
    pub receiver: Receiver<BackgroundMessage>,
    pub startup_notice: Option<String>,
}
#[derive(Clone)]
enum PendingAction {
    Navigate(PathBuf, Option<usize>),
    Select(String, bool, bool),
    Exit,
    Reveal(String),
    Command(String, FileCommand),
}
struct NameDialog {
    command: FileCommand,
    value: String,
    focus: bool,
}
struct DirectorySnapshot {
    path: PathBuf,
    files: Vec<FileItem>,
    bytes: usize,
}

pub struct FilemoApp {
    context: egui::Context,
    dependencies: AppDependencies,
    config: AppConfig,
    configuration_worker: crate::services::configuration::ConfigurationService,
    scanner: FileScannerService,
    sizes_worker: FolderSizeService,
    search_worker: SearchService,
    presentation: crate::services::presentation::PresentationService,
    canvas_generation: u64,
    operations: Arc<OperationService>,
    vectors: Arc<VectorService>,
    agent: AgentService,
    current: PathBuf,
    directory_cache: VecDeque<DirectorySnapshot>,
    history: Vec<PathBuf>,
    history_index: usize,
    navigation: NavigationState,
    location_generation: u64,
    pending_history: Option<usize>,
    select_after_navigation: Option<PathBuf>,
    scan_generation: u64,
    scan_seen: HashSet<String>,
    search_generation: u64,
    editor_generation: u64,
    reference_generation: u64,
    files: Vec<FileItem>,
    visible: Vec<FileItem>,
    ranks: HashMap<String, f32>,
    selected: HashSet<String>,
    selection_total: String,
    anchor: Option<String>,
    canvas: CanvasState,
    sizes: HashMap<PathBuf, FolderSize>,
    requested_sizes: HashSet<PathBuf>,
    sidebar: SidebarState,
    frequent: Vec<PathBuf>,
    tags: Vec<Tag>,
    watcher: Option<notify::RecommendedWatcher>,
    is_scanning: bool,
    search: SearchBarState,
    search_changed: Option<Instant>,
    is_search_active: bool,
    editor: Option<DescriptionEditor>,
    preview: Option<FilePreview>,
    texture: Option<egui::TextureHandle>,
    references: Vec<FileItem>,
    pending: Option<PendingAction>,
    is_saving: bool,
    settings: Option<SettingsState>,
    name_dialog: Option<NameDialog>,
    delete_tag: Option<(i64, String)>,
    confirmation: Option<OperationRequest>,
    approval: Option<(MutationIntent, crossbeam_channel::Sender<bool>)>,
    operation_reports: Vec<OperationReport>,
    recovery: Vec<(String, PathBuf, PathBuf, String)>,
    clipboard: Option<ClipboardPayload>,
    cut_operation: Option<String>,
    paste_pending: bool,
    last_clipboard: Instant,
    was_focused: bool,
    backend: ServiceStatus,
    model_status: String,
    pending_vectors: usize,
    active_dimension: usize,
    notice: Option<String>,
    is_system: bool,
    download_cancel: Arc<AtomicBool>,
    chat: Vec<ChatMessage>,
    agent_input: String,
    agent_endpoint: usize,
    agent_key: String,
    agent_generation: u64,
    is_agent_busy: bool,
    include_selection: bool,
    toasts: Vec<Toast>,
    vector_error: Option<String>,
    /// Keyboard cursor for arrow navigation; `anchor` stays where a Shift range started.
    cursor: Option<String>,
    scroll_to: Option<String>,
    /// File ID → index in `files`, so scan batches merge in O(1) per file.
    file_index: HashMap<String, usize>,
    canvas_dirty: bool,
    last_canvas: Instant,
    paste_seen: bool,
    disk_space: Option<(PathBuf, u64, u64)>,
}

impl FilemoApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        config: AppConfig,
        dependencies: AppDependencies,
    ) -> Self {
        FilemoTheme::apply(&cc.egui_ctx, &config.general);
        let vectors = Arc::new(VectorService::new(
            dependencies.repository.clone(),
            dependencies.models.clone(),
            dependencies.encoders.clone(),
            dependencies.backends.clone(),
            config.clone(),
            dependencies.sender.clone(),
        ));
        let scanner = FileScannerService::new(
            dependencies.validator.clone(),
            dependencies.inspector.clone(),
            dependencies.repository.clone(),
            dependencies.sender.clone(),
        );
        let sizes_worker = FolderSizeService::new(
            dependencies.repository.clone(),
            dependencies.sender.clone(),
            config.scanner.clone(),
            dependencies.validator.clone(),
        );
        let search_worker = SearchService::new(
            dependencies.repository.clone(),
            dependencies.names.clone(),
            dependencies.inspector.clone(),
            vectors.clone(),
            dependencies.sender.clone(),
        );
        let operations = Arc::new(OperationService::new(
            dependencies.repository.clone(),
            dependencies.paths.clone(),
            dependencies.sender.clone(),
        ));
        let tools = Arc::new(MetadataTools::new(
            dependencies.repository.clone(),
            vectors.clone(),
            dependencies.policy.clone(),
            Some(Arc::new(
                crate::services::approval::InteractiveApproval::new(dependencies.sender.clone()),
            )),
            Some(operations.clone()),
        ));
        let agent = AgentService::new(
            dependencies.policy.clone(),
            tools,
            dependencies.sender.clone(),
            tool_schemas(true),
        );
        let initial = if config.general.restore_session {
            crate::shared::utils::parse_config_path(&config.general.last_path)
                .unwrap_or_else(|_| PathBuf::from(std::path::MAIN_SEPARATOR_STR))
        } else {
            directories::UserDirs::new()
                .map(|dirs| dirs.home_dir().to_path_buf())
                .unwrap_or_else(|| PathBuf::from(std::path::MAIN_SEPARATOR.to_string()))
        };
        let search = SearchBarState {
            query: String::new(),
            mode: config.search.mode,
            scope: config.search.scope,
            focus_requested: false,
            error: None,
            is_searching: false,
        };
        let configuration_worker = crate::services::configuration::ConfigurationService::new(
            dependencies.configuration.clone(),
            dependencies.sender.clone(),
        );
        let mut app = Self {
            presentation: crate::services::presentation::PresentationService::new(
                dependencies.sender.clone(),
            ),
            canvas_generation: 0,
            context: cc.egui_ctx.clone(),
            configuration_worker,
            dependencies,
            config,
            scanner,
            sizes_worker,
            search_worker,
            operations,
            vectors,
            agent,
            current: initial.clone(),
            directory_cache: VecDeque::new(),
            history: Vec::new(),
            history_index: 0,
            navigation: NavigationState {
                address: initial.display().to_string(),
                is_editing: false,
                focus_requested: false,
                can_back: false,
                can_forward: false,
            },
            location_generation: 0,
            pending_history: None,
            select_after_navigation: None,
            scan_generation: 0,
            scan_seen: HashSet::new(),
            search_generation: 0,
            editor_generation: 0,
            reference_generation: 0,
            files: Vec::new(),
            visible: Vec::new(),
            ranks: HashMap::new(),
            selected: HashSet::new(),
            selection_total: String::new(),
            anchor: None,
            canvas: CanvasState::new(),
            sizes: HashMap::new(),
            requested_sizes: HashSet::new(),
            sidebar: SidebarState {
                locations: Vec::new(),
                drives: Vec::new(),
                children: HashMap::new(),
            },
            frequent: Vec::new(),
            tags: Vec::new(),
            watcher: None,
            is_scanning: false,
            search,
            search_changed: None,
            is_search_active: false,
            editor: None,
            preview: None,
            texture: None,
            references: Vec::new(),
            pending: None,
            is_saving: false,
            settings: None,
            name_dialog: None,
            delete_tag: None,
            confirmation: None,
            approval: None,
            operation_reports: Vec::new(),
            recovery: Vec::new(),
            clipboard: None,
            cut_operation: None,
            paste_pending: false,
            last_clipboard: Instant::now(),
            was_focused: false,
            backend: ServiceStatus::Offline,
            model_status: "Not loaded".into(),
            pending_vectors: 0,
            active_dimension: DEFAULT_DIMENSIONS,
            notice: None,
            is_system: false,
            download_cancel: Arc::new(AtomicBool::new(false)),
            chat: Vec::new(),
            agent_input: String::new(),
            agent_endpoint: 0,
            agent_key: String::new(),
            agent_generation: 0,
            is_agent_busy: false,
            include_selection: false,
            toasts: Vec::new(),
            vector_error: None,
            cursor: None,
            scroll_to: None,
            file_index: HashMap::new(),
            canvas_dirty: false,
            last_canvas: Instant::now(),
            paste_seen: false,
            disk_space: None,
        };
        if let Some(text) = app.dependencies.startup_notice.take() {
            app.info(text);
        }
        let repository = app.dependencies.repository.clone();
        let platform = app.dependencies.platform.clone();
        let sender = app.dependencies.sender.clone();
        rayon::spawn(move || {
            let _ = sender.send(BackgroundMessage::StartupState {
                tags: repository.get_all_tags().unwrap_or_default(),
                frequent: repository
                    .get_frequent(HISTORY_FREQUENT_LIMIT)
                    .unwrap_or_default(),
                recovery: repository.unfinished_operations().unwrap_or_default(),
            });
            let _ = sender.send(BackgroundMessage::PlatformReady {
                locations: platform.locations(),
                drives: platform.drives(),
            });
        });
        app.resolve_location(initial, None);
        app.dependencies.clipboard.refresh();
        app
    }
    fn request_action(&mut self, action: PendingAction) {
        if self
            .editor
            .as_ref()
            .is_some_and(DescriptionEditor::is_dirty)
        {
            self.pending = Some(action);
            return;
        }
        self.perform_action(action);
    }
    fn perform_action(&mut self, action: PendingAction) {
        match action {
            PendingAction::Navigate(path, index) => self.resolve_location(path, index),
            PendingAction::Select(id, toggle, range) => self.select(id, toggle, range),
            PendingAction::Exit => {}
            PendingAction::Reveal(id) => self.reveal_reference(id),
            PendingAction::Command(id, command) => {
                if !self.selected.contains(&id) {
                    self.select(id, false, false);
                }
                self.command(command, &self.context.clone());
            }
        }
    }
    fn resolve_location(&mut self, path: PathBuf, index: Option<usize>) {
        self.location_generation += 1;
        let generation = self.location_generation;
        self.pending_history = index;
        let validator = self.dependencies.validator.clone();
        let sender = self.dependencies.sender.clone();
        rayon::spawn(move || {
            let result = (|| {
                let canonical = if let Some(text) = path.to_str() {
                    validator
                        .validate(text)
                        .map_err(|error| error.to_string())?
                } else {
                    let value = crate::shared::utils::canonicalize(&path)
                        .map_err(|_| "Location is unavailable")?;
                    if validator.is_excluded(&value) {
                        return Err("Location is excluded".into());
                    }
                    value
                };
                if canonical.is_dir() {
                    return Ok((canonical, None));
                }
                if canonical.is_file() {
                    let parent = canonical
                        .parent()
                        .ok_or("Parent folder is unavailable")?
                        .to_path_buf();
                    return Ok((parent, Some(canonical)));
                }
                Err("Location is unavailable".into())
            })();
            let _ = sender.send(BackgroundMessage::LocationResolved { generation, result });
        });
    }
    fn navigate_ready(&mut self, path: PathBuf, selection: Option<PathBuf>) {
        if !self.is_search_active && !self.is_scanning && !self.files.is_empty() {
            let bytes = self
                .files
                .iter()
                .map(|file| {
                    std::mem::size_of::<FileItem>()
                        + crate::shared::utils::path_key(&file.path).len() * 2
                        + file.name.len()
                        + file.extension.len()
                        + file.description.len()
                        + file
                            .tags
                            .iter()
                            .map(|tag| std::mem::size_of::<Tag>() + tag.name.len())
                            .sum::<usize>()
                })
                .sum::<usize>()
                .saturating_mul(2);
            self.directory_cache
                .retain(|snapshot| snapshot.path != self.current);
            if bytes <= DIRECTORY_CACHE_BYTES {
                while !self.directory_cache.is_empty()
                    && (self.directory_cache.len() >= DIRECTORY_CACHE_ENTRIES
                        || self
                            .directory_cache
                            .iter()
                            .map(|snapshot| snapshot.bytes)
                            .sum::<usize>()
                            + bytes
                            > DIRECTORY_CACHE_BYTES)
                {
                    self.directory_cache.pop_front();
                }
                self.directory_cache.push_back(DirectorySnapshot {
                    path: self.current.clone(),
                    files: self.files.clone(),
                    bytes,
                });
            }
        }
        if let Some(index) = self.pending_history.take() {
            self.history_index = index;
        } else if self.history.get(self.history_index) != Some(&path) {
            if !self.history.is_empty() {
                self.history.truncate(self.history_index + 1);
            }
            self.history.push(path.clone());
            self.history_index = self.history.len() - 1;
        }
        self.current = path;
        self.sizes.clear();
        let repository = self.dependencies.repository.clone();
        let sender = self.dependencies.sender.clone();
        let visited = self.current.clone();
        rayon::spawn(move || {
            if repository.record_visit(&visited).is_ok()
                && let Ok(folders) = repository.get_frequent(HISTORY_FREQUENT_LIMIT)
            {
                let _ = sender.send(BackgroundMessage::FrequentFolders(folders));
            }
        });
        self.navigation.address = crate::shared::utils::config_path(&self.current);
        self.navigation.is_editing = false;
        let title = self
            .current
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.current.display().to_string());
        self.context
            .send_viewport_cmd(egui::ViewportCommand::Title(format!(
                "{title} — {APP_NAME}"
            )));
        self.cursor = None;
        self.is_system = false;
        self.search_worker.cancel();
        self.search.query.clear();
        self.search.error = None;
        self.is_search_active = false;
        self.search.is_searching = false;
        self.search_changed = None;
        self.selected.clear();
        self.anchor = None;
        self.editor_generation += 1;
        self.reference_generation += 1;
        self.editor = None;
        self.preview = None;
        self.texture = None;
        self.select_after_navigation = selection;
        self.files = self
            .directory_cache
            .iter()
            .find(|snapshot| snapshot.path == self.current)
            .map(|snapshot| snapshot.files.clone())
            .unwrap_or_default();
        self.visible.clear();
        self.rebuild_canvas();
        self.refresh(false);
        // The status bar shows this folder's total size and the free space on its volume.
        self.sizes_worker
            .request(self.current.clone(), self.scan_generation);
        let sender = self.dependencies.sender.clone();
        let path = self.current.clone();
        rayon::spawn(move || {
            if let (Ok(free), Ok(total)) = (fs2::available_space(&path), fs2::total_space(&path)) {
                let _ = sender.send(BackgroundMessage::DiskSpace { path, free, total });
            }
        });
        self.watcher = if self.config.scanner.watch_enabled {
            start_file_watcher(self.current.clone(), self.dependencies.sender.clone())
        } else {
            None
        };
    }
    fn refresh(&mut self, clear: bool) {
        if self.is_search_active {
            self.execute_search();
            return;
        }
        if clear {
            self.files.clear();
            self.visible.clear();
            self.rebuild_canvas();
        }
        self.reindex_files();
        self.scan_seen.clear();
        self.is_scanning = true;
        self.scan_generation = self.scanner.scan_directory(self.current.clone());
        self.sizes_worker.reset(self.scan_generation);
        self.requested_sizes.clear();
        // Contents may have changed, so the folder total is measured again.
        self.sizes_worker
            .request(self.current.clone(), self.scan_generation);
    }
    fn reindex_files(&mut self) {
        self.file_index = self
            .files
            .iter()
            .enumerate()
            .map(|(index, file)| (file.id.clone(), index))
            .collect();
    }
    fn go_up(&mut self) {
        if let Some(parent) = self.current.parent() {
            self.request_action(PendingAction::Navigate(parent.into(), None));
        }
    }
    fn rebuild_canvas(&mut self) {
        self.canvas_dirty = false;
        self.last_canvas = Instant::now();
        let files = self
            .files
            .iter()
            .filter(|file| self.config.files.show_hidden || !file.is_hidden)
            .cloned()
            .collect();
        self.canvas_generation = self.presentation.sort(
            files,
            self.config.files.clone(),
            self.sizes.clone(),
            self.ranks.clone(),
        );
        self.rebuild_selection_total();
    }
    fn select(&mut self, id: String, toggle: bool, range: bool) {
        if id.is_empty() {
            self.selected.clear();
            self.anchor = None;
            self.cursor = None;
            self.load_selection();
            return;
        }
        if range {
            let anchor = self
                .anchor
                .as_ref()
                .and_then(|id| self.visible.iter().position(|file| &file.id == id));
            let end = self.visible.iter().position(|file| file.id == id);
            match (anchor, end) {
                (Some(start), Some(end)) => {
                    if !toggle {
                        self.selected.clear();
                    }
                    for file in &self.visible[start.min(end)..=start.max(end)] {
                        self.selected.insert(file.id.clone());
                    }
                }
                _ => {
                    self.selected.clear();
                    self.selected.insert(id.clone());
                    self.anchor = Some(id.clone());
                }
            }
        } else if toggle {
            if !self.selected.remove(&id) {
                self.selected.insert(id.clone());
            }
            self.anchor = Some(id.clone());
        } else {
            self.selected.clear();
            self.selected.insert(id.clone());
            self.anchor = Some(id.clone());
        }
        self.cursor = Some(id);
        self.load_selection();
    }
    fn rebuild_selection_total(&mut self) {
        let mut bytes = 0u64;
        let mut unknown = 0usize;
        let mut partial = false;
        let mut selected = self
            .files
            .iter()
            .filter(|file| self.selected.contains(&file.id))
            .collect::<Vec<_>>();
        selected.sort_by_key(|file| file.path.components().count());
        let mut covered = Vec::<&Path>::new();
        for file in selected {
            if covered.iter().any(|parent| file.path.starts_with(parent)) {
                continue;
            }
            covered.push(&file.path);
            if file.is_directory {
                if let Some(size) = self.sizes.get(&file.path) {
                    bytes = bytes.saturating_add(size.bytes);
                    partial |= size.state != SizeState::Complete;
                } else {
                    unknown += 1;
                }
            } else {
                bytes = bytes.saturating_add(file.size);
            }
        }
        self.selection_total = format!(
            "{}{}{}",
            crate::shared::utils::format_size(bytes),
            if partial { " partial" } else { "" },
            if unknown > 0 {
                format!(" + {unknown} unknown")
            } else {
                String::new()
            }
        );
    }
    fn load_selection(&mut self) {
        self.rebuild_selection_total();
        self.editor_generation += 1;
        let generation = self.editor_generation;
        self.editor = None;
        self.preview = None;
        self.texture = None;
        self.references.clear();
        if self.selected.len() != 1 {
            return;
        }
        let Some(file) = self
            .files
            .iter()
            .find(|file| self.selected.contains(&file.id))
            .cloned()
        else {
            return;
        };
        if file.is_directory {
            self.sizes_worker
                .request(file.path.clone(), self.scan_generation);
        } else {
            request_preview(file.clone(), generation, self.dependencies.sender.clone());
        }
        let repository = self.dependencies.repository.clone();
        let sender = self.dependencies.sender.clone();
        rayon::spawn(move || {
            let result = (|| {
                let file = repository
                    .get_file(&file.id)
                    .map_err(|error| error.to_string())?
                    .ok_or("Metadata is unavailable")?;
                let targets = reference_nodes(&file.description)
                    .into_iter()
                    .filter_map(|(_, id, _)| repository.get_file(&id).ok().flatten())
                    .collect();
                Ok((file, targets))
            })();
            let _ = sender.send(BackgroundMessage::EditorLoaded { generation, result });
        });
    }
    fn save_description(&mut self) {
        if self.is_saving {
            return;
        }
        let Some(editor) = &self.editor else { return };
        let id = editor.file.id.clone();
        let revision = editor.file.revision;
        let source = editor.source();
        self.is_saving = true;
        let repository = self.dependencies.repository.clone();
        let sender = self.dependencies.sender.clone();
        rayon::spawn(move || {
            let result = repository
                .update_description(&id, revision, &source)
                .map_err(|error| error.to_string());
            let _ = sender.send(BackgroundMessage::DescriptionSaved { id, result });
        });
    }
    fn execute_search(&mut self) {
        self.search_changed = None;
        if self
            .editor
            .as_ref()
            .is_some_and(DescriptionEditor::is_dirty)
        {
            self.notice = Some("Save or discard the description before searching".into());
            return;
        }
        if self.search.query.trim().is_empty() {
            self.search_worker.cancel();
            self.is_search_active = false;
            self.refresh(true);
            return;
        }
        let roots = self
            .config
            .search
            .roots
            .iter()
            .filter_map(|value| crate::shared::utils::parse_config_path(value).ok())
            .collect();
        match parse_query(
            &self.search.query,
            self.search.scope,
            self.current.clone(),
            roots,
            self.config.search.limit,
        )
        .and_then(|query| self.search_worker.search(query, self.search.mode))
        {
            Ok(generation) => {
                self.search_generation = generation;
                self.search.error = None;
                self.search.is_searching = true;
                self.is_search_active = true;
            }
            Err(error) => self.search.error = Some(error),
        }
    }
    fn open(&mut self, id: &str) {
        let Some(file) = self.files.iter().find(|file| file.id == id).cloned() else {
            return;
        };
        if file.is_directory {
            self.request_action(PendingAction::Navigate(file.path, None));
            return;
        }
        let platform = self.dependencies.platform.clone();
        let sender = self.dependencies.sender.clone();
        rayon::spawn(move || {
            if let Err(error) = platform.open(&file.path) {
                let _ = sender.send(BackgroundMessage::Diagnostic(Err(error)));
            }
        });
    }
    fn operation_request(&self, kind: OperationKind) -> OperationRequest {
        OperationRequest {
            id: uuid::Uuid::new_v4().to_string(),
            kind,
            sources: self
                .files
                .iter()
                .filter(|file| self.selected.contains(&file.id))
                .map(|file| file.path.clone())
                .collect(),
            identities: self
                .files
                .iter()
                .filter(|file| self.selected.contains(&file.id))
                .filter_map(|file| file.native_id.clone().map(|id| (file.path.clone(), id)))
                .collect(),
            revisions: self
                .files
                .iter()
                .filter(|file| self.selected.contains(&file.id))
                .map(|file| (file.id.clone(), file.revision))
                .collect(),
            destination: self.current.clone(),
            name: String::new(),
            collision: self.config.files.collision,
            is_permanent_confirmed: false,
            allow_trash_fallback: self.config.files.trash_fallback,
        }
    }
    fn command(&mut self, command: FileCommand, ctx: &egui::Context) {
        match command {
            FileCommand::Open => {
                // The focused item, or the only selected one; never an arbitrary pick.
                let id = self
                    .cursor
                    .clone()
                    .filter(|id| self.selected.contains(id))
                    .or_else(|| {
                        (self.selected.len() == 1)
                            .then(|| self.selected.iter().next().cloned())
                            .flatten()
                    });
                if let Some(id) = id {
                    self.open(&id);
                }
            }
            FileCommand::Copy | FileCommand::Cut => {
                let paths = self.operation_request(OperationKind::Copy).sources;
                if paths.is_empty() {
                    return;
                }
                let payload = ClipboardPayload {
                    paths,
                    identities: self.operation_request(OperationKind::Copy).identities,
                    is_cut: matches!(command, FileCommand::Cut),
                };
                let count = payload.paths.len();
                let is_cut = payload.is_cut;
                match self.dependencies.clipboard.write(payload.clone()) {
                    Ok(()) => {
                        self.clipboard = Some(payload);
                        self.info(if is_cut {
                            crate::tr!("{count} item(s) cut — paste with Ctrl+V", count = count)
                        } else {
                            crate::tr!("{count} item(s) copied — paste with Ctrl+V", count = count)
                        });
                    }
                    Err(error) => self.notice = Some(error),
                }
            }
            FileCommand::Paste => {
                self.paste_pending = true;
                self.dependencies.clipboard.refresh();
            }
            FileCommand::Trash => {
                // Moving to the trash can be undone from the system trash, so no prompt.
                let request = self.operation_request(OperationKind::Trash);
                if !request.sources.is_empty() {
                    self.submit_operation(request);
                }
            }
            FileCommand::Delete => {
                let request = self.operation_request(OperationKind::Delete);
                if !request.sources.is_empty() {
                    self.confirmation = Some(request);
                }
            }
            FileCommand::Rename => {
                if self.selected.len() == 1
                    && let Some(file) = self
                        .files
                        .iter()
                        .find(|file| self.selected.contains(&file.id))
                {
                    self.name_dialog = Some(NameDialog {
                        command,
                        value: file.name.clone(),
                        focus: true,
                    });
                }
            }
            FileCommand::NewFolder | FileCommand::NewFile => {
                self.name_dialog = Some(NameDialog {
                    command,
                    value: String::new(),
                    focus: true,
                })
            }
            FileCommand::CopyPath => {
                ctx.copy_text(
                    self.files
                        .iter()
                        .filter(|file| self.selected.contains(&file.id))
                        .map(|file| file.path.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
            }
            FileCommand::Pin => {
                let folder = self
                    .files
                    .iter()
                    .find(|file| self.selected.contains(&file.id) && file.is_directory)
                    .map(|file| file.path.clone())
                    .unwrap_or_else(|| self.current.clone());
                let path = crate::shared::utils::config_path(&folder);
                if self.config.sidebar.favorites.contains(&path) {
                    self.info("This folder is already pinned");
                } else {
                    self.config.sidebar.favorites.push(path);
                    self.persist_config();
                }
            }
            FileCommand::Properties => {
                self.config.general.show_inspector = true;
                self.config.general.inspector_tab = InspectorTab::Details;
            }
            FileCommand::SelectAll => {
                if self
                    .editor
                    .as_ref()
                    .is_some_and(DescriptionEditor::is_dirty)
                {
                    self.notice =
                        Some("Save or discard the description before changing selection".into());
                    return;
                }
                self.selected = self.visible.iter().map(|file| file.id.clone()).collect();
                self.load_selection();
            }
            FileCommand::SelectNone | FileCommand::Invert => {
                if self
                    .editor
                    .as_ref()
                    .is_some_and(DescriptionEditor::is_dirty)
                {
                    self.notice =
                        Some("Save or discard the description before changing selection".into());
                    return;
                }
                if matches!(command, FileCommand::Invert) {
                    self.selected = self
                        .visible
                        .iter()
                        .filter(|file| !self.selected.contains(&file.id))
                        .map(|file| file.id.clone())
                        .collect();
                } else {
                    self.selected.clear();
                }
                self.load_selection();
            }
            FileCommand::Settings => self.open_settings(SettingsTab::General),
            FileCommand::Terminal => {
                let platform = self.dependencies.platform.clone();
                let path = self.current.clone();
                let sender = self.dependencies.sender.clone();
                rayon::spawn(move || {
                    let _ = sender.send(BackgroundMessage::Diagnostic(
                        platform.terminal(&path).map(|_| "Terminal opened".into()),
                    ));
                });
            }
        }
    }
    /// Pastes immediately like Explorer; asks only when names collide in this folder.
    fn paste(&mut self, payload: ClipboardPayload) {
        if self.is_search_active {
            self.notice = Some("Open a folder before pasting".into());
            return;
        }
        let mut request = self.operation_request(if payload.is_cut {
            OperationKind::Move
        } else {
            OperationKind::Copy
        });
        request.sources = payload.paths.clone();
        request.identities = payload.identities.clone();
        request.revisions.clear();
        // Windows names are case-insensitive, so `Notes.md` collides with `notes.md` there.
        let fold = |name: &std::ffi::OsStr| {
            let name = name.to_string_lossy();
            if cfg!(windows) {
                name.to_lowercase()
            } else {
                name.into_owned()
            }
        };
        let names: HashSet<String> = self
            .files
            .iter()
            .filter_map(|file| file.path.file_name().map(fold))
            .collect();
        let collides = payload.paths.iter().any(|path| {
            path.parent() != Some(self.current.as_path())
                && path
                    .file_name()
                    .is_some_and(|name| names.contains(&fold(name)))
        });
        if collides {
            self.confirmation = Some(request);
        } else {
            self.submit_operation(request);
        }
    }
    fn submit_operation(&mut self, request: OperationRequest) {
        if request.kind == OperationKind::Move
            && self
                .clipboard
                .as_ref()
                .is_some_and(|payload| payload.is_cut && payload.paths == request.sources)
        {
            self.cut_operation = Some(request.id.clone());
        }
        if let Err(error) = self.operations.submit(request) {
            self.notice = Some(error);
        }
    }
    fn report_outcome(&mut self, report: &OperationReport) {
        if report.is_cancelled {
            self.info(crate::tr!(
                "Operation cancelled after {done} of {total} item(s)",
                done = report.done,
                total = report.total
            ));
        } else if let Some(first) = report.failures.first() {
            let message = if report.failures.len() == 1 {
                first.clone()
            } else {
                crate::tr!(
                    "{failed} of {total} item(s) failed. First: {first}",
                    failed = report.failures.len(),
                    total = report.total,
                    first = first
                )
            };
            self.notice = Some(message);
        }
    }
    fn persist_config(&self) {
        if let Err(error) = self.configuration_worker.save(self.config.clone(), false) {
            tracing::error!("{error}");
        }
    }
    fn process_messages(&mut self, ctx: &egui::Context) {
        for _ in 0..MESSAGE_FRAME_BUDGET {
            let Ok(message) = self.dependencies.receiver.try_recv() else {
                break;
            };
            match message {
                BackgroundMessage::FrequentFolders(folders) => self.frequent = folders,
                BackgroundMessage::RootRelocated { old, new } => {
                    for paths in [
                        &mut self.config.search.roots,
                        &mut self.config.sidebar.favorites,
                        &mut self.config.sidebar.shares,
                    ] {
                        for value in paths {
                            if let Ok(path) = crate::shared::utils::parse_config_path(value)
                                && let Ok(relative) = path.strip_prefix(&old)
                            {
                                *value = crate::shared::utils::config_path(&new.join(relative));
                            }
                        }
                    }
                    self.directory_cache.clear();
                    self.persist_config();
                    self.request_action(PendingAction::Navigate(new, None));
                }
                BackgroundMessage::CanvasReady { generation, files }
                    if generation == self.canvas_generation =>
                {
                    self.visible = files;
                    self.canvas
                        .rebuild(&self.visible, &self.sizes, self.config.files.group);
                }
                BackgroundMessage::LocationResolved { generation, result }
                    if generation == self.location_generation =>
                {
                    match result {
                        Ok((path, selection)) => self.navigate_ready(path, selection),
                        Err(error) => self.notice = Some(error),
                    }
                }
                BackgroundMessage::ScanBatch {
                    generation,
                    files,
                    is_complete,
                } if generation == self.scan_generation && !self.is_search_active => {
                    if !self.is_scanning {
                        continue;
                    }
                    for file in files {
                        self.scan_seen.insert(file.id.clone());
                        if let Some(index) = self.file_index.get(&file.id).copied() {
                            self.files[index] = file;
                        } else {
                            self.file_index.insert(file.id.clone(), self.files.len());
                            self.files.push(file);
                        }
                    }
                    self.is_scanning = !is_complete;
                    // Show the first batch immediately, then re-sort at most a few times a second.
                    if self.visible.is_empty() {
                        self.rebuild_canvas();
                    } else {
                        self.canvas_dirty = true;
                    }
                    if is_complete {
                        self.files.retain(|file| self.scan_seen.contains(&file.id));
                        self.reindex_files();
                        let known = &self.file_index;
                        self.selected.retain(|id| known.contains_key(id));
                        self.rebuild_canvas();
                        if self.selected.len() == 1
                            && !self
                                .editor
                                .as_ref()
                                .is_some_and(DescriptionEditor::is_dirty)
                        {
                            self.load_selection();
                        }
                        if let Some(path) = self.select_after_navigation.take()
                            && let Some(file) = self.files.iter().find(|file| file.path == path)
                        {
                            self.select(file.id.clone(), false, false);
                        }
                    }
                }
                BackgroundMessage::ScanError { generation, error }
                    if generation == self.scan_generation =>
                {
                    self.is_scanning = false;
                    self.notice = Some(error);
                }
                BackgroundMessage::DirectoryChanged(path) if path == self.current => {
                    // Rescan in place: the current list stays visible and keeps its scroll
                    // position while the new listing merges in.
                    if !self
                        .editor
                        .as_ref()
                        .is_some_and(DescriptionEditor::is_dirty)
                        && !self.is_scanning
                    {
                        self.refresh(false);
                    }
                }
                BackgroundMessage::SearchResults {
                    generation,
                    results,
                } if generation == self.search_generation => {
                    self.search.is_searching = false;
                    match results {
                        Ok(results) => {
                            self.ranks = results
                                .iter()
                                .map(|hit| (hit.file.id.clone(), hit.score))
                                .collect();
                            self.files = results.into_iter().map(|hit| hit.file).collect();
                            self.reindex_files();
                            self.rebuild_canvas();
                        }
                        Err(error) => self.search.error = Some(error),
                    }
                }
                BackgroundMessage::FolderSizeReady { generation, size }
                    if generation == self.scan_generation =>
                {
                    self.sizes.insert(size.path.clone(), size);
                    // Sizes are drawn straight from the map; only size ordering needs a re-sort.
                    if self.config.files.sort == SortColumn::Size
                        || self.config.files.group == GroupColumn::Size
                    {
                        self.canvas_dirty = true;
                    }
                    self.rebuild_selection_total();
                }
                BackgroundMessage::PreviewReady {
                    generation,
                    file_id,
                    preview,
                } if generation == self.editor_generation && self.selected.contains(&file_id) => {
                    if let FilePreview::Image {
                        width,
                        height,
                        rgba_data,
                    } = &preview
                    {
                        self.texture = Some(ctx.load_texture(
                            file_id,
                            egui::ColorImage::from_rgba_unmultiplied(
                                [*width as usize, *height as usize],
                                rgba_data,
                            ),
                            Default::default(),
                        ));
                    }
                    self.preview = Some(preview);
                }
                BackgroundMessage::PreviewFailed {
                    generation,
                    file_id,
                    error,
                } if generation == self.editor_generation && self.selected.contains(&file_id) => {
                    self.preview = Some(FilePreview::DecodeFailed(error))
                }
                BackgroundMessage::EditorLoaded { generation, result }
                    if generation == self.editor_generation =>
                {
                    match result {
                        Ok((file, targets)) => {
                            self.editor = Some(DescriptionEditor::new(file, targets))
                        }
                        Err(error) => self.notice = Some(error),
                    }
                }
                BackgroundMessage::DescriptionSaved { id, result } => {
                    self.is_saving = false;
                    match result {
                        Ok(()) => {
                            if let Some(editor) = &mut self.editor
                                && editor.file.id == id
                            {
                                editor.saved = editor.draft.clone();
                                editor.file.revision += 1;
                            }
                            self.info("Description saved");
                            if let Some(action) = self.pending.take() {
                                if matches!(action, PendingAction::Exit) {
                                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                } else {
                                    self.perform_action(action);
                                }
                            } else {
                                self.refresh(false);
                                self.load_selection();
                            }
                        }
                        Err(error) => {
                            self.pending = None;
                            self.notice = Some(error);
                        }
                    }
                }
                BackgroundMessage::ReferenceCandidates { generation, result }
                    if generation == self.reference_generation =>
                {
                    match result {
                        Ok(files) => self.references = files,
                        Err(error) => self.notice = Some(error),
                    }
                }
                BackgroundMessage::NavigationChildren { path, result } => {
                    self.sidebar.children.insert(path, result);
                }
                BackgroundMessage::PlatformReady { locations, drives } => {
                    let drives: Vec<(String, PathBuf)> = drives;
                    self.sidebar.locations = locations;
                    self.sidebar.drives = drives;
                }
                BackgroundMessage::StartupState {
                    tags,
                    frequent,
                    recovery,
                } => {
                    self.tags = tags;
                    self.frequent = frequent;
                    self.recovery = recovery;
                }
                BackgroundMessage::TagsLoaded(tags) => self.tags = tags,
                BackgroundMessage::MetadataChanged(result) => match result {
                    Ok(()) => {
                        self.refresh(false);
                        self.load_selection();
                    }
                    Err(error) => self.notice = Some(error),
                },
                BackgroundMessage::OperationProgress(report) => self.update_report(report),
                BackgroundMessage::OperationComplete(report) => {
                    if self.cut_operation.as_ref() == Some(&report.id)
                        && report.done == report.total
                        && report.failures.is_empty()
                        && !report.is_cancelled
                    {
                        self.dependencies.clipboard.clear();
                        self.clipboard = None;
                        self.cut_operation = None;
                    }
                    self.report_outcome(&report);
                    self.update_report(report);
                    self.refresh(false);
                }
                BackgroundMessage::VectorStatus {
                    backend,
                    model,
                    pending,
                    error,
                } => {
                    // Vector problems show on the status indicator and its tooltip, not as
                    // repeated notifications (this message arrives every second).
                    self.backend = backend;
                    self.model_status = model;
                    self.pending_vectors = pending;
                    self.vector_error = error;
                }
                BackgroundMessage::DatasetActivated { config } => {
                    self.active_dimension = config.dimension;
                    self.config.embedding = config;
                    self.persist_config();
                    self.info("New vector generation is active");
                }
                BackgroundMessage::ConfigurationSaved { config, result } => match result {
                    Ok(()) => {
                        self.dependencies
                            .validator
                            .set_exclusions(config.scanner.excluded_paths.clone());
                        self.dependencies.policy.set(config.agent.clone());
                        self.cancel_agent();
                        let names = self.dependencies.names.clone();
                        rayon::spawn(move || names.reset());
                        self.config = *config;
                        self.search.mode = self.config.search.mode;
                        self.search.scope = self.config.search.scope;
                        self.sizes_worker = FolderSizeService::new(
                            self.dependencies.repository.clone(),
                            self.dependencies.sender.clone(),
                            self.config.scanner.clone(),
                            self.dependencies.validator.clone(),
                        );
                        let _ = self
                            .vectors
                            .command(VectorCommand::Configure(self.config.clone()));
                        FilemoTheme::apply(ctx, &self.config.general);
                        if let Err(error) = crate::shared::i18n::set_language(
                            &self.config.general.language,
                            &self.dependencies.paths.root,
                        ) {
                            self.notice = Some(error);
                        }
                        self.settings = None;
                        self.refresh(false);
                        self.watcher = if self.config.scanner.watch_enabled {
                            start_file_watcher(
                                self.current.clone(),
                                self.dependencies.sender.clone(),
                            )
                        } else {
                            None
                        };
                    }
                    Err(error) => {
                        if let Some(settings) = &mut self.settings {
                            settings.error = Some(error);
                            settings.is_busy = false;
                        }
                    }
                },
                BackgroundMessage::ModelInspected(result) => {
                    if let Some(settings) = &mut self.settings {
                        settings.is_busy = false;
                        match result {
                            Ok(artifact) => {
                                settings.artifact = Some(artifact);
                                settings.error = None;
                            }
                            Err(error) => settings.error = Some(error),
                        }
                    }
                }
                BackgroundMessage::DownloadProgress { done, total } => {
                    if let Some(settings) = &mut self.settings {
                        settings.download = Some((done, total));
                    }
                }
                BackgroundMessage::ModelDownloaded(result) => {
                    if let Some(settings) = &mut self.settings {
                        settings.is_busy = false;
                        match result {
                            Ok(model) => {
                                settings.downloaded = Some(model);
                                settings.error = None;
                            }
                            Err(error) => settings.error = Some(error),
                        }
                    }
                }
                BackgroundMessage::ClipboardReady(result) => {
                    match result {
                        Ok(payload) => {
                            self.clipboard = if payload.paths.is_empty() {
                                None
                            } else {
                                Some(payload)
                            };
                            if self.paste_pending {
                                if let Some(payload) = self.clipboard.clone() {
                                    self.paste(payload);
                                } else {
                                    self.notice = Some("Clipboard has no files to paste".into());
                                }
                            }
                        }
                        Err(error) => {
                            self.clipboard = None;
                            if self.paste_pending {
                                self.notice = Some(error);
                            }
                        }
                    }
                    self.paste_pending = false;
                }
                BackgroundMessage::AgentDelta { generation, text }
                    if generation == self.agent_generation =>
                {
                    if let Some(message) = self.chat.last_mut()
                        && message.role == "assistant"
                    {
                        message.content.push_str(&text);
                    }
                }
                BackgroundMessage::AgentComplete { generation, result }
                    if generation == self.agent_generation =>
                {
                    self.is_agent_busy = false;
                    if result.is_ok()
                        && !self
                            .editor
                            .as_ref()
                            .is_some_and(DescriptionEditor::is_dirty)
                    {
                        self.refresh(false);
                    }
                    if let Err(error) = result {
                        self.notice = Some(error);
                    }
                }
                BackgroundMessage::ConnectionTest(result) => {
                    let message = result
                        .map(|_| "Endpoint connection succeeded".into())
                        .unwrap_or_else(|error| error);
                    if let Some(settings) = &mut self.settings {
                        settings.error = Some(message);
                    }
                }
                BackgroundMessage::Diagnostic(result) => match result {
                    Ok(text) => self.info(text),
                    Err(error) => self.notice = Some(error),
                },
                BackgroundMessage::RestoreScheduled { path, result } => match result {
                    Ok(()) => {
                        let mut config = self.config.clone();
                        config.general.pending_restore =
                            Some(crate::shared::utils::config_path(&path));
                        match self.configuration_worker.save(config.clone(), true) {
                            Ok(()) => {
                                self.config = config;
                                self.info("Restore scheduled. Close and reopen FilemoFM. A safety backup is made before replacing metadata.");
                            }
                            Err(error) => self.notice = Some(error),
                        }
                    }
                    Err(error) => self.notice = Some(error),
                },
                BackgroundMessage::MutationApproval { intent, response } => {
                    if self.approval.is_some() {
                        let _ = response.send(false);
                    } else {
                        self.approval = Some((intent, response));
                    }
                }
                BackgroundMessage::OperationReview(request) => {
                    self.settings = None;
                    self.confirmation = Some(request);
                }
                BackgroundMessage::DiskSpace { path, free, total } if path == self.current => {
                    self.disk_space = Some((path, free, total));
                }
                BackgroundMessage::PickedLocation(path) => {
                    self.request_action(PendingAction::Navigate(path, None))
                }
                _ => {}
            }
        }
        if !self.dependencies.receiver.is_empty() {
            ctx.request_repaint();
        }
    }
    fn update_report(&mut self, report: OperationReport) {
        if let Some(previous) = self
            .operation_reports
            .iter_mut()
            .find(|previous| previous.id == report.id)
        {
            *previous = report;
        } else {
            self.operation_reports.push(report);
        }
        if self.operation_reports.len() > 100 {
            self.operation_reports.remove(0);
        }
    }
    fn settings_action(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::Relocate(old, new) => {
                let repository = self.dependencies.repository.clone();
                let validator = self.dependencies.validator.clone();
                let sender = self.dependencies.sender.clone();
                rayon::spawn(move || {
                    let result = (|| -> Result<OperationRequest, String> {
                        let old = crate::shared::utils::parse_config_path(&old)?;
                        if old.exists() {
                            return Err(
                                "The old root still exists; relocation is only for missing roots"
                                    .into(),
                            );
                        }
                        let new = validator
                            .validate(&new)
                            .map_err(|error| error.to_string())?;
                        if !new.is_dir() {
                            return Err("Replacement root must be a folder".into());
                        }
                        let revisions = repository
                            .all_files()
                            .map_err(|error| error.to_string())?
                            .into_iter()
                            .filter(|file| file.path.starts_with(&old))
                            .map(|file| (file.id, file.revision))
                            .collect::<Vec<_>>();
                        if revisions.is_empty() || revisions.len() > MAX_RELOCATION_FILES {
                            return Err(
                                "Root has no metadata or exceeds the relocation limit".into()
                            );
                        }
                        Ok(OperationRequest {
                            id: uuid::Uuid::new_v4().to_string(),
                            kind: OperationKind::Relocate,
                            sources: vec![old],
                            identities: Vec::new(),
                            revisions,
                            destination: new,
                            name: String::new(),
                            collision: CollisionPolicy::Skip,
                            is_permanent_confirmed: false,
                            allow_trash_fallback: false,
                        })
                    })();
                    let message = match result {
                        Ok(request) => BackgroundMessage::OperationReview(request),
                        Err(error) => BackgroundMessage::Diagnostic(Err(error)),
                    };
                    let _ = sender.send(message);
                });
            }
            SettingsAction::Cancel => {
                self.download_cancel.store(true, Ordering::Release);
                self.settings = None;
            }
            SettingsAction::Apply => {
                let Some(settings) = &mut self.settings else {
                    return;
                };
                let mut config = settings.draft.clone();
                config.embedding.dimension = self.active_dimension;
                config.embedding.repo_url = self.config.embedding.repo_url.clone();
                settings.is_busy = true;
                if let Err(error) = self.configuration_worker.save(config, true) {
                    settings.is_busy = false;
                    settings.error = Some(error);
                }
            }
            SettingsAction::Inspect => {
                let Some(settings) = &mut self.settings else {
                    return;
                };
                settings.is_busy = true;
                let repo = settings.draft.embedding.repo_url.clone();
                let models = self.dependencies.models.clone();
                let sender = self.dependencies.sender.clone();
                rayon::spawn(move || {
                    let _ = sender.send(BackgroundMessage::ModelInspected(models.inspect(&repo)));
                });
            }
            SettingsAction::Download => {
                let Some(settings) = &mut self.settings else {
                    return;
                };
                let Some(artifact) = settings.artifact.clone() else {
                    return;
                };
                settings.is_busy = true;
                self.download_cancel.store(false, Ordering::Release);
                let cancel = self.download_cancel.clone();
                let models = self.dependencies.models.clone();
                let sender = self.dependencies.sender.clone();
                std::thread::spawn(move || {
                    let _ = sender.send(BackgroundMessage::ModelDownloaded(
                        models.download(&artifact, &cancel, &sender),
                    ));
                });
            }
            SettingsAction::CancelDownload => self.download_cancel.store(true, Ordering::Release),
            SettingsAction::Pause => self.vectors.cancel(),
            SettingsAction::Rollback => {
                if let Err(error) = self.vectors.command(VectorCommand::Rollback) {
                    self.notice = Some(error);
                }
            }
            SettingsAction::Load | SettingsAction::Unload | SettingsAction::Synchronize => {
                let command = match action {
                    SettingsAction::Load => VectorCommand::Load,
                    SettingsAction::Unload => VectorCommand::Unload,
                    _ => VectorCommand::Synchronize,
                };
                if let Err(error) = self.vectors.command(command) {
                    self.notice = Some(error);
                }
            }
            SettingsAction::Rebuild => {
                let Some(settings) = &self.settings else {
                    return;
                };
                let config = settings.draft.embedding.clone();
                let downloaded = settings.downloaded.clone();
                let models = self.dependencies.models.clone();
                let vectors = self.vectors.clone();
                let sender = self.dependencies.sender.clone();
                rayon::spawn(move || {
                    let result = (|| {
                        let model = downloaded
                            .or(models.active()?)
                            .ok_or("Download a verified model first")?;
                        vectors.command(VectorCommand::Rebuild(config, model))?;
                        Ok("Vector rebuild queued".into())
                    })();
                    let _ = sender.send(BackgroundMessage::Diagnostic(result));
                });
            }
            SettingsAction::Test(endpoint, key) => self.agent.test_connection(endpoint, key),
            SettingsAction::Register(kind, version) => {
                let platform = self.dependencies.platform.clone();
                let sender = self.dependencies.sender.clone();
                std::thread::spawn(move || {
                    let Some(path) = rfd::FileDialog::new()
                        .set_title("Select trusted native executable")
                        .pick_file()
                    else {
                        return;
                    };
                    let _ = sender.send(BackgroundMessage::Diagnostic(
                        platform
                            .register_runtime(&path, &kind, &version)
                            .map(|_| "Runtime registered; Synchronize to retry startup".into()),
                    ));
                });
            }
            SettingsAction::Backup => {
                let repository = self.dependencies.repository.clone();
                let sender = self.dependencies.sender.clone();
                std::thread::spawn(move || {
                    let Some(path) = rfd::FileDialog::new()
                        .set_file_name("filemofm-backup.db")
                        .save_file()
                    else {
                        return;
                    };
                    let _ = sender.send(BackgroundMessage::Diagnostic(
                        repository
                            .backup(&path)
                            .map(|_| "Backup saved".into())
                            .map_err(|error| error.to_string()),
                    ));
                });
            }
            SettingsAction::Restore => {
                let sender = self.dependencies.sender.clone();
                std::thread::spawn(move || {
                    let Some(path) = rfd::FileDialog::new()
                        .set_title("Restore metadata on next startup")
                        .pick_file()
                    else {
                        return;
                    };
                    let _ = sender.send(BackgroundMessage::RestoreScheduled {
                        path,
                        result: Ok(()),
                    });
                });
            }
            SettingsAction::ClearCaches => {
                self.sizes.clear();
                let repository = self.dependencies.repository.clone();
                let sender = self.dependencies.sender.clone();
                rayon::spawn(move || {
                    let _ = sender.send(BackgroundMessage::Diagnostic(
                        repository
                            .clear_caches()
                            .map(|_| "Size cache cleared".into())
                            .map_err(|error| error.to_string()),
                    ));
                });
            }
            SettingsAction::Diagnostic => {
                let repository = self.dependencies.repository.clone();
                let sender = self.dependencies.sender.clone();
                rayon::spawn(move || {
                    let result = repository
                        .unfinished_operations()
                        .map(|rows| {
                            if rows.is_empty() {
                                "No unfinished journal entries".into()
                            } else {
                                rows.into_iter()
                                    .map(|(id, source, target, phase)| {
                                        format!(
                                            "{id}: {} → {} ({phase})",
                                            source.display(),
                                            target.display()
                                        )
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            }
                        })
                        .map_err(|error| error.to_string());
                    let _ = sender.send(BackgroundMessage::Diagnostic(result));
                });
            }
        }
    }
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let count = self.selected.len();
        let can_paste = self.clipboard.is_some();
        let before = serde_json::to_string(&self.config.files).unwrap_or_default();
        let mut command = None;
        let labels = ui.available_width() > 920.0;
        let text = |icon: &str, label: &str| {
            if labels {
                format!("{icon} {}", tr(label))
            } else {
                icon.to_owned()
            }
        };
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.menu_button(text(PLUS, n_("New")), |ui| {
                if ui
                    .button(format!("{FOLDER_SIMPLE} {}", tr("Folder")))
                    .clicked()
                {
                    command = Some(FileCommand::NewFolder);
                    ui.close();
                }
                if ui.button(format!("{FILE} {}", tr("File"))).clicked() {
                    command = Some(FileCommand::NewFile);
                    ui.close();
                }
            });
            ui.separator();
            for (icon, label, hint, value, enabled) in [
                (SCISSORS, n_("Cut"), "Ctrl+X", FileCommand::Cut, count > 0),
                (COPY, n_("Copy"), "Ctrl+C", FileCommand::Copy, count > 0),
                (
                    CLIPBOARD,
                    n_("Paste"),
                    "Ctrl+V",
                    FileCommand::Paste,
                    can_paste,
                ),
                (
                    PENCIL_SIMPLE,
                    "Rename",
                    "F2",
                    FileCommand::Rename,
                    count == 1,
                ),
            ] {
                if ui
                    .add_enabled(enabled, egui::Button::new(text(icon, label)).frame(false))
                    .on_hover_text(format!("{} · {hint}", tr(label)))
                    .on_disabled_hover_text(match value {
                        FileCommand::Paste => tr("Clipboard has no files").into_owned(),
                        FileCommand::Rename => tr("Select exactly one item").into_owned(),
                        _ => tr("Select one or more items").into_owned(),
                    })
                    .clicked()
                {
                    command = Some(value);
                }
            }
            ui.add_enabled_ui(count > 0, |ui| {
                ui.menu_button(
                    format!("{} {CARET_DOWN}", text(TRASH, n_("Delete"))),
                    |ui| {
                        if ui
                            .button(format!("{TRASH} {} · Del", tr("Move to Trash")))
                            .clicked()
                        {
                            command = Some(FileCommand::Trash);
                            ui.close();
                        }
                        if ui
                            .button(format!(
                                "{WARNING} {} · Shift+Del",
                                tr("Delete permanently…")
                            ))
                            .clicked()
                        {
                            command = Some(FileCommand::Delete);
                            ui.close();
                        }
                    },
                )
                .response
                .on_disabled_hover_text(tr("Select one or more items"));
            });
            ui.separator();
            ui.menu_button(
                format!("{} {CARET_DOWN}", text(ARROWS_DOWN_UP, n_("Sort"))),
                |ui| sort_menu(ui, &mut self.config.files),
            );
            ui.menu_button(format!("{} {CARET_DOWN}", text(LIST, n_("View"))), |ui| {
                view_menu(
                    ui,
                    &mut self.config.files,
                    &mut self.config.general.show_sidebar,
                    &mut self.config.general.show_inspector,
                )
            });
            ui.menu_button(
                format!("{} {CARET_DOWN}", text(DOTS_THREE, n_("More"))),
                |ui| {
                    for (label, value) in [
                        (n_("Select all"), FileCommand::SelectAll),
                        (n_("Select none"), FileCommand::SelectNone),
                        (n_("Invert selection"), FileCommand::Invert),
                        (n_("Copy path"), FileCommand::CopyPath),
                        (n_("Pin folder"), FileCommand::Pin),
                        (n_("Properties"), FileCommand::Properties),
                        (n_("Open terminal"), FileCommand::Terminal),
                        (n_("Settings"), FileCommand::Settings),
                    ] {
                        if ui.button(tr(label)).clicked() {
                            command = Some(value);
                            ui.close();
                        }
                    }
                },
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(GEAR).on_hover_text("Settings").clicked() {
                    command = Some(FileCommand::Settings);
                }
                let shown = self.config.general.show_inspector;
                if ui
                    .selectable_label(shown, text(SIDEBAR_SIMPLE, n_("Details pane")))
                    .on_hover_text(format!(
                        "{} · Ctrl+P",
                        tr("Show or hide Preview / Details / Agent")
                    ))
                    .clicked()
                {
                    self.config.general.show_inspector = !shown;
                    self.persist_config();
                }
            });
        });
        ui.add_space(2.0);
        if before != serde_json::to_string(&self.config.files).unwrap_or_default() {
            self.rebuild_canvas();
            self.persist_config();
        }
        if let Some(command) = command {
            self.command(command, &ctx);
        }
    }
    fn sidebar_action(&mut self, action: SidebarAction) {
        match action {
            SidebarAction::DeleteTag(id) => {
                self.delete_tag = self
                    .tags
                    .iter()
                    .find(|tag| tag.id == id)
                    .map(|tag| (id, tag.name.clone()));
            }
            SidebarAction::Navigate(path) => {
                self.request_action(PendingAction::Navigate(path, None))
            }
            SidebarAction::Expand(path) => {
                let sender = self.dependencies.sender.clone();
                rayon::spawn(move || {
                    let result = (|| {
                        let mut children = Vec::new();
                        for entry in std::fs::read_dir(&path)
                            .map_err(|_| "Folder cannot be accessed")?
                            .take(MAX_RESULTS)
                        {
                            let entry = entry.map_err(|_| "Some folders cannot be accessed")?;
                            if entry
                                .file_type()
                                .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
                            {
                                children.push(entry.path());
                            }
                        }
                        children.sort_by(|a, b| {
                            natural_compare(
                                &a.file_name().unwrap_or_default().to_string_lossy(),
                                &b.file_name().unwrap_or_default().to_string_lossy(),
                            )
                        });
                        Ok(children)
                    })();
                    let _ = sender.send(BackgroundMessage::NavigationChildren { path, result });
                });
            }
            SidebarAction::Tag(name) => {
                self.search.query = format!("tag:\"{}\"", name.replace('"', "\\\""));
                self.search.scope = SearchScope::IndexedFiles;
                self.execute_search();
            }
            SidebarAction::CreateTag => {
                self.name_dialog = Some(NameDialog {
                    command: FileCommand::Properties,
                    value: String::new(),
                    focus: true,
                })
            }
            SidebarAction::Unpin(path) => {
                self.config.sidebar.favorites.retain(|value| {
                    crate::shared::utils::parse_config_path(value).is_ok_and(|value| value != path)
                });
                self.persist_config();
            }
            SidebarAction::System => self.is_system = true,
            SidebarAction::Settings => self.open_settings(SettingsTab::General),
            SidebarAction::PinCurrent => {
                let path = crate::shared::utils::config_path(&self.current);
                if self.config.sidebar.favorites.contains(&path) {
                    self.info("This folder is already pinned");
                } else {
                    self.config.sidebar.favorites.push(path);
                    self.persist_config();
                }
            }
        }
    }
    fn preview_action(&mut self, action: PreviewAction, ctx: &egui::Context) {
        match action {
            PreviewAction::Open => self.command(FileCommand::Open, ctx),
            PreviewAction::Save => self.save_description(),
            PreviewAction::Discard => self.load_selection(),
            PreviewAction::CopyPath => self.command(FileCommand::CopyPath, ctx),
            PreviewAction::ReferenceSearch => {
                let Some(editor) = &self.editor else { return };
                let text = editor.picker_query.clone();
                self.reference_generation += 1;
                let generation = self.reference_generation;
                let directory = self.current.clone();
                let repository = self.dependencies.repository.clone();
                let sender = self.dependencies.sender.clone();
                rayon::spawn(move || {
                    let result = if let Ok(path) = crate::shared::utils::parse_config_path(&text) {
                        let path = crate::shared::utils::canonicalize(&path).unwrap_or(path);
                        repository
                            .get_file_by_path(&path)
                            .map(|file| file.into_iter().collect())
                            .map_err(|error| error.to_string())
                    } else {
                        parse_query(&text, SearchScope::IndexedFiles, directory, Vec::new(), 20)
                            .and_then(|query| {
                                repository
                                    .search_keyword(&query)
                                    .map(|hits| hits.into_iter().map(|hit| hit.file).collect())
                                    .map_err(|error| error.to_string())
                            })
                    };
                    let _ =
                        sender.send(BackgroundMessage::ReferenceCandidates { generation, result });
                });
            }
            PreviewAction::OpenReference(id) => self.request_action(PendingAction::Reveal(id)),
            PreviewAction::AddTag(tag) | PreviewAction::RemoveTag(tag) => {
                if self
                    .editor
                    .as_ref()
                    .is_some_and(DescriptionEditor::is_dirty)
                {
                    self.notice =
                        Some("Save or discard the description before editing tags".into());
                    return;
                }
                let is_add = matches!(action, PreviewAction::AddTag(_));
                let ids = self.selected.iter().cloned().collect::<Vec<_>>();
                let repository = self.dependencies.repository.clone();
                let sender = self.dependencies.sender.clone();
                rayon::spawn(move || {
                    let result = ids
                        .iter()
                        .try_for_each(|id| {
                            if is_add {
                                repository.assign_tag(id, tag)
                            } else {
                                repository.remove_tag_from_file(id, tag)
                            }
                        })
                        .map_err(|error| error.to_string());
                    let _ = sender.send(BackgroundMessage::MetadataChanged(result));
                });
            }
        }
    }
    fn reveal_reference(&mut self, id: String) {
        let repository = self.dependencies.repository.clone();
        let sender = self.dependencies.sender.clone();
        self.location_generation += 1;
        let generation = self.location_generation;
        self.pending_history = None;
        rayon::spawn(move || {
            let result = (|| {
                let file = repository
                    .get_file(&id)
                    .map_err(|error| error.to_string())?
                    .ok_or("Reference is unavailable")?;
                if file.is_missing || !file.path.exists() {
                    return Err(
                        "Reference target is unavailable; it is never rebound to a different file"
                            .into(),
                    );
                }
                let parent = file
                    .path
                    .parent()
                    .ok_or("Parent folder is unavailable")?
                    .to_path_buf();
                Ok((parent, Some(file.path)))
            })();
            let _ = sender.send(BackgroundMessage::LocationResolved { generation, result });
        });
    }
    fn agent_panel(&mut self, ui: &mut egui::Ui) {
        if !self.config.agent.enabled {
            ui.weak("Enable Agent and choose allowed roots in Settings");
            if ui.button("Open Agent settings").clicked() {
                self.open_settings(SettingsTab::Permissions);
            }
            return;
        }
        if self.config.agent.endpoints.is_empty() {
            ui.weak("Add an endpoint in Settings → Agent");
            return;
        }
        self.agent_endpoint = self
            .agent_endpoint
            .min(self.config.agent.endpoints.len() - 1);
        egui::ComboBox::from_id_salt("chat-endpoint")
            .selected_text(&self.config.agent.endpoints[self.agent_endpoint].name)
            .show_ui(ui, |ui| {
                for (index, endpoint) in self.config.agent.endpoints.iter().enumerate() {
                    ui.selectable_value(&mut self.agent_endpoint, index, &endpoint.name);
                }
            });
        ui.weak(format!(
            "Model: {}",
            self.config.agent.endpoints[self.agent_endpoint].model
        ));
        ui.add(
            egui::TextEdit::singleline(&mut self.agent_key)
                .password(true)
                .hint_text("Session credential"),
        );
        ui.checkbox(
            &mut self.include_selection,
            "Include selected file IDs as context",
        );
        egui::ScrollArea::vertical()
            .id_salt("agent-messages")
            .max_height((ui.available_height() - 150.0).max(120.0))
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for (index, message) in self.chat.iter().enumerate() {
                    ui.push_id(index, |ui| {
                        ui.strong(&message.role);
                        ui.add(egui::Label::new(&message.content).wrap().selectable(true));
                        ui.separator();
                    });
                }
                if self.chat.is_empty() {
                    ui.weak("Ask about files inside the allowed roots");
                }
            });
        ui.add(
            egui::TextEdit::multiline(&mut self.agent_input)
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.is_agent_busy && !self.agent_input.trim().is_empty(),
                    egui::Button::new("Send"),
                )
                .clicked()
            {
                let mut text = std::mem::take(&mut self.agent_input);
                if self.include_selection {
                    let mut ids = Vec::new();
                    for file in self
                        .files
                        .iter()
                        .filter(|file| self.selected.contains(&file.id))
                    {
                        if self
                            .dependencies
                            .policy
                            .authorize(&file.path, "metadata_read")
                            .is_ok()
                        {
                            ids.push(file.id.clone());
                        }
                    }
                    if !ids.is_empty() {
                        text.push_str(&format!("\nContext file IDs: {}", ids.join(", ")));
                    }
                }
                self.chat.push(ChatMessage {
                    role: "user".into(),
                    content: text,
                });
                let endpoint = self.config.agent.endpoints[self.agent_endpoint].clone();
                match self
                    .agent
                    .send(endpoint, self.agent_key.clone(), self.chat.clone())
                {
                    Ok(generation) => {
                        self.agent_generation = generation;
                        self.is_agent_busy = true;
                        self.chat.push(ChatMessage {
                            role: "assistant".into(),
                            content: String::new(),
                        });
                    }
                    Err(error) => self.notice = Some(error),
                }
            }
            if ui
                .add_enabled(self.is_agent_busy, egui::Button::new("Cancel"))
                .clicked()
            {
                self.cancel_agent();
            }
            if ui
                .add_enabled(!self.is_agent_busy, egui::Button::new("Clear chat"))
                .clicked()
            {
                self.chat.clear();
            }
        });
    }
    fn system_panel(&mut self, ui: &mut egui::Ui) {
        let mut navigate = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.strong(tr("Drives and locations"));
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    for (label, drive) in &self.sidebar.drives {
                        let card = egui::Frame::group(ui.style())
                            .corner_radius(8)
                            .inner_margin(egui::Margin::symmetric(12, 10))
                            .show(ui, |ui| {
                                ui.set_width(200.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(HARD_DRIVE)
                                            .size(22.0)
                                            .color(ui.visuals().selection.stroke.color),
                                    );
                                    ui.vertical(|ui| {
                                        ui.add(egui::Label::new(egui::RichText::new(label).strong()).truncate());
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(drive.display().to_string()).small().weak(),
                                            )
                                            .truncate(),
                                        );
                                    });
                                });
                            });
                        if card.response.interact(egui::Sense::click()).clicked() {
                            navigate = Some(drive.clone());
                        }
                    }
                    for share in &self.config.sidebar.shares {
                        if ui.button(format!("{NETWORK} {share}")).clicked() {
                            navigate = crate::shared::utils::parse_config_path(share).ok();
                        }
                    }
                });
                ui.weak(tr("Network shares and WSL paths can be added in Settings › System & transfer, or typed into the address bar (\\\\server\\share, \\\\wsl.localhost\\Ubuntu)."));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.strong(tr("Transfer queue"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                !self.operation_reports.is_empty(),
                                egui::Button::new(format!("{STOP} {}", tr("Cancel queued work"))),
                            )
                            .on_hover_text(tr("Stops at the next safe boundary; finished items stay"))
                            .clicked()
                        {
                            self.operations.cancel();
                        }
                        if ui
                            .button(format!("{ARROW_COUNTER_CLOCKWISE} {}", tr("Restore from app trash…")))
                            .clicked()
                        {
                            let sender = self.dependencies.sender.clone();
                            let trash = self.dependencies.paths.trash.clone();
                            let mut request = self.operation_request(OperationKind::Restore);
                            request.identities.clear();
                            request.revisions.clear();
                            std::thread::spawn(move || {
                                let Some(path) = rfd::FileDialog::new()
                                    .set_directory(trash)
                                    .set_title("Choose restore.json from an app trash entry")
                                    .pick_file()
                                else {
                                    return;
                                };
                                request.sources = vec![path];
                                let _ = sender.send(BackgroundMessage::OperationReview(request));
                            });
                        }
                    });
                });
                if self.operation_reports.is_empty() {
                    ui.weak(tr("No file operations in this session"));
                }
                for report in self.operation_reports.iter().rev() {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let finished = report.done + report.failures.len();
                        ui.horizontal(|ui| {
                            let (icon, color) = if report.is_cancelled {
                                (PROHIBIT, ui.visuals().weak_text_color())
                            } else if !report.failures.is_empty() {
                                (WARNING_CIRCLE, ui.visuals().error_fg_color)
                            } else if finished >= report.total {
                                (CHECK_CIRCLE, FilemoTheme::SUCCESS)
                            } else {
                                (SPINNER, ui.visuals().selection.stroke.color)
                            };
                            ui.label(egui::RichText::new(icon).color(color));
                            ui.add(
                                egui::ProgressBar::new(finished as f32 / report.total.max(1) as f32)
                                    .desired_width(160.0)
                                    .text(format!("{}/{}", report.done, report.total)),
                            );
                            ui.weak(&report.id[..8.min(report.id.len())]);
                        });
                        for error in &report.failures {
                            ui.colored_label(ui.visuals().error_fg_color, tr(error));
                        }
                    });
                }
                if !self.recovery.is_empty() {
                    ui.add_space(8.0);
                    ui.strong(tr("Unfinished operations from a previous run"));
                    ui.weak(tr("They were rolled back or cleaned up at startup. Check the paths before retrying."));
                    for (id, source, target, phase) in &self.recovery {
                        ui.label(format!(
                            "{}: {} → {} ({phase})",
                            &id[..8.min(id.len())],
                            source.display(),
                            target.display()
                        ));
                    }
                }
            });
        if let Some(path) = navigate {
            self.request_action(PendingAction::Navigate(path, None));
        }
    }
    fn cancel_agent(&mut self) {
        self.agent.cancel();
        self.is_agent_busy = false;
        if let Some((_, response)) = self.approval.take() {
            let _ = response.send(false);
        }
    }
    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some((id, name)) = self.delete_tag.clone() {
            let mut choice = None;
            egui::Modal::new(egui::Id::new("delete-tag")).show(ctx, |ui| {
                ui.heading("Delete tag");
                ui.label(format!(
                    "Remove {name} from the tag list and all assigned files?"
                ));
                ui.horizontal(|ui| {
                    if ui.button("Delete tag").clicked() {
                        choice = Some(true);
                    }
                    if ui.button("Cancel").clicked()
                        || ui.input(|input| input.key_pressed(egui::Key::Escape))
                    {
                        choice = Some(false);
                    }
                });
            });
            if let Some(choice) = choice {
                self.delete_tag = None;
                if choice {
                    let repository = self.dependencies.repository.clone();
                    let sender = self.dependencies.sender.clone();
                    rayon::spawn(move || {
                        let result = repository.delete_tag(id).map_err(|error| error.to_string());
                        if result.is_ok()
                            && let Ok(tags) = repository.get_all_tags()
                        {
                            let _ = sender.send(BackgroundMessage::TagsLoaded(tags));
                        }
                        let _ = sender.send(BackgroundMessage::MetadataChanged(result));
                    });
                }
            }
            return;
        }
        if let Some((intent, _)) = &self.approval {
            let mut choice = None;
            egui::Modal::new(egui::Id::new("agent-mutation-approval")).show(ctx,|ui| {
                ui.set_max_width(620.0);ui.heading("Agent requests a change");ui.label("Approval applies only to this operation, target IDs and revisions. Changed targets require a new review.");
                egui::ScrollArea::vertical().max_height(320.0).show(ui,|ui| { match intent {
                    MutationIntent::Description { id,revision,source }=>{ ui.label(format!("Replace description · file {id} · revision {revision}"));ui.add(egui::Label::new(source).wrap().selectable(true)); },
                    MutationIntent::Tags { id,revision,tags }=>{ ui.label(format!("Replace tags · file {id} · revision {revision}"));ui.label(format!("Tag IDs: {tags:?}")); },
                    MutationIntent::Files(request)=>{ ui.label(format!("{:?} {} target(s)",request.kind,request.sources.len()));ui.label(format!("Destination: {}",request.destination.display()));if !request.name.is_empty() { ui.label(format!("Name: {}",request.name)); }for source in &request.sources { ui.label(source.display().to_string()); }ui.label(format!("Revisions: {:?}",request.revisions));if request.kind==OperationKind::Delete { ui.colored_label(ui.visuals().error_fg_color,"Permanent deletion cannot be undone"); } },
                } });
                ui.horizontal(|ui| { if ui.button("Approve this change").clicked() { choice=Some(true); }if ui.button("Reject").clicked() || ui.input(|input| input.key_pressed(egui::Key::Escape)) { choice=Some(false); } });
            });
            if let Some(choice) = choice
                && let Some((_, response)) = self.approval.take()
                && response.send(choice).is_err()
                && choice
            {
                self.notice = Some("The approval request expired; ask the agent again".into());
            }
            return;
        }
        if self.pending.is_some() {
            let mut choice = None;
            egui::Modal::new(egui::Id::new("dirty-description")).show(ctx, |ui| {
                ui.heading("Unsaved description");
                ui.label("Save the description before changing selection or location?");
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.is_saving, egui::Button::new("Save"))
                        .clicked()
                    {
                        choice = Some(0);
                    }
                    if ui.button("Discard").clicked() {
                        choice = Some(1);
                    }
                    if ui.button("Cancel").clicked()
                        || ui.input(|input| input.key_pressed(egui::Key::Escape))
                    {
                        choice = Some(2);
                    }
                });
            });
            match choice {
                Some(0) => self.save_description(),
                Some(1) => {
                    if let Some(editor) = &mut self.editor {
                        editor.draft = editor.saved.clone();
                    }
                    if let Some(action) = self.pending.take() {
                        if matches!(action, PendingAction::Exit) {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        } else {
                            self.perform_action(action);
                        }
                    }
                }
                Some(2) => self.pending = None,
                _ => {}
            }
            return;
        }
        if let Some(request) = &mut self.confirmation {
            let mut submit = false;
            let mut cancel = false;
            egui::Modal::new(egui::Id::new("operation-review")).show(ctx, |ui| {
                ui.set_max_width(560.0);
                let (icon, title) = match request.kind {
                    OperationKind::Delete => (WARNING, tr("Delete permanently?")),
                    OperationKind::Copy => (COPY, tr("Paste — some names already exist")),
                    OperationKind::Move => (SCISSORS, tr("Move — some names already exist")),
                    OperationKind::Relocate => (MAP_PIN, tr("Relocate missing root")),
                    OperationKind::Restore => (ARROW_COUNTER_CLOCKWISE, tr("Restore from app trash")),
                    _ => (INFO, tr("Review operation")),
                };
                ui.heading(format!("{icon} {title}"));
                if matches!(request.kind, OperationKind::Delete | OperationKind::Trash) {
                    ui.label(crate::tr!("{count} item(s)", count = request.sources.len()));
                } else {
                    ui.label(crate::tr!(
                        "{count} item(s) → {destination}",
                        count = request.sources.len(),
                        destination = request.destination.display()
                    ));
                }
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .show(ui, |ui| {
                        for path in &request.sources {
                            ui.weak(path.display().to_string());
                        }
                    });
                if request.kind == OperationKind::Delete {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        tr("This bypasses the trash and cannot be undone."),
                    );
                }
                if request.kind == OperationKind::Relocate {
                    ui.label(crate::tr!("Map {count} indexed file identities to the new root. Files are not moved. Existing tags and descriptions will refer to the reviewed replacement paths.", count = request.revisions.len()));
                }
                if matches!(request.kind, OperationKind::Copy | OperationKind::Move) {
                    ui.separator();
                    ui.label(tr("When an item with the same name exists:"));
                    for (policy, label) in [
                        (CollisionPolicy::Skip, n_("Skip it")),
                        (CollisionPolicy::RenameCopy, n_("Keep both (add a number)")),
                        (CollisionPolicy::Replace, n_("Replace it (overwrites)")),
                        (CollisionPolicy::Merge, n_("Merge folders, skip existing files")),
                    ] {
                        ui.radio_value(&mut request.collision, policy, tr(label));
                    }
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let label = if request.kind == OperationKind::Delete {
                        egui::RichText::new(format!("{TRASH} {}", tr("Delete permanently")))
                            .color(ui.visuals().error_fg_color)
                    } else {
                        egui::RichText::new(format!("{CHECK} {}", tr("Continue")))
                    };
                    submit = ui.button(label).clicked()
                        || ui.input(|input| input.key_pressed(egui::Key::Enter));
                    cancel = ui.button(tr("Cancel")).clicked()
                        || ui.input(|input| input.key_pressed(egui::Key::Escape));
                });
            });
            if cancel {
                self.confirmation = None;
            } else if submit && let Some(mut request) = self.confirmation.take() {
                request.is_permanent_confirmed = request.kind == OperationKind::Delete;
                self.submit_operation(request);
            }
            return;
        }
        if let Some(dialog) = &mut self.name_dialog {
            let mut submit = false;
            let mut cancel = false;
            let is_tag = matches!(dialog.command, FileCommand::Properties);
            let validation = if is_tag {
                if dialog.value.trim().is_empty()
                    || dialog.value.len() > MAX_TAG_NAME_LENGTH
                    || dialog.value.chars().any(char::is_control)
                {
                    Err(crate::tr!(
                        "Tag names need 1–{max} characters without control characters",
                        max = MAX_TAG_NAME_LENGTH
                    ))
                } else {
                    Ok(())
                }
            } else {
                validate_component(&dialog.value)
            };
            egui::Modal::new(egui::Id::new("name-dialog")).show(ctx, |ui| {
                ui.set_min_width(380.0);
                ui.heading(match dialog.command {
                    FileCommand::Rename => format!("{PENCIL_SIMPLE} {}", tr("Rename")),
                    FileCommand::NewFolder => format!("{FOLDER_PLUS} {}", tr("New folder")),
                    FileCommand::NewFile => format!("{FILE_PLUS} {}", tr("New file")),
                    _ => format!("{TAG} {}", tr("Create tag")),
                });
                let response = ui.add(
                    egui::TextEdit::singleline(&mut dialog.value)
                        .id(egui::Id::new("name-dialog-input"))
                        .desired_width(f32::INFINITY),
                );
                if dialog.focus {
                    response.request_focus();
                    // Select the name without its extension, as Explorer does on F2.
                    let end = if matches!(dialog.command, FileCommand::Rename) {
                        dialog
                            .value
                            .rfind('.')
                            .filter(|index| *index > 0)
                            .map_or(dialog.value.chars().count(), |index| {
                                dialog.value[..index].chars().count()
                            })
                    } else {
                        dialog.value.chars().count()
                    };
                    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), response.id) {
                        state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::two(
                                egui::text::CCursor::new(0),
                                egui::text::CCursor::new(end),
                            )));
                        state.store(ui.ctx(), response.id);
                    }
                    dialog.focus = false;
                }
                match &validation {
                    Err(error) if !dialog.value.is_empty() => {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            format!("{WARNING_CIRCLE} {}", tr(error)),
                        );
                        if !is_tag {
                            let suggestion = crate::shared::utils::suggest_component(&dialog.value);
                            if ui
                                .link(crate::tr!("Use “{name}”", name = suggestion))
                                .clicked()
                            {
                                dialog.value = suggestion;
                            }
                        }
                    }
                    Ok(()) if !is_tag => {
                        if let Some(warning) =
                            crate::shared::utils::portability_warning(&dialog.value)
                        {
                            ui.colored_label(
                                FilemoTheme::WARNING,
                                format!("{WARNING} {}", tr(warning)),
                            );
                        }
                    }
                    _ => {}
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    submit = ui
                        .add_enabled(validation.is_ok(), egui::Button::new(tr("Confirm")))
                        .clicked()
                        || (validation.is_ok()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter)));
                    cancel = ui.button(tr("Cancel")).clicked()
                        || ui.input(|input| input.key_pressed(egui::Key::Escape));
                });
            });
            if cancel {
                self.name_dialog = None;
            } else if submit {
                let Some(dialog) = self.name_dialog.take() else {
                    return;
                };
                if matches!(dialog.command, FileCommand::Rename) {
                    // Renaming never overwrites (collisions are refused), so it runs directly.
                    let mut request = self.operation_request(OperationKind::Rename);
                    request.name = dialog.value;
                    self.submit_operation(request);
                } else if is_tag {
                    let repository = self.dependencies.repository.clone();
                    let sender = self.dependencies.sender.clone();
                    let hue = (self.tags.len() as f32 * 47.0) % 360.0;
                    rayon::spawn(move || {
                        let result = repository
                            .create_tag(dialog.value.trim(), hue, 0.65, 0.6)
                            .map_err(|error| error.to_string());
                        match result {
                            Ok(_) => {
                                if let Ok(tags) = repository.get_all_tags() {
                                    let _ = sender.send(BackgroundMessage::TagsLoaded(tags));
                                }
                            }
                            Err(error) => {
                                let _ = sender.send(BackgroundMessage::Diagnostic(Err(error)));
                            }
                        }
                    });
                } else {
                    let mut request = self.operation_request(
                        if matches!(dialog.command, FileCommand::NewFolder) {
                            OperationKind::CreateFolder
                        } else {
                            OperationKind::CreateFile
                        },
                    );
                    request.name = dialog.value;
                    request.sources = vec![self.current.clone()];
                    request.identities.clear();
                    request.revisions.clear();
                    self.submit_operation(request);
                }
            }
            return;
        }
        let action = self.settings.as_mut().and_then(|settings| {
            render_settings(
                ctx,
                settings,
                &self.dependencies.paths,
                self.backend,
                &self.model_status,
                self.pending_vectors,
                self.active_dimension,
            )
        });
        if let Some(action) = action {
            self.settings_action(action);
        }
    }
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.settings.is_some()
            || self.pending.is_some()
            || self.confirmation.is_some()
            || self.name_dialog.is_some()
            || self.approval.is_some()
            || self.delete_tag.is_some()
        {
            return;
        }
        // egui-winit turns Ctrl+C/X/V into Copy/Cut/Paste events instead of key presses, and
        // emits nothing on Ctrl+V when the clipboard holds files but no text. The key release
        // still arrives, so it is the fallback for pasting files.
        let (command, alt, shift, key, clipboard_event, paste_released, v_released) =
            ctx.input(|input| {
                let clipboard_event = input.events.iter().find_map(|event| match event {
                    egui::Event::Copy => Some(FileCommand::Copy),
                    egui::Event::Cut => Some(FileCommand::Cut),
                    egui::Event::Paste(_) => Some(FileCommand::Paste),
                    _ => None,
                });
                let paste_released = input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::V,
                            pressed: false,
                            modifiers,
                            ..
                        } if modifiers.command
                    )
                });
                let v_released = input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::V,
                            pressed: false,
                            ..
                        }
                    )
                });
                (
                    input.modifiers.command,
                    input.modifiers.alt,
                    input.modifiers.shift,
                    input.events.iter().find_map(|event| match event {
                        egui::Event::Key {
                            key, pressed: true, ..
                        } => Some(*key),
                        _ => None,
                    }),
                    clipboard_event,
                    paste_released,
                    v_released,
                )
            });
        if command && key == Some(egui::Key::L) {
            self.navigation.is_editing = true;
            self.navigation.focus_requested = true;
            self.navigation.address = crate::shared::utils::config_path(&self.current);
            return;
        }
        if command && key == Some(egui::Key::F) {
            self.search.focus_requested = true;
            return;
        }
        if command && key == Some(egui::Key::S) {
            if self
                .editor
                .as_ref()
                .is_some_and(DescriptionEditor::is_dirty)
            {
                self.save_description();
            }
            return;
        }
        if ctx.egui_wants_keyboard_input() {
            self.paste_seen = false;
            return;
        }
        match clipboard_event {
            Some(FileCommand::Paste) => {
                self.paste_seen = true;
                self.command(FileCommand::Paste, ctx);
            }
            Some(command) => self.command(command, ctx),
            None if paste_released && !self.paste_seen => self.command(FileCommand::Paste, ctx),
            None => {}
        }
        // Any release of V ends the key press, whichever modifier was let go first.
        if v_released && clipboard_event.is_none() {
            self.paste_seen = false;
        }
        if command && key == Some(egui::Key::P) {
            self.config.general.show_inspector = !self.config.general.show_inspector;
            self.persist_config();
            return;
        }
        if command && shift && key == Some(egui::Key::N) {
            self.command(FileCommand::NewFolder, ctx);
            return;
        }
        if alt {
            match key {
                Some(egui::Key::ArrowLeft) => self.history_move(-1),
                Some(egui::Key::ArrowRight) => self.history_move(1),
                Some(egui::Key::ArrowUp) => self.go_up(),
                _ => {}
            }
            return;
        }
        let action = match (command, key) {
            (true, Some(egui::Key::A)) => Some(FileCommand::SelectAll),
            (_, Some(egui::Key::F2)) => Some(FileCommand::Rename),
            (_, Some(egui::Key::Delete)) => Some(if shift {
                FileCommand::Delete
            } else {
                FileCommand::Trash
            }),
            (_, Some(egui::Key::Enter)) => Some(FileCommand::Open),
            _ => None,
        };
        if let Some(action) = action {
            self.command(action, ctx);
        }
        match key {
            Some(egui::Key::F5) => self.refresh(false),
            Some(egui::Key::Backspace) => self.go_up(),
            Some(egui::Key::Escape) if !self.selected.is_empty() => {
                self.request_action(PendingAction::Select(String::new(), false, false));
            }
            Some(egui::Key::ArrowDown | egui::Key::ArrowUp | egui::Key::Home | egui::Key::End)
                if !self.visible.is_empty() =>
            {
                let last = self.visible.len() - 1;
                let index = self
                    .cursor
                    .as_ref()
                    .or(self.anchor.as_ref())
                    .and_then(|id| self.visible.iter().position(|file| &file.id == id));
                let index = match (key, index) {
                    (Some(egui::Key::Home), _) => 0,
                    (Some(egui::Key::End), _) => last,
                    (Some(egui::Key::ArrowDown), Some(index)) => (index + 1).min(last),
                    (Some(egui::Key::ArrowUp), Some(index)) => index.saturating_sub(1),
                    _ => 0,
                };
                let id = self.visible[index].id.clone();
                self.scroll_to = Some(id.clone());
                self.request_action(PendingAction::Select(id, false, shift));
            }
            _ => {}
        }
    }
    fn history_move(&mut self, delta: isize) {
        let index = self.history_index as isize + delta;
        if index >= 0 && (index as usize) < self.history.len() {
            self.request_action(PendingAction::Navigate(
                self.history[index as usize].clone(),
                Some(index as usize),
            ));
        }
    }
    fn navigation_panel(&mut self, ui: &mut egui::Ui) {
        let context = SidebarContext {
            current: &self.current,
            favorites: &self.config.sidebar.favorites,
            frequent: &self.frequent,
            shares: &self.config.sidebar.shares,
            tags: &self.tags,
            is_system: self.is_system,
        };
        if let Some(action) = render_sidebar(
            ui,
            &self.sidebar,
            context,
            &mut self.config.sidebar.closed_sections,
        ) {
            self.sidebar_action(action);
        }
    }
    fn inspector_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (tab, label, icon) in [
                (InspectorTab::Preview, n_("Preview"), EYE),
                (InspectorTab::Details, n_("Details"), INFO),
                (InspectorTab::Agent, n_("Agent"), ROBOT),
            ] {
                if underline_tab(
                    ui,
                    &format!("{icon} {}", tr(label)),
                    self.config.general.inspector_tab == tab,
                )
                .clicked()
                {
                    self.config.general.inspector_tab = tab;
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(X).frame(false))
                    .on_hover_text(format!("{} · Ctrl+P", tr("Hide details pane")))
                    .clicked()
                {
                    self.config.general.show_inspector = false;
                    self.persist_config();
                }
            });
        });
        ui.separator();
        if self.config.general.inspector_tab == InspectorTab::Agent {
            self.agent_panel(ui);
            return;
        }
        let selected = self
            .files
            .iter()
            .find(|file| self.selected.contains(&file.id))
            .cloned();
        let mut action = None;
        egui::ScrollArea::vertical()
            .id_salt("inspector-content")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Leave room for the overlay scroll bar so right-aligned values stay visible.
                ui.set_max_width((ui.available_width() - 12.0).max(120.0));
                ui.add_enabled_ui(!self.is_saving, |ui| {
                    action = render_preview_panel(
                        ui,
                        selected.as_ref(),
                        self.selected.len(),
                        self.config.general.inspector_tab,
                        self.preview.as_ref(),
                        self.texture.as_ref(),
                        &mut self.editor,
                        &self.tags,
                        &self.sizes,
                        &self.references,
                    );
                });
            });
        if let Some(action) = action {
            self.preview_action(action, &ui.ctx().clone());
        }
    }
    fn info(&mut self, text: impl Into<String>) {
        self.push_toast(text.into(), false);
    }
    fn push_toast(&mut self, text: String, is_error: bool) {
        if text.is_empty() {
            return;
        }
        let text = tr(&text).into_owned();
        self.toasts.retain(|toast| toast.text != text);
        self.toasts.push(Toast {
            text,
            is_error,
            shown: Instant::now(),
        });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }
    /// Short-lived notifications in the bottom-right corner instead of a banner over the list.
    fn render_toasts(&mut self, ctx: &egui::Context) {
        let lifetime = |toast: &Toast| {
            Duration::from_secs(if toast.is_error {
                TOAST_SECONDS * 2
            } else {
                TOAST_SECONDS
            })
        };
        self.toasts
            .retain(|toast| toast.shown.elapsed() < lifetime(toast));
        if self.toasts.is_empty() {
            return;
        }
        let next = self
            .toasts
            .iter()
            .map(|toast| lifetime(toast).saturating_sub(toast.shown.elapsed()))
            .min()
            .unwrap_or_default();
        ctx.request_repaint_after(next);
        let mut dismissed = None;
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(
                egui::Align2::RIGHT_BOTTOM,
                egui::vec2(-16.0, -(STATUS_BAR_HEIGHT + 12.0)),
            )
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_max_width(380.0);
                for (index, toast) in self.toasts.iter().enumerate() {
                    let accent = if toast.is_error {
                        ui.visuals().error_fg_color
                    } else {
                        ui.visuals().selection.stroke.color
                    };
                    egui::Frame::popup(ui.style())
                        .stroke(egui::Stroke::new(1.0_f32, accent.gamma_multiply(0.6)))
                        .inner_margin(egui::Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(if toast.is_error {
                                        WARNING_CIRCLE
                                    } else {
                                        INFO
                                    })
                                    .color(accent),
                                );
                                ui.add(egui::Label::new(&toast.text).wrap());
                                if ui.add(egui::Button::new(X).frame(false)).clicked() {
                                    dismissed = Some(index);
                                }
                            });
                        });
                    ui.add_space(6.0);
                }
            });
        if let Some(index) = dismissed {
            self.toasts.remove(index);
        }
    }
    /// One line of facts on the left, compact status indicators with tooltips on the right.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            let folders = self.visible.iter().filter(|file| file.is_directory).count();
            ui.label(count_label(
                self.visible.len(),
                "{count} item",
                "{count} items",
            ))
            .on_hover_text(format!(
                "{} · {}",
                count_label(folders, "{count} folder", "{count} folders"),
                count_label(
                    self.visible.len() - folders,
                    "{count} file",
                    "{count} files"
                )
            ));
            if !self.selected.is_empty() {
                ui.separator();
                ui.label(crate::tr!("{count} selected", count = self.selected.len()));
                ui.weak(&self.selection_total);
            } else if let Some(size) = self.sizes.get(&self.current) {
                ui.separator();
                ui.weak(crate::tr!("Folder {size}", size = size_label_for(size)));
            }
            if let Some((path, free, total)) = &self.disk_space
                && path == &self.current
            {
                ui.separator();
                let used = 1.0 - *free as f32 / (*total).max(1) as f32;
                ui.add(
                    egui::ProgressBar::new(used)
                        .desired_width(60.0)
                        .desired_height(6.0),
                )
                .on_hover_text(crate::tr!(
                    "{used}% of this drive is used",
                    used = (used * 100.0).round()
                ));
                ui.weak(crate::tr!(
                    "{free} free of {total}",
                    free = crate::shared::utils::format_size(*free),
                    total = crate::shared::utils::format_size(*total)
                ));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let (color, state) = match self.backend {
                    ServiceStatus::Online => (FilemoTheme::SUCCESS, tr("online")),
                    ServiceStatus::Offline => (FilemoTheme::DANGER, tr("offline")),
                    ServiceStatus::Disabled => (ui.visuals().weak_text_color(), tr("disabled")),
                };
                let mut tooltip = format!(
                    "Qdrant: {state}\n{}: {}\n{}: {}",
                    tr("Model"),
                    self.model_status,
                    tr("Pending sync"),
                    self.pending_vectors
                );
                if let Some(error) = &self.vector_error {
                    tooltip.push_str(&format!("\n\n{}", tr(error)));
                }
                tooltip.push_str(&format!(
                    "\n\n{}",
                    tr("Click to open Index & vector settings")
                ));
                if status_indicator(ui, DATABASE, color, &tr("Vector"))
                    .on_hover_text(tooltip)
                    .clicked()
                {
                    self.open_settings(SettingsTab::Index);
                }
                if self.pending_vectors > 0 && self.backend == ServiceStatus::Online {
                    ui.weak(format!("{ARROWS_CLOCKWISE} {}", self.pending_vectors))
                        .on_hover_text(tr("Metadata revisions waiting for vector sync"));
                }
                let running = self.operation_reports.iter().rev().find(|report| {
                    report.done + report.failures.len() < report.total && !report.is_cancelled
                });
                if let Some(report) = running {
                    ui.add(
                        egui::ProgressBar::new(
                            (report.done + report.failures.len()) as f32
                                / report.total.max(1) as f32,
                        )
                        .desired_width(80.0)
                        .text(format!("{}/{}", report.done, report.total)),
                    )
                    .on_hover_text(tr("File operation in progress — open System & transfers"));
                }
                if self.is_agent_busy
                    && ui
                        .add(egui::Button::new(format!("{ROBOT} {}", tr("Cancel agent"))).small())
                        .clicked()
                {
                    self.cancel_agent();
                }
                if self.is_scanning || self.search.is_searching {
                    ui.add(egui::Spinner::new().size(12.0))
                        .on_hover_text(if self.is_scanning {
                            tr("Reading folder")
                        } else {
                            tr("Searching")
                        });
                }
            });
        });
    }
    /// Browse / System tabs and the folder heading above the listing.
    fn central_header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if underline_tab(
                ui,
                &format!("{FOLDER_OPEN} {}", tr("Browse")),
                !self.is_system,
            )
            .clicked()
            {
                self.is_system = false;
            }
            if underline_tab(
                ui,
                &format!("{HARD_DRIVES} {}", tr("System")),
                self.is_system,
            )
            .clicked()
            {
                self.is_system = true;
            }
        });
        if self.is_system {
            return;
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let title = if self.is_search_active {
                tr("Search results").into_owned()
            } else {
                self.current
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| self.current.display().to_string())
            };
            ui.label(
                egui::RichText::new(if self.is_search_active {
                    MAGNIFYING_GLASS
                } else {
                    FOLDER_OPEN
                })
                .size(24.0)
                .color(ui.visuals().selection.stroke.color),
            );
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(title).size(18.0).strong());
                let folders = self.visible.iter().filter(|file| file.is_directory).count();
                let sort = match self.config.files.sort {
                    SortColumn::Name => n_("name"),
                    SortColumn::Size => n_("size"),
                    SortColumn::Modified => n_("date modified"),
                    SortColumn::Extension => n_("type"),
                    SortColumn::Created => n_("date created"),
                    SortColumn::Tags => n_("tags"),
                    SortColumn::Relevance => n_("relevance"),
                };
                ui.weak(format!(
                    "{} · {} · {}",
                    count_label(folders, "{count} folder", "{count} folders"),
                    count_label(
                        self.visible.len() - folders,
                        "{count} file",
                        "{count} files"
                    ),
                    crate::tr!("Sorted by {sort}", sort = tr(sort))
                ));
            });
        });
        ui.add_space(4.0);
    }
    fn open_settings(&mut self, tab: SettingsTab) {
        let mut settings = SettingsState::new(&self.config);
        settings.tab = tab;
        settings.languages = crate::shared::i18n::available(&self.dependencies.paths.root);
        self.settings = Some(settings);
    }
}

#[derive(Clone)]
struct Toast {
    text: String,
    is_error: bool,
    shown: Instant,
}

/// Singular/plural pair as separate translation keys, so every language can phrase both.
fn count_label(count: usize, one: &str, many: &str) -> String {
    crate::tr!(if count == 1 { one } else { many }, count = count)
}

fn size_label_for(size: &FolderSize) -> String {
    format!(
        "{}{}",
        match size.state {
            SizeState::Complete => "",
            SizeState::Partial => "≥ ",
            SizeState::Stale => "~",
        },
        crate::shared::utils::format_size(size.bytes)
    )
}

/// Text tab with an accent underline when active (inspector and Browse/System).
fn underline_tab(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let text = if selected {
        egui::RichText::new(label).strong()
    } else {
        egui::RichText::new(label).color(ui.visuals().weak_text_color())
    };
    let response = ui.add(egui::Button::new(text).frame(false));
    if selected {
        let rect = response.rect;
        ui.painter().line_segment(
            [
                egui::pos2(rect.left() + 4.0, rect.bottom() + 2.0),
                egui::pos2(rect.right() - 4.0, rect.bottom() + 2.0),
            ],
            egui::Stroke::new(2.0_f32, ui.visuals().selection.stroke.color),
        );
    }
    response
}

/// A coloured dot with an icon and short label; details live in the tooltip.
fn status_indicator(
    ui: &mut egui::Ui,
    icon: &str,
    color: egui::Color32,
    label: &str,
) -> egui::Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
        ui.painter().circle_filled(dot.center(), 4.0, color);
        ui.add(
            egui::Button::new(
                egui::RichText::new(format!("{icon} {label}"))
                    .color(ui.visuals().weak_text_color()),
            )
            .frame(false),
        )
    })
    .inner
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

impl eframe::App for FilemoApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.process_messages(&ctx);
        if let Some(error) = self.notice.take() {
            self.push_toast(error, true);
        }
        self.shortcuts(&ctx);
        let focused = ctx.input(|input| input.focused);
        if focused && (!self.was_focused || self.last_clipboard.elapsed() > Duration::from_secs(3))
        {
            self.dependencies.clipboard.refresh();
            self.last_clipboard = Instant::now();
        }
        self.was_focused = focused;
        if ctx.input(|input| input.viewport().close_requested()) {
            if self
                .editor
                .as_ref()
                .is_some_and(DescriptionEditor::is_dirty)
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.pending = Some(PendingAction::Exit);
            } else {
                self.config.general.last_path = crate::shared::utils::config_path(&self.current);
                self.config.general.maximized =
                    ctx.input(|input| input.viewport().maximized.unwrap_or(false));
                self.config.general.last_window_width =
                    ((ctx.content_rect().width() * ctx.zoom_factor()) as u32)
                        .clamp(MIN_WINDOW_WIDTH, MAX_WINDOW_SIZE);
                self.config.general.last_window_height =
                    ((ctx.content_rect().height() * ctx.zoom_factor()) as u32)
                        .clamp(MIN_WINDOW_HEIGHT, MAX_WINDOW_SIZE);
                self.persist_config();
            }
        }
        if self.search_changed.is_some_and(|changed| {
            changed.elapsed() >= Duration::from_millis(self.config.search.debounce_ms)
        }) {
            self.execute_search();
        }
        if self.canvas_dirty
            && self.last_canvas.elapsed() >= Duration::from_millis(CANVAS_REBUILD_INTERVAL_MS)
        {
            self.rebuild_canvas();
        } else if self.canvas_dirty {
            ctx.request_repaint_after(Duration::from_millis(CANVAS_REBUILD_INTERVAL_MS));
        }
        self.navigation.can_back = self.history_index > 0;
        self.navigation.can_forward = self.history_index + 1 < self.history.len();
        let location = self
            .current
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.current.display().to_string());
        egui::Panel::top("navigation").show_inside(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let search_width = (ui.available_width() * 0.3).clamp(200.0, 380.0);
                if let Some(action) = render_breadcrumb(
                    ui,
                    &self.current,
                    &self.sidebar.drives,
                    &mut self.navigation,
                    search_width,
                ) {
                    match action {
                        BreadcrumbAction::NavigateTo(path) => {
                            self.request_action(PendingAction::Navigate(path, None))
                        }
                        BreadcrumbAction::Submit(path) => {
                            self.request_action(PendingAction::Navigate(PathBuf::from(path), None))
                        }
                        BreadcrumbAction::Back => self.history_move(-1),
                        BreadcrumbAction::Forward => self.history_move(1),
                        BreadcrumbAction::Up => self.go_up(),
                        BreadcrumbAction::Refresh => self.refresh(false),
                        BreadcrumbAction::PickFolder => {
                            let sender = self.dependencies.sender.clone();
                            std::thread::spawn(move || {
                                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                                    let _ = sender.send(BackgroundMessage::PickedLocation(path));
                                }
                            });
                        }
                    }
                }
                if let Some(action) = render_search_bar(ui, &mut self.search, &location) {
                    match action {
                        SearchAction::Execute => self.execute_search(),
                        SearchAction::Changed => {
                            self.search_changed = Some(Instant::now());
                            ctx.request_repaint_after(Duration::from_millis(
                                self.config.search.debounce_ms,
                            ));
                        }
                        SearchAction::Clear => {
                            self.search.query.clear();
                            self.execute_search();
                        }
                    }
                }
            });
            if let Some(error) = &self.search.error {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("{WARNING_CIRCLE} {}", tr(error)),
                );
            }
            ui.add_space(4.0);
        });
        egui::Panel::top("file-toolbar").show_inside(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status")
            .exact_size(STATUS_BAR_HEIGHT)
            .show_inside(ui, |ui| self.status_bar(ui));
        let width = ui.available_width();
        let panels_width = if self.config.general.show_sidebar {
            self.config.general.sidebar_width
        } else {
            0.0
        } + if self.config.general.show_inspector {
            self.config.general.inspector_width
        } else {
            0.0
        };
        let use_overlays = width < FILE_CANVAS_MIN_WIDTH + panels_width + OVERLAY_MARGIN;
        if self.config.general.show_sidebar && !use_overlays {
            let response = egui::Panel::left("navigation-pane")
                .resizable(true)
                .default_size(self.config.general.sidebar_width)
                .size_range(SIDEBAR_MIN_WIDTH..=400.0)
                .show_inside(ui, |ui| self.navigation_panel(ui));
            self.config.general.sidebar_width = response
                .response
                .rect
                .width()
                .clamp(SIDEBAR_MIN_WIDTH, 400.0);
        }
        if self.config.general.show_inspector && !use_overlays {
            let response = egui::Panel::right("inspector")
                .resizable(true)
                .default_size(self.config.general.inspector_width)
                .size_range(INSPECTOR_MIN_WIDTH..=600.0)
                .show_inside(ui, |ui| self.inspector_panel(ui));
            self.config.general.inspector_width = response
                .response
                .rect
                .width()
                .clamp(INSPECTOR_MIN_WIDTH, 600.0);
        }
        egui::CentralPanel::default().show_inside(ui, |ui| {
            self.central_header(ui);
            if self.is_system {
                self.system_panel(ui);
                return;
            }
            if self.visible.is_empty() {
                ui.centered_and_justified(|ui| {
                    let (icon, text) = if self.is_scanning {
                        (HOURGLASS, tr("Loading folder"))
                    } else if self.search.is_searching {
                        (MAGNIFYING_GLASS, tr("Searching"))
                    } else if self.is_search_active {
                        (MAGNIFYING_GLASS, tr("No results for this query"))
                    } else {
                        (FOLDER_DASHED, tr("This folder is empty"))
                    };
                    ui.label(
                        egui::RichText::new(format!("{icon}\n{text}"))
                            .size(15.0)
                            .color(ui.visuals().weak_text_color()),
                    );
                });
                return;
            }
            let scroll_to = self.scroll_to.take().and_then(|id| {
                self.canvas.rows.iter().position(|row| {
                    matches!(row, CanvasRow::File(index) if self.visible.get(*index).is_some_and(|file| file.id == id))
                })
            });
            let action = render_file_table(
                ui,
                &self.visible,
                &self.selected,
                &self.config.files,
                &self.sizes,
                &mut self.canvas,
                TableContext {
                    can_paste: self.clipboard.is_some(),
                    is_global: self.is_search_active
                        && self.search.scope != SearchScope::CurrentFolder,
                    clipboard: self.clipboard.as_ref(),
                    scroll_to,
                },
            );
            for path in &self.canvas.visible_folders {
                if !self.requested_sizes.contains(path)
                    && self
                        .sizes_worker
                        .request(path.clone(), self.scan_generation)
                {
                    self.requested_sizes.insert(path.clone());
                }
            }
            if let Some(action) = action {
                match action {
                    TableAction::Select {
                        id,
                        is_toggle,
                        is_range,
                    } => self.request_action(PendingAction::Select(id, is_toggle, is_range)),
                    TableAction::Open(id) => self.open(&id),
                    TableAction::Command { id, command } => {
                        self.request_action(PendingAction::Command(id, command));
                    }
                    TableAction::Background(command) => self.command(command, &ctx),
                    TableAction::Sort(column) => {
                        if self.config.files.sort == column {
                            self.config.files.ascending = !self.config.files.ascending;
                        } else {
                            self.config.files.sort = column;
                            self.config.files.ascending = true;
                        }
                        self.rebuild_canvas();
                        self.persist_config();
                    }
                    TableAction::Group(name) => {
                        if !self.canvas.collapsed.remove(&name) {
                            self.canvas.collapsed.insert(name);
                        }
                        self.canvas
                            .rebuild(&self.visible, &self.sizes, self.config.files.group);
                    }
                }
            }
        });
        if use_overlays {
            let maximum_width =
                (ctx.content_rect().width() - OVERLAY_MARGIN).max(SIDEBAR_MIN_WIDTH);
            if self.config.general.show_sidebar {
                let mut open = true;
                egui::Window::new(tr("Navigation"))
                    .id(egui::Id::new("navigation-overlay"))
                    .open(&mut open)
                    .default_width(self.config.general.sidebar_width.min(maximum_width))
                    .max_width(maximum_width)
                    .max_height((ctx.content_rect().height() - OVERLAY_MARGIN).max(100.0))
                    .show(&ctx, |ui| self.navigation_panel(ui));
                self.config.general.show_sidebar = open;
            }
            if self.config.general.show_inspector {
                let mut open = true;
                egui::Window::new(tr("Details"))
                    .id(egui::Id::new("inspector-overlay"))
                    .open(&mut open)
                    .default_width(self.config.general.inspector_width.min(maximum_width))
                    .max_width(maximum_width)
                    .max_height((ctx.content_rect().height() - OVERLAY_MARGIN).max(100.0))
                    .show(&ctx, |ui| self.inspector_panel(ui));
                self.config.general.show_inspector = open;
            }
        }
        self.dialogs(&ctx);
        self.render_toasts(&ctx);
    }
}
impl Drop for FilemoApp {
    fn drop(&mut self) {
        self.dependencies.sender.close();
        self.config.general.last_path = crate::shared::utils::config_path(&self.current);
        let _ = self.configuration_worker.flush(self.config.clone());
        self.download_cancel.store(true, Ordering::Release);
        self.cancel_agent();
        self.scanner.cancel();
        self.search_worker.cancel();
        self.operations.shutdown();
        self.vectors.shutdown();
    }
}

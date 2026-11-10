use crate::shared::constants::*;
use crate::shared::i18n::n_;
use crate::shared::models::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    Appearance,
    Files,
    Search,
    Index,
    Agent,
    Permissions,
    System,
    Storage,
}
pub struct SettingsState {
    pub draft: AppConfig,
    pub tab: SettingsTab,
    pub artifact: Option<ModelArtifact>,
    pub downloaded: Option<VerifiedModel>,
    pub download: Option<(u64, u64)>,
    pub is_busy: bool,
    pub error: Option<String>,
    pub endpoint: usize,
    pub session_key: String,
    pub runtime_version: String,
    pub review_rebuild: bool,
    pub review_restore: bool,
    pub review_rollback: bool,
    pub relocation_old: String,
    pub relocation_new: String,
    /// `(code, name)` of installed locales, filled when the dialog opens.
    pub languages: Vec<(String, String)>,
}
impl SettingsState {
    pub fn new(config: &AppConfig) -> Self {
        Self {
            draft: config.clone(),
            tab: SettingsTab::General,
            artifact: None,
            downloaded: None,
            download: None,
            is_busy: false,
            error: None,
            endpoint: 0,
            session_key: String::new(),
            runtime_version: String::new(),
            review_rebuild: false,
            review_restore: false,
            review_rollback: false,
            relocation_old: String::new(),
            relocation_new: String::new(),
            languages: Vec::new(),
        }
    }
}
pub enum SettingsAction {
    Apply,
    Cancel,
    Inspect,
    Download,
    CancelDownload,
    Load,
    Unload,
    Synchronize,
    Pause,
    Rebuild,
    Rollback,
    Test(AgentEndpoint, String),
    Register(String, String),
    Backup,
    Restore,
    ClearCaches,
    Diagnostic,
    Relocate(String, String),
}

pub fn render_settings(
    ctx: &egui::Context,
    state: &mut SettingsState,
    paths: &RuntimePaths,
    backend: ServiceStatus,
    model: &str,
    pending: usize,
    active_dimension: usize,
) -> Option<SettingsAction> {
    let mut action = None;
    egui::Modal::new(egui::Id::new("settings-modal")).show(ctx,|ui| {
        ui.set_width((ctx.content_rect().width()-64.0).clamp(220.0,900.0));ui.heading(crate::shared::i18n::tr("Settings"));ui.separator();
        let narrow = ui.available_width() < 600.0;
        let tabs = |ui: &mut egui::Ui, state: &mut SettingsState| {
            for (tab, label) in [(SettingsTab::General,n_("General")),(SettingsTab::Appearance,n_("Appearance")),(SettingsTab::Files,n_("Navigation & files")),(SettingsTab::Search,n_("Search")),(SettingsTab::Index,n_("Index & vector")),(SettingsTab::Agent,n_("Agent")),(SettingsTab::Permissions,n_("Permissions & MCP")),(SettingsTab::System,n_("System & transfer")),(SettingsTab::Storage,n_("Storage & diagnostics"))] {
                ui.selectable_value(&mut state.tab, tab, crate::shared::i18n::tr(label));
            }
        };
        if narrow {
            ui.horizontal_wrapped(|ui| tabs(ui, state));
            ui.separator();
        }
        // A fixed tab column beside a top-down content column; `with_layout` here would take
        // all remaining width and lay the content out horizontally.
        let content_height = (ctx.content_rect().height() - 200.0).max(80.0);
        ui.horizontal_top(|ui| {
            if !narrow {
                ui.vertical(|ui| {
                    ui.set_width(170.0);
                    tabs(ui, state);
                });
                // Fixed-height divider: a separator here sizes itself to the available height,
                // which grows with the modal and makes the dialog grow every frame.
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(9.0, content_height), egui::Sense::hover());
                ui.painter().vline(
                    rect.center().x,
                    rect.y_range(),
                    ui.visuals().widgets.noninteractive.bg_stroke,
                );
            }
            ui.vertical(|ui| {
            egui::ScrollArea::vertical().id_salt("settings-content").auto_shrink([false, false]).max_height(content_height).show(ui,|ui| {
                ui.set_max_width(ui.available_width());
                match state.tab {
                    SettingsTab::General=>{
                        ui.horizontal(|ui| {
                            ui.label(crate::shared::i18n::tr("Language"));
                            let current = state.languages.iter().find(|(code, _)| *code == state.draft.general.language).map_or(state.draft.general.language.clone(), |(_, name)| name.clone());
                            egui::ComboBox::from_id_salt("language").selected_text(current).show_ui(ui, |ui| {
                                for (code, name) in &state.languages {
                                    ui.selectable_value(&mut state.draft.general.language, code.clone(), name);
                                }
                            });
                        });
                        ui.weak(crate::shared::i18n::tr("Add a translation by placing <code>.toml in the locales folder beside the app. Untranslated text stays in English."));
                        ui.separator();
                        ui.checkbox(&mut state.draft.general.restore_session,"Restore last location");ui.label("Startup / last folder");ui.text_edit_singleline(&mut state.draft.general.last_path);
                        ui.horizontal(|ui| { ui.label("Log retention (days)");ui.add(egui::DragValue::new(&mut state.draft.general.log_retention_days).range(1..=90)); });
                        ui.weak("Closing stops the app-owned workers and native processes. Logs stay beside the executable.");
                    }
                    SettingsTab::Appearance=>{
                        ui.horizontal(|ui| { ui.label("Theme");for (value,label) in [("dark","Dark"),("light","Light"),("system","System")] { ui.selectable_value(&mut state.draft.general.theme,value.to_string(),label); } });
                        ui.add(egui::Slider::new(&mut state.draft.general.font_size,10.0..=24.0).text("Font size"));ui.add(egui::Slider::new(&mut state.draft.general.ui_scale,0.75..=2.5).text("UI scale"));
                        ui.checkbox(&mut state.draft.general.show_sidebar,"Show navigation pane");ui.checkbox(&mut state.draft.general.show_inspector,"Show inspector");
                        ui.add(egui::Slider::new(&mut state.draft.general.sidebar_width,160.0..=480.0).text("Navigation width"));ui.add(egui::Slider::new(&mut state.draft.general.inspector_width,240.0..=600.0).text("Inspector width"));
                        ui.checkbox(&mut state.draft.files.compact,"Compact view");ui.checkbox(&mut state.draft.files.checkboxes,"Item check boxes");
                        egui::ComboBox::from_id_salt("default-view").selected_text(format!("{:?}",state.draft.files.view)).show_ui(ui,|ui| { for view in [ViewMode::Details,ViewMode::List,ViewMode::SmallIcons,ViewMode::MediumIcons,ViewMode::LargeIcons,ViewMode::ExtraLargeIcons,ViewMode::Tiles,ViewMode::Content] { ui.selectable_value(&mut state.draft.files.view,view,format!("{view:?}")); } });
                    }
                    SettingsTab::Files=>{
                        ui.checkbox(&mut state.draft.files.folders_first,"Folders first");ui.checkbox(&mut state.draft.files.show_extensions,"Show extensions");ui.checkbox(&mut state.draft.files.show_hidden,"Show hidden files");ui.checkbox(&mut state.draft.scanner.watch_enabled,"Watch current folder");
                        ui.label("Collision policy");ui.radio_value(&mut state.draft.files.collision,CollisionPolicy::Skip,"Skip existing targets");ui.radio_value(&mut state.draft.files.collision,CollisionPolicy::RenameCopy,"Create a renamed copy");ui.radio_value(&mut state.draft.files.collision,CollisionPolicy::Replace,"Replace (overwrite)");ui.radio_value(&mut state.draft.files.collision,CollisionPolicy::Merge,"Merge folders");
                        ui.checkbox(&mut state.draft.files.trash_fallback,"Allow application trash if OS trash fails").on_hover_text("Stores a restore manifest beside this app. Never falls back to permanent deletion.");
                        ui.weak("Links and reparse points are not traversed by file operations. Folder navigation remains explicit.");
                        path_list(ui,"Favorites",&mut state.draft.sidebar.favorites);path_list(ui,"Excluded paths",&mut state.draft.scanner.excluded_paths);
                        ui.horizontal(|ui| { ui.label("Size traversal entries");ui.add(egui::DragValue::new(&mut state.draft.scanner.size_entry_budget).range(100..=50_000_000)); });ui.horizontal(|ui| { ui.label("Size traversal seconds");ui.add(egui::DragValue::new(&mut state.draft.scanner.size_time_budget_seconds).range(1..=600)); });
                    }
                    SettingsTab::Search=>{
                        ui.label("Default scope");for scope in [SearchScope::CurrentFolder,SearchScope::IndexedFiles,SearchScope::SelectedRoots] { ui.radio_value(&mut state.draft.search.scope,scope,format!("{scope:?}")); }
                        ui.horizontal(|ui| { for mode in [SearchMode::Keyword,SearchMode::Semantic,SearchMode::Hybrid] { ui.radio_value(&mut state.draft.search.mode,mode,format!("{mode:?}")); } });
                        ui.add(egui::Slider::new(&mut state.draft.search.debounce_ms,100..=2000).text("Debounce (ms)"));ui.add(egui::Slider::new(&mut state.draft.search.limit,1..=MAX_RESULTS).text("Result limit"));
                        path_list(ui,"Recursive fff search roots",&mut state.draft.search.roots);ui.weak("Browsing only lists one folder. Recursive name indexing starts for selected roots when searched.");
                        ui.label("intitle:\"quarterly report\" type:document tag:work date:2026-10-01..2026-10-15");ui.weak("Filters combine with AND. Categories: document, image, video, audio, archive, folder.");
                    }
                    SettingsTab::Index=>{
                        ui.checkbox(&mut state.draft.qdrant.enabled,"Enable local Qdrant");ui.label(format!("Qdrant: {backend:?}"));ui.label(format!("Model: {model}"));ui.label(format!("Pending metadata revisions: {pending}"));ui.label(format!("Active dimensions: {active_dimension}"));ui.separator();
                        ui.strong("Qwen3-Embedding-0.6B");ui.label("Hugging Face model repository");ui.text_edit_singleline(&mut state.draft.embedding.repo_url);
                        if ui.button("Use default model").clicked() { state.draft.embedding.repo_url=DEFAULT_MODEL_REPO.into();state.draft.embedding.dimension=DEFAULT_DIMENSIONS;state.artifact=None; }
                        if ui.add_enabled(!state.is_busy,egui::Button::new("Inspect repository")).clicked() { action=Some(SettingsAction::Inspect); }
                        if let Some(artifact)=&state.artifact {
                            ui.label(format!("{} / {}",artifact.repo,artifact.filename));ui.label(format!("Revision: {}",artifact.revision));ui.label(format!("License: {} · {}",artifact.license,crate::shared::utils::format_size(artifact.bytes)));ui.small(format!("SHA-256: {}",artifact.sha256));
                            if ui.add_enabled(!state.is_busy,egui::Button::new("Download verified artifact")).clicked() { action=Some(SettingsAction::Download); }
                        }
                        if let Some((done,total))=state.download { ui.add(egui::ProgressBar::new(done as f32/total.max(1) as f32).show_percentage());if state.is_busy && ui.button("Cancel download").clicked() { action=Some(SettingsAction::CancelDownload); } }
                        if state.downloaded.is_some() { ui.label("Verified model available on disk"); }
                        ui.add(egui::Slider::new(&mut state.draft.embedding.dimension,MIN_QWEN_DIMENSIONS..=DEFAULT_DIMENSIONS).text("Requested output dimensions"));
                        if ui.add_enabled(!state.is_busy,egui::Button::new("Review model / dimension change")).clicked() { state.review_rebuild=true; }
                        if state.review_rebuild {
                            ui.group(|ui| { ui.label(format!("Rebuild {active_dimension} → {} dimensions",state.draft.embedding.dimension));ui.label("This loads the verified model and builds a new vector generation. It uses additional RAM, CPU and disk. Tags and descriptions stay in SQLite. The current generation stays active until verification completes.");ui.horizontal(|ui| { if ui.button("Start rebuild").clicked() { action=Some(SettingsAction::Rebuild);state.review_rebuild=false; }if ui.button("Cancel change").clicked() { state.review_rebuild=false; } }); });
                        }
                        if ui.add_enabled(!state.is_busy, egui::Button::new("Review rollback to previous model")).clicked() { state.review_rollback = true; }
                        if state.review_rollback { ui.label("Rebuild using the previous model and dimensions with current SQLite metadata. The active generation stays available until verification completes.");ui.horizontal(|ui| { if ui.button("Rebuild previous model").clicked() { state.review_rollback=false; action=Some(SettingsAction::Rollback); }if ui.button("Keep current model").clicked() { state.review_rollback=false; } }); }
                        ui.separator();ui.checkbox(&mut state.draft.qdrant.auto_sync,"Automatically synchronize saved metadata");ui.weak("Model loads when needed and unloads after the idle timeout.");
                        ui.add(egui::Slider::new(&mut state.draft.embedding.idle_seconds,30..=3600).text("Idle timeout (seconds)"));ui.horizontal(|ui| { ui.label("CPU threads");ui.add(egui::DragValue::new(&mut state.draft.embedding.threads).range(1..=64)); });ui.horizontal(|ui| { ui.label("Context tokens");ui.add(egui::DragValue::new(&mut state.draft.embedding.context_tokens).range(512..=32768)); });
                        ui.horizontal_wrapped(|ui| { for (label,value) in [("Synchronize",SettingsAction::Synchronize),("Pause",SettingsAction::Pause),("Load now",SettingsAction::Load),("Unload now",SettingsAction::Unload)] { if ui.button(label).clicked() { action=Some(value); } } });
                        ui.collapsing("Native runtimes",|ui| { ui.label("Select trusted Qdrant and llama-server executables. Registration records their SHA-256; startup verifies it.");ui.label("Runtime version label");ui.text_edit_singleline(&mut state.runtime_version);for kind in ["qdrant","llama-server"] { if ui.add_enabled(!state.runtime_version.trim().is_empty(),egui::Button::new(format!("Register {kind} executable…"))).clicked() { action=Some(SettingsAction::Register(kind.into(),state.runtime_version.clone())); } } });
                    }
                    SettingsTab::Agent=>{
                        if ui.button("Add endpoint").clicked() { state.draft.agent.endpoints.push(AgentEndpoint::default());state.endpoint=state.draft.agent.endpoints.len()-1; }
                        let mut remove=false;
                        if !state.draft.agent.endpoints.is_empty() {
                            state.endpoint=state.endpoint.min(state.draft.agent.endpoints.len()-1);
                            egui::ComboBox::from_id_salt("settings-endpoint").selected_text(&state.draft.agent.endpoints[state.endpoint].name).show_ui(ui,|ui| { for (index,endpoint) in state.draft.agent.endpoints.iter().enumerate() { ui.selectable_value(&mut state.endpoint,index,&endpoint.name); } });
                            let endpoint=&mut state.draft.agent.endpoints[state.endpoint];ui.label("Name");ui.text_edit_singleline(&mut endpoint.name);ui.label("Base URL");ui.text_edit_singleline(&mut endpoint.base_url);ui.label("Model");ui.text_edit_singleline(&mut endpoint.model);
                            ui.horizontal(|ui| { ui.radio_value(&mut endpoint.endpoint_type,EndpointType::ChatCompatible,"Chat compatible");ui.radio_value(&mut endpoint.endpoint_type,EndpointType::Ollama,"Ollama native"); });ui.label("Credential env variable name");ui.text_edit_singleline(&mut endpoint.credential_env);ui.label("Session credential (not saved)");ui.add(egui::TextEdit::singleline(&mut state.session_key).password(true));ui.checkbox(&mut endpoint.allow_local_unauthenticated,"Explicitly allow unauthenticated loopback endpoint");ui.add(egui::Slider::new(&mut endpoint.timeout_seconds,1..=600).text("Request timeout (seconds)"));
                            if ui.button("Test connection").clicked() { action=Some(SettingsAction::Test(endpoint.clone(),state.session_key.clone())); }remove=ui.button("Remove endpoint").clicked();
                        }
                        if remove { state.draft.agent.endpoints.remove(state.endpoint); }
                    }
                    SettingsTab::Permissions=>{
                        ui.checkbox(&mut state.draft.agent.enabled,"Enable Agent");path_list(ui,"Allowed roots",&mut state.draft.agent.allowed_roots);ui.checkbox(&mut state.draft.agent.metadata_read,"Read metadata");ui.checkbox(&mut state.draft.agent.content_read,"Read bounded UTF-8 file content");ui.checkbox(&mut state.draft.agent.semantic_search,"Semantic retrieval");ui.checkbox(&mut state.draft.agent.metadata_write,"Propose tag / description changes with approval");ui.checkbox(&mut state.draft.agent.filesystem_write,"Propose file operations with approval");ui.checkbox(&mut state.draft.agent.network,"Connect to configured Agent endpoints");ui.checkbox(&mut state.draft.agent.mcp_stdio,"Expose read tools through --mcp stdio");ui.weak("No HTTP or LAN MCP listener. File data and model output cannot grant permissions. Agent write tools require interactive approval. MCP exposes read tools only.");
                    }
                    SettingsTab::System=>{ path_list(ui,"OS mounted / UNC / WSL shares",&mut state.draft.sidebar.shares);ui.weak("Shares use OS credentials and filesystem access. File operations run through one queue, with per-item progress and cancellation. Direct peer transfer is outside this package."); }
                    SettingsTab::Storage=>{
                        for (label,path) in [("App",&paths.root),("Data",&paths.data),("Models",&paths.models),("Runtime",&paths.runtime),("Logs",&paths.logs),("App trash",&paths.trash)] { ui.label(format!("{label}: {}",path.display())); }
                        if ui.button("Clear size cache").clicked() { action=Some(SettingsAction::ClearCaches); }if ui.button("Create SQLite backup…").clicked() { action=Some(SettingsAction::Backup); }if ui.button("Restore SQLite backup…").clicked() { state.review_restore=true; }
                        if state.review_restore { ui.label("Restore replaces current metadata. Stop active operations first. A safety backup is created before restore.");ui.horizontal(|ui| { if ui.button("Choose backup to restore").clicked() { action=Some(SettingsAction::Restore);state.review_restore=false; }if ui.button("Cancel restore").clicked() { state.review_restore=false; } }); }
                        if ui.button("Inspect recovery journal").clicked() { action=Some(SettingsAction::Diagnostic); }
                        ui.separator();ui.strong("Relocate an unavailable root");ui.label("Old absolute root");ui.text_edit_singleline(&mut state.relocation_old);ui.label("New absolute root");ui.text_edit_singleline(&mut state.relocation_new);ui.weak("Review the old and new root before confirming. This maps indexed IDs to existing matching relative paths, preserves tags and descriptions, and does not move files. Agent permissions require a separate update.");
                        if ui.button("Review metadata relocation").clicked() { action=Some(SettingsAction::Relocate(state.relocation_old.clone(),state.relocation_new.clone())); }
                    }
                }
            });
            });
        });
        ui.separator();if let Some(error)=&state.error { ui.colored_label(ui.visuals().error_fg_color,error); }
        let validation = crate::shared::validation::validate_config(&state.draft);
        if let Err(error) = &validation { ui.colored_label(ui.visuals().error_fg_color, error.to_string()); }
        ui.horizontal(|ui| { if ui.add_enabled(!state.is_busy && validation.is_ok(),egui::Button::new("Apply")).clicked() { action=Some(SettingsAction::Apply); }if ui.button("Cancel").clicked() || ui.input(|input| input.key_pressed(egui::Key::Escape)) { action=Some(SettingsAction::Cancel); } });
    });
    action
}

fn path_list(ui: &mut egui::Ui, label: &str, paths: &mut Vec<String>) {
    ui.label(label);
    let mut remove = None;
    for (index, path) in paths.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.push_id((label, index), |ui| {
                ui.text_edit_singleline(path);
                if crate::shared::utils::parse_config_path(path).is_err() {
                    ui.colored_label(ui.visuals().error_fg_color, "Absolute path required");
                }
                if ui.button("Remove").clicked() {
                    remove = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove {
        paths.remove(index);
    }
    if ui.button(format!("Add {label}")).clicked() {
        paths.push(String::new());
    }
}

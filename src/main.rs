#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod infrastructure;
mod services;
mod shared;
mod ui;

use infrastructure::{
    clipboard::FileClipboard,
    config::{ConfigStorage, runtime_paths},
    model_store::HuggingFaceModelStore,
    name_index::FffNameIndex,
    runtime::{LlamaEncoderFactory, QdrantBackendFactory},
    sqlite::SqliteStorage,
    system::{FsPathValidator, SystemFileInspector, SystemPlatform},
};
use shared::{constants::*, models::*, traits::*};
use std::sync::Arc;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

fn main() {
    if let Err(error) = run() {
        if std::env::args_os().any(|argument| argument == "--mcp") {
            eprintln!("{error}");
            std::process::exit(1);
        }
        rfd::MessageDialog::new()
            .set_title(APP_NAME)
            .set_description(error)
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
}

fn run() -> Result<(), String> {
    let paths = runtime_paths().map_err(|error| error.to_string())?;
    let storage = Arc::new(ConfigStorage::new(paths.config.clone()));
    let (mut config, mut startup_notice) = storage
        .load_or_recover()
        .map_err(|error| error.to_string())?;
    let configuration: Arc<dyn ConfigRepository> = storage;
    if let Err(error) = shared::i18n::set_language(&config.general.language, &paths.root) {
        startup_notice.get_or_insert(error);
    }
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("filemofm")
        .filename_suffix("log")
        .max_log_files(config.general.log_retention_days)
        .build(&paths.logs)
        .map_err(|_| "Log directory is not writable")?;
    let (writer, _guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::registry()
        .with(EnvFilter::new("filemofm=info"))
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false),
        )
        .try_init()
        .map_err(|_| "Logging could not start")?;
    let repository: Arc<dyn MetadataRepository> =
        Arc::new(SqliteStorage::new(&paths).map_err(|error| error.to_string())?);
    if let Some(source) = config.general.pending_restore.take() {
        // Clear the request first: a broken backup must not block every later launch.
        configuration
            .save(&config)
            .map_err(|error| error.to_string())?;
        let result = (|| -> Result<(), String> {
            let source = shared::utils::parse_config_path(&source)?;
            if !source.is_absolute() || source == paths.database {
                return Err("Restore backup path is invalid".into());
            }
            repository
                .backup(&paths.data.join(format!(
                    "before-restore-{}.db",
                    chrono::Utc::now().timestamp()
                )))
                .map_err(|error| error.to_string())?;
            repository
                .restore(&source)
                .map_err(|error| error.to_string())
        })();
        startup_notice = Some(match result {
            Ok(()) => "Metadata restored from backup".into(),
            Err(error) => format!("Restore failed; current metadata was kept: {error}"),
        });
    }
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments
        .first()
        .is_some_and(|value| value == "--restore-backup")
    {
        if arguments.len() != 2 {
            return Err(
                "Use --restore-backup <absolute-backup-path> while the app is closed".into(),
            );
        }
        let source = std::path::PathBuf::from(&arguments[1]);
        if !source.is_absolute() || source == paths.database {
            return Err(
                "Choose an absolute backup path that differs from the active database".into(),
            );
        }
        let safety = paths.data.join(format!(
            "before-restore-{}.db",
            chrono::Utc::now().timestamp()
        ));
        repository
            .backup(&safety)
            .map_err(|error| error.to_string())?;
        repository
            .restore(&source)
            .map_err(|error| error.to_string())?;
        return Ok(());
    }
    if arguments.len() > 1 || arguments.first().is_some_and(|value| value != "--mcp") {
        return Err("Unknown command-line argument".into());
    }
    let models: Arc<dyn ModelRepository> = Arc::new(HuggingFaceModelStore::new(paths.clone()));
    let encoders: Arc<dyn EncoderFactory> = Arc::new(LlamaEncoderFactory::new(paths.clone()));
    let backends: Arc<dyn VectorBackendFactory> =
        Arc::new(QdrantBackendFactory::new(paths.clone()));
    let policy: Arc<dyn PermissionPolicy> = Arc::new(services::access::AgentAccessPolicy::new(
        config.agent.clone(),
    ));
    if arguments.first().is_some_and(|value| value == "--mcp") {
        let (tx, rx) = crossbeam_channel::bounded(CHANNEL_CAPACITY);
        let sender = EventSink::new(tx, Arc::new(|| {}));
        std::thread::spawn(move || while rx.recv().is_ok() {});
        let vectors = Arc::new(services::vector::VectorService::new(
            repository.clone(),
            models,
            encoders,
            backends,
            config,
            sender,
        ));
        let tools = Arc::new(services::tools::MetadataTools::new(
            repository,
            vectors,
            policy.clone(),
            None,
            None,
        ));
        return services::mcp::run_stdio(tools, policy, services::tools::tool_schemas(false));
    }
    let validator: Arc<dyn PathValidator> = Arc::new(FsPathValidator::new(
        config.scanner.excluded_paths.clone(),
        vec![
            paths.data.clone(),
            paths.models.clone(),
            paths.runtime.clone(),
            paths.logs.clone(),
            paths.trash.clone(),
        ],
    ));
    let platform: Arc<dyn PlatformIntegration> = Arc::new(SystemPlatform::new(paths.clone()));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_inner_size([
                config.general.last_window_width as f32,
                config.general.last_window_height as f32,
            ])
            .with_min_inner_size([MIN_WINDOW_WIDTH as f32, MIN_WINDOW_HEIGHT as f32])
            .with_maximized(config.general.maximized),
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| {
            let mut fonts = egui::FontDefinitions::default();
            egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
            // Arrows and box-drawing glyphs exist only in the bundled monospace font.
            if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                family.push("Hack".to_owned());
            }
            cc.egui_ctx.set_fonts(fonts);
            let (sender, receiver) = crossbeam_channel::bounded(CHANNEL_CAPACITY);
            let context = cc.egui_ctx.clone();
            let sender = EventSink::new(sender, Arc::new(move || context.request_repaint()));
            let clipboard = Arc::new(FileClipboard::new(sender.clone()));
            let dependencies = ui::app::AppDependencies {
                paths,
                repository,
                configuration,
                models,
                names: Arc::new(FffNameIndex::new(validator.clone())),
                inspector: Arc::new(SystemFileInspector),
                validator,
                platform,
                encoders,
                backends,
                clipboard,
                policy,
                sender,
                receiver,
                startup_notice,
            };
            Ok(Box::new(ui::app::FilemoApp::new(cc, config, dependencies)))
        }),
    )
    .map_err(|_| "FilemoFM window could not start; check the graphics/display session".into())
}

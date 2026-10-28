pub const APP_NAME: &str = "FilemoFM";
pub const CONFIG_NAME: &str = "config.toml";
pub const DATABASE_NAME: &str = "filemofm.db";
pub const CONFIG_VERSION: u32 = 1;
pub const MAX_PATH_BYTES: usize = 32768;
pub const MAX_QUERY_BYTES: usize = 4096;
pub const MAX_DESCRIPTION_BYTES: usize = 65536;
pub const MAX_TAG_NAME_LENGTH: usize = 50;
pub const MAX_RESULTS: usize = 1000;
pub const CHANNEL_CAPACITY: usize = 256;
pub const MESSAGE_FRAME_BUDGET: usize = 64;
pub const SCAN_BATCH_SIZE: usize = 128;
pub const DIRECTORY_CACHE_ENTRIES: usize = 8;
pub const CANVAS_REBUILD_INTERVAL_MS: u64 = 120;
pub const TOAST_SECONDS: u64 = 5;
pub const DIRECTORY_CACHE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_RELOCATION_FILES: usize = 10000;
pub const MAX_OPERATION_ITEMS: usize = 10000;
pub const MAX_FILE_TAGS: usize = 64;
pub const MAX_CONFIG_ROOTS: usize = 32;
pub const MAX_AGENT_ENDPOINTS: usize = 16;
pub const MAX_RUNTIME_LIBRARIES: usize = 64;
pub const MAX_REFERENCE_LABEL_BYTES: usize = 256;
pub const MIN_WINDOW_WIDTH: u32 = 760;
pub const MIN_WINDOW_HEIGHT: u32 = 480;
pub const MAX_WINDOW_SIZE: u32 = 16384;
pub const SIZE_ENTRY_BUDGET: usize = 2_000_000;
pub const LEGACY_SIZE_ENTRY_BUDGET: usize = 100_000;
pub const SIZE_TIME_BUDGET_SECS: u64 = 60;
pub const LEGACY_SIZE_TIME_BUDGET_SECS: u64 = 10;
pub const SIZE_WORKERS: usize = 4;
pub const SIZE_CACHE_TTL_SECS: i64 = 300;
pub const SEARCH_DEBOUNCE_MS: u64 = 300;
pub const WATCHER_DEBOUNCE_MS: u64 = 500;
pub const DEFAULT_DIMENSIONS: usize = 1024;
pub const MIN_QWEN_DIMENSIONS: usize = 32;
pub const DEFAULT_MODEL_REPO: &str = "https://huggingface.co/Qwen/Qwen3-Embedding-0.6B";
pub const DEFAULT_ARTIFACT_REPO: &str = "Qwen/Qwen3-Embedding-0.6B-GGUF";
pub const DEFAULT_ARTIFACT_FILE: &str = "Qwen3-Embedding-0.6B-f16.gguf";
pub const QUERY_INSTRUCTION: &str =
    "Instruct: Retrieve files relevant to the user's query\nQuery: ";
pub const RRF_OFFSET: f32 = 60.0;
pub const MAX_HTTP_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_MODEL_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_AGENT_TURNS: usize = 8;
pub const MAX_AGENT_OUTPUT_BYTES: usize = 65536;
pub const CLIPBOARD_INTENT_FORMAT: &str = "application/x-filemofm-intent";
pub const MAX_PREVIEW_IMAGE_BYTES: u64 = 50 * 1024 * 1024;
pub const MAX_PREVIEW_TEXT_BYTES: u64 = 1024 * 1024;
pub const PREVIEW_MAX_DIMENSION: u32 = 512;
pub const SIDEBAR_WIDTH: f32 = 220.0;
pub const PREVIEW_PANEL_WIDTH: f32 = 300.0;
pub const FILE_CANVAS_MIN_WIDTH: f32 = 320.0;
pub const SIDEBAR_MIN_WIDTH: f32 = 150.0;
pub const INSPECTOR_MIN_WIDTH: f32 = 240.0;
pub const OVERLAY_MARGIN: f32 = 32.0;
pub const STATUS_BAR_HEIGHT: f32 = 28.0;
pub const HISTORY_FREQUENT_LIMIT: usize = 10;
pub const PREVIEWABLE_IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp"];
pub const PREVIEWABLE_TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "json", "toml", "yaml", "yml", "xml", "csv", "log", "rs", "py", "js", "ts",
    "html", "css",
];
pub const DOCUMENT_EXTENSIONS: &[&str] = &[
    "pdf", "doc", "docx", "odt", "txt", "md", "rtf", "xls", "xlsx", "ods", "csv", "ppt", "pptx",
    "odp",
];

use egui_phosphor::regular::*;

pub fn icon_for_extension(ext: &str, is_directory: bool) -> &'static str {
    if is_directory {
        return FOLDER;
    }

    match ext {
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif" | "svg" | "ico" | "tiff" => IMAGE,
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" => FILE_VIDEO,
        "mp3" | "wav" | "flac" | "ogg" | "aac" | "wma" | "m4a" => MUSIC_NOTE,
        "rs" => FILE_RS,
        "py" => FILE_PY,
        "js" | "mjs" | "cjs" => FILE_JS,
        "ts" | "mts" | "cts" => FILE_TS,
        "jsx" => FILE_JSX,
        "tsx" => FILE_TSX,
        "html" | "htm" => FILE_HTML,
        "css" | "scss" | "sass" | "less" => FILE_CSS,
        "json" | "jsonc" => FILE_CODE,
        "md" | "mdx" => FILE_MD,
        "txt" | "log" | "cfg" | "ini" | "conf" => FILE_TEXT,
        "pdf" => FILE_PDF,
        "doc" | "docx" | "odt" | "rtf" => FILE_DOC,
        "xls" | "xlsx" | "ods" => FILE_XLS,
        "ppt" | "pptx" | "odp" => FILE_PPT,
        "csv" | "tsv" => FILE_CSV,
        "zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz" | "zst" => FILE_ZIP,
        "c" | "h" => FILE_C,
        "cpp" | "cc" | "cxx" | "hpp" => FILE_CPP,
        "cs" => FILE_C_SHARP,
        "sql" | "db" | "sqlite" => FILE_SQL,
        "xml" | "xsl" | "xslt" => FILE_CODE,
        "yaml" | "yml" | "toml" => FILE_CODE,
        "vue" => FILE_VUE,
        "exe" | "msi" | "bat" | "cmd" | "ps1" | "sh" => GEAR,
        "dll" | "so" | "dylib" | "lib" | "a" => FILE_LOCK,
        _ => FILE_TEXT,
    }
}

pub fn icon_color_for_extension(ext: &str, is_directory: bool) -> egui::Color32 {
    if is_directory {
        return egui::Color32::from_rgb(96, 165, 250);
    }

    match ext {
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif" | "svg" | "ico" | "tiff" => {
            egui::Color32::from_rgb(168, 85, 247)
        }
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" => {
            egui::Color32::from_rgb(244, 63, 94)
        }
        "mp3" | "wav" | "flac" | "ogg" | "aac" | "wma" | "m4a" => {
            egui::Color32::from_rgb(251, 146, 60)
        }
        "rs" => egui::Color32::from_rgb(222, 112, 60),
        "py" => egui::Color32::from_rgb(74, 183, 128),
        "js" | "mjs" | "cjs" | "jsx" => egui::Color32::from_rgb(250, 204, 21),
        "ts" | "mts" | "cts" | "tsx" => egui::Color32::from_rgb(59, 130, 246),
        "html" | "htm" => egui::Color32::from_rgb(239, 68, 68),
        "css" | "scss" | "sass" | "less" => egui::Color32::from_rgb(96, 165, 250),
        "json" | "jsonc" | "xml" | "yaml" | "yml" | "toml" => {
            egui::Color32::from_rgb(156, 163, 175)
        }
        "md" | "mdx" => egui::Color32::from_rgb(96, 165, 250),
        "txt" | "log" | "cfg" | "ini" | "conf" => egui::Color32::from_rgb(156, 163, 175),
        "pdf" => egui::Color32::from_rgb(239, 68, 68),
        "doc" | "docx" | "odt" | "rtf" => egui::Color32::from_rgb(59, 130, 246),
        "xls" | "xlsx" | "ods" | "csv" | "tsv" => egui::Color32::from_rgb(34, 197, 94),
        "ppt" | "pptx" | "odp" => egui::Color32::from_rgb(251, 146, 60),
        "zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz" | "zst" => {
            egui::Color32::from_rgb(234, 179, 8)
        }
        "exe" | "msi" | "bat" | "cmd" | "ps1" | "sh" => egui::Color32::from_rgb(156, 163, 175),
        _ => egui::Color32::from_rgb(107, 114, 128),
    }
}

pub fn sidebar_icon_for_label(label: &str) -> &'static str {
    match label {
        "Home" => HOUSE,
        "Documents" => FILE_TEXT,
        "Downloads" => DOWNLOAD,
        "Desktop" => DESKTOP,
        "Pictures" => IMAGE,
        "Music" => MUSIC_NOTE,
        "Videos" => VIDEO,
        _ => FOLDER,
    }
}

pub fn type_label_for_extension(ext: &str, is_directory: bool) -> String {
    use crate::shared::i18n::tr;
    if is_directory {
        return tr("File folder").into_owned();
    }
    let known = match ext {
        "" => return tr("File").into_owned(),
        "md" | "mdx" => crate::shared::i18n::n_("Markdown file"),
        "txt" => crate::shared::i18n::n_("Text document"),
        "log" => crate::shared::i18n::n_("Log file"),
        "pdf" => crate::shared::i18n::n_("PDF document"),
        "doc" | "docx" | "odt" | "rtf" => crate::shared::i18n::n_("Word document"),
        "xls" | "xlsx" | "ods" => crate::shared::i18n::n_("Excel workbook"),
        "ppt" | "pptx" | "odp" => crate::shared::i18n::n_("Presentation"),
        "csv" | "tsv" => crate::shared::i18n::n_("Spreadsheet data"),
        "json" | "jsonc" => crate::shared::i18n::n_("JSON file"),
        "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" => {
            crate::shared::i18n::n_("Configuration file")
        }
        "zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz" | "zst" => {
            crate::shared::i18n::n_("Archive")
        }
        "exe" | "msi" => crate::shared::i18n::n_("Application"),
        "sh" | "bat" | "cmd" | "ps1" => crate::shared::i18n::n_("Script"),
        "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif" | "svg" | "ico" | "tiff" => {
            return crate::tr!("{ext} image", ext = ext.to_uppercase());
        }
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" => crate::shared::i18n::n_("Video"),
        "mp3" | "wav" | "flac" | "ogg" | "aac" | "wma" | "m4a" => crate::shared::i18n::n_("Audio"),
        _ => return crate::tr!("{ext} file", ext = ext.to_uppercase()),
    };
    tr(known).into_owned()
}

use crate::shared::models::{FileConfig, GroupColumn, SortColumn, ViewMode};
use egui_phosphor::regular::*;

#[derive(Clone, Copy)]
pub enum FileCommand {
    Open,
    Cut,
    Copy,
    Paste,
    Rename,
    Trash,
    Delete,
    CopyPath,
    Pin,
    Properties,
    SelectAll,
    SelectNone,
    Invert,
    Settings,
    NewFolder,
    NewFile,
    Terminal,
}

/// Right-click on empty space in the file list.
pub fn background_menu(ui: &mut egui::Ui, can_paste: bool) -> Option<FileCommand> {
    use crate::shared::i18n::tr;
    let mut action = None;
    for (icon, label, command, enabled) in [
        (
            FOLDER_PLUS,
            crate::shared::i18n::n_("New folder · Ctrl+Shift+N"),
            FileCommand::NewFolder,
            true,
        ),
        (
            FILE_PLUS,
            crate::shared::i18n::n_("New file"),
            FileCommand::NewFile,
            true,
        ),
        (
            CLIPBOARD,
            crate::shared::i18n::n_("Paste · Ctrl+V"),
            FileCommand::Paste,
            can_paste,
        ),
        (
            CHECK_SQUARE,
            crate::shared::i18n::n_("Select all · Ctrl+A"),
            FileCommand::SelectAll,
            true,
        ),
        (
            TERMINAL_WINDOW,
            crate::shared::i18n::n_("Open terminal here"),
            FileCommand::Terminal,
            true,
        ),
        (
            PUSH_PIN,
            crate::shared::i18n::n_("Pin this folder"),
            FileCommand::Pin,
            true,
        ),
        (
            INFO,
            crate::shared::i18n::n_("Properties"),
            FileCommand::Properties,
            true,
        ),
    ] {
        if ui
            .add_enabled(enabled, egui::Button::new(format!("{icon} {}", tr(label))))
            .clicked()
        {
            action = Some(command);
            ui.close();
        }
    }
    action
}

pub fn selection_menu(ui: &mut egui::Ui, count: usize, can_paste: bool) -> Option<FileCommand> {
    let mut action = None;
    ui.horizontal(|ui| {
        for (icon, label, command, enabled) in [
            (SCISSORS, "Cut", FileCommand::Cut, count > 0),
            (COPY, "Copy", FileCommand::Copy, count > 0),
            (PENCIL, "Rename", FileCommand::Rename, count == 1),
            (TRASH, "Trash", FileCommand::Trash, count > 0),
        ] {
            if ui
                .add_enabled(enabled, egui::Button::new(icon))
                .on_hover_text(label)
                .clicked()
            {
                action = Some(command);
                ui.close();
            }
        }
    });
    ui.separator();
    for (label, command, enabled) in [
        ("Open", FileCommand::Open, count == 1),
        ("Paste", FileCommand::Paste, can_paste),
        ("Copy path", FileCommand::CopyPath, count > 0),
        ("Pin folder", FileCommand::Pin, count == 1),
        ("Properties", FileCommand::Properties, count > 0),
        ("Delete permanently…", FileCommand::Delete, count > 0),
    ] {
        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
            action = Some(command);
            ui.close();
        }
    }
    action
}

pub fn sort_menu(ui: &mut egui::Ui, config: &mut FileConfig) {
    for (field, label) in [
        (SortColumn::Name, "Name"),
        (SortColumn::Modified, "Date modified"),
        (SortColumn::Extension, "Type"),
    ] {
        ui.radio_value(&mut config.sort, field, label);
    }
    ui.menu_button("More", |ui| {
        for (field, label) in [
            (SortColumn::Size, "Size"),
            (SortColumn::Created, "Date created"),
            (SortColumn::Tags, "Tags"),
            (SortColumn::Relevance, "Relevance"),
        ] {
            ui.radio_value(&mut config.sort, field, label);
        }
    });
    ui.separator();
    ui.radio_value(&mut config.ascending, true, "Ascending");
    ui.radio_value(&mut config.ascending, false, "Descending");
    ui.checkbox(&mut config.folders_first, "Folders first");
    ui.separator();
    ui.menu_button("Group by", |ui| {
        for (field, label) in [
            (GroupColumn::None, "None"),
            (GroupColumn::Name, "Name"),
            (GroupColumn::Modified, "Date modified"),
            (GroupColumn::Extension, "Type"),
            (GroupColumn::Size, "Size"),
            (GroupColumn::Created, "Date created"),
            (GroupColumn::Tags, "Tags"),
        ] {
            ui.radio_value(&mut config.group, field, label);
        }
    });
}

pub fn view_menu(
    ui: &mut egui::Ui,
    config: &mut FileConfig,
    sidebar: &mut bool,
    inspector: &mut bool,
) {
    for (view, label) in [
        (ViewMode::ExtraLargeIcons, "Extra large icons"),
        (ViewMode::LargeIcons, "Large icons"),
        (ViewMode::MediumIcons, "Medium icons"),
        (ViewMode::SmallIcons, "Small icons"),
        (ViewMode::List, "List"),
        (ViewMode::Details, "Details"),
        (ViewMode::Tiles, "Tiles"),
        (ViewMode::Content, "Content"),
    ] {
        ui.radio_value(&mut config.view, view, label);
    }
    ui.separator();
    ui.checkbox(sidebar, "Navigation pane");
    ui.checkbox(inspector, "Inspector");
    ui.checkbox(&mut config.compact, "Compact view");
    ui.checkbox(&mut config.checkboxes, "Item check boxes");
    ui.checkbox(&mut config.show_extensions, "File name extensions");
    ui.checkbox(&mut config.show_hidden, "Hidden items");
}

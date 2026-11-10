use crate::shared::i18n::{n_, tr};
use crate::shared::models::*;
use crate::shared::utils::{format_size, group_label};
use crate::ui::context_menu::{FileCommand, background_menu, selection_menu};
use crate::ui::icons::{icon_color_for_extension, icon_for_extension, type_label_for_extension};
use crate::ui::theme::FilemoTheme;
use egui_phosphor::regular::{CARET_DOWN, CARET_RIGHT, CARET_UP};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub enum TableAction {
    Select {
        id: String,
        is_toggle: bool,
        is_range: bool,
    },
    Open(String),
    Command {
        id: String,
        command: FileCommand,
    },
    Sort(SortColumn),
    Group(String),
    /// A command from the empty-area menu or a click on empty space.
    Background(FileCommand),
}

pub struct TableContext<'a> {
    pub can_paste: bool,
    pub is_global: bool,
    pub clipboard: Option<&'a ClipboardPayload>,
    /// Canvas row to bring into view (keyboard navigation).
    pub scroll_to: Option<usize>,
}
#[derive(Clone)]
pub enum CanvasRow {
    Group(String, usize),
    File(usize),
}
pub struct CanvasState {
    pub rows: Vec<CanvasRow>,
    pub collapsed: HashSet<String>,
    pub layout: Vec<Vec<CanvasRow>>,
    pub columns: usize,
    pub visible_folders: Vec<PathBuf>,
}
impl CanvasState {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            collapsed: HashSet::new(),
            layout: Vec::new(),
            columns: 0,
            visible_folders: Vec::new(),
        }
    }
    pub fn rebuild(
        &mut self,
        files: &[FileItem],
        sizes: &HashMap<PathBuf, FolderSize>,
        group: GroupColumn,
    ) {
        self.rows.clear();
        let mut last = String::new();
        for (index, file) in files.iter().enumerate() {
            let name = group_label(file, sizes, group);
            if group != GroupColumn::None && (index == 0 || name != last) {
                let count = files[index..]
                    .iter()
                    .take_while(|file| group_label(file, sizes, group) == name)
                    .count();
                self.rows.push(CanvasRow::Group(name.clone(), count));
                last = name.clone();
            }
            if group == GroupColumn::None || !self.collapsed.contains(&name) {
                self.rows.push(CanvasRow::File(index));
            }
        }
        self.columns = 0;
    }
}

pub fn render_file_table(
    ui: &mut egui::Ui,
    files: &[FileItem],
    selected: &HashSet<String>,
    config: &FileConfig,
    sizes: &HashMap<PathBuf, FolderSize>,
    state: &mut CanvasState,
    context: TableContext<'_>,
) -> Option<TableAction> {
    let TableContext {
        can_paste,
        is_global,
        clipboard,
        scroll_to,
    } = context;
    state.visible_folders.clear();
    // Registered first so rows drawn later win the hit test; only empty space reaches it.
    let background = ui.interact(
        ui.available_rect_before_wrap(),
        ui.id().with("file-canvas-background"),
        egui::Sense::click(),
    );
    let action = render_rows(
        ui, files, selected, config, sizes, state, can_paste, is_global, clipboard, scroll_to,
    );
    if action.is_some() {
        return action;
    }
    let mut action = None;
    if background.clicked() {
        action = Some(TableAction::Background(FileCommand::SelectNone));
    }
    background.context_menu(|ui| {
        if let Some(command) = background_menu(ui, can_paste) {
            action = Some(TableAction::Background(command));
        }
    });
    action
}

#[allow(clippy::too_many_arguments)]
fn render_rows(
    ui: &mut egui::Ui,
    files: &[FileItem],
    selected: &HashSet<String>,
    config: &FileConfig,
    sizes: &HashMap<PathBuf, FolderSize>,
    state: &mut CanvasState,
    can_paste: bool,
    is_global: bool,
    clipboard: Option<&ClipboardPayload>,
    scroll_to: Option<usize>,
) -> Option<TableAction> {
    let line_height = ui.text_style_height(&egui::TextStyle::Body);
    let detail_height = if is_global {
        (line_height * 2.0 + 22.0).max(52.0)
    } else if config.compact {
        (line_height + 12.0).max(27.0)
    } else {
        (line_height + 16.0).max(35.0)
    };
    let mut action = None;
    let list = matches!(
        config.view,
        ViewMode::Details | ViewMode::List | ViewMode::Content
    );
    if list {
        if config.view == ViewMode::Details {
            let current_sort = config.sort;
            let ascending = config.ascending;
            // Like Explorer, less important columns drop out as the list gets narrower.
            let width = ui.available_width();
            let columns: Vec<DetailColumn> = DetailColumn::ALL
                .into_iter()
                .filter(|column| width >= column.min_list_width())
                .collect();
            let mut table = egui_extras::TableBuilder::new(ui);
            if let Some(row) = scroll_to {
                table = table.scroll_to_row(row, None);
            }
            table = table
                .resizable(true)
                .striped(false)
                .auto_shrink([false, false])
                .sense(egui::Sense::click())
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
            for column in &columns {
                table = table.column(column.spec());
            }
            table
                .header(30.0, |mut header| {
                    for column in &columns {
                        header.col(|ui| {
                            let sort = column.sort();
                            let label = tr(column.label());
                            let text = if current_sort == sort {
                                format!("{label} {}", if ascending { CARET_UP } else { CARET_DOWN })
                            } else {
                                label.into_owned()
                            };
                            let rich = egui::RichText::new(text).color(if current_sort == sort {
                                ui.visuals().text_color()
                            } else {
                                ui.visuals().weak_text_color()
                            });
                            if ui
                                .add(egui::Label::new(rich).sense(egui::Sense::click()))
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                action = Some(TableAction::Sort(sort));
                            }
                        });
                    }
                })
                .body(|body| {
                    body.rows(detail_height, state.rows.len(), |mut row| {
                        match &state.rows[row.index()] {
                            CanvasRow::Group(name, count) => {
                                row.col(|ui| {
                                    ui.strong(format!(
                                        "{} {name} ({count})",
                                        if state.collapsed.contains(name) {
                                            CARET_RIGHT
                                        } else {
                                            CARET_DOWN
                                        }
                                    ));
                                });
                                for _ in 1..columns.len() {
                                    row.col(|_| {});
                                }
                                if row.response().clicked() {
                                    action = Some(TableAction::Group(name.clone()));
                                }
                            }
                            CanvasRow::File(index) => {
                                let file = &files[*index];
                                if file.is_directory {
                                    state.visible_folders.push(file.path.clone());
                                }
                                row.set_selected(selected.contains(&file.id));
                                for column in &columns {
                                    row.col(|ui| {
                                        detail_cell(
                                            ui, *column, file, selected, config, sizes, clipboard,
                                            is_global,
                                        )
                                    });
                                }
                                let response = row
                                    .response()
                                    .on_hover_text(file.path.display().to_string());
                                if let Some(value) = interact(&response, file, selected, can_paste)
                                {
                                    action = Some(value);
                                }
                            }
                        }
                    })
                });
        } else {
            let height = if config.view == ViewMode::Content {
                detail_height.max(72.0)
            } else {
                detail_height
            };
            egui::ScrollArea::vertical().show_rows(ui, height, state.rows.len(), |ui, range| {
                for index in range {
                    match &state.rows[index] {
                        CanvasRow::Group(name, count) => {
                            if ui.button(format!("{name} ({count})")).clicked() {
                                action = Some(TableAction::Group(name.clone()));
                            }
                        }
                        CanvasRow::File(index) => {
                            let file = &files[*index];
                            if file.is_directory {
                                state.visible_folders.push(file.path.clone());
                            }
                            if let Some(value) = file_button(
                                ui, file, selected, config, can_paste, is_global, clipboard,
                            ) {
                                action = Some(value);
                            }
                            if config.view == ViewMode::Content {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(format!(
                                            "{} · {}",
                                            size_label(file, sizes),
                                            file.description.chars().take(100).collect::<String>()
                                        ))
                                        .weak(),
                                    )
                                    .truncate(),
                                );
                            }
                        }
                    }
                }
            });
        }
        return action;
    }
    let (width, height, icon_size): (f32, f32, f32) = match config.view {
        ViewMode::SmallIcons => (110.0, 70.0, 24.0),
        ViewMode::MediumIcons => (140.0, 100.0, 40.0),
        ViewMode::LargeIcons => (170.0, 140.0, 64.0),
        ViewMode::ExtraLargeIcons => (210.0, 180.0, 88.0),
        _ => (260.0, 80.0, 32.0),
    };
    let height = height.max(
        icon_size
            + if is_global {
                line_height * 2.0 + 28.0
            } else {
                line_height + 20.0
            },
    );
    let columns = (ui.available_width() / width).floor().max(1.0) as usize;
    if state.columns != columns {
        state.layout.clear();
        let mut cells = Vec::new();
        for item in &state.rows {
            if matches!(item, CanvasRow::Group(_, _)) {
                if !cells.is_empty() {
                    state.layout.push(std::mem::take(&mut cells));
                }
                state.layout.push(vec![item.clone()]);
            } else {
                cells.push(item.clone());
                if cells.len() == columns {
                    state.layout.push(std::mem::take(&mut cells));
                }
            }
        }
        if !cells.is_empty() {
            state.layout.push(cells);
        }
        state.columns = columns;
    }
    egui::ScrollArea::vertical().show_rows(ui, height, state.layout.len(), |ui, range| {
        for index in range {
            ui.horizontal(|ui| {
                for item in &state.layout[index] {
                    match item {
                        CanvasRow::Group(name, count) => {
                            if ui.button(format!("{name} ({count})")).clicked() {
                                action = Some(TableAction::Group(name.clone()));
                            }
                        }
                        CanvasRow::File(index) => {
                            let file = &files[*index];
                            if file.is_directory {
                                state.visible_folders.push(file.path.clone());
                            }
                            ui.allocate_ui(egui::vec2(width - 8.0, height - 8.0), |ui| {
                                ui.vertical_centered(|ui| {
                                    ui.label(
                                        egui::RichText::new(icon_for_extension(
                                            &file.extension,
                                            file.is_directory,
                                        ))
                                        .size(icon_size)
                                        .color(
                                            icon_color_for_extension(
                                                &file.extension,
                                                file.is_directory,
                                            ),
                                        ),
                                    );
                                    if let Some(value) = file_button(
                                        ui, file, selected, config, can_paste, is_global, clipboard,
                                    ) {
                                        action = Some(value);
                                    }
                                    if config.view == ViewMode::Tiles {
                                        ui.weak(size_label(file, sizes));
                                    }
                                });
                            });
                        }
                    }
                }
            });
        }
    });
    action
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DetailColumn {
    Name,
    Modified,
    Type,
    Size,
    Tags,
}
impl DetailColumn {
    const ALL: [Self; 5] = [
        Self::Name,
        Self::Modified,
        Self::Type,
        Self::Size,
        Self::Tags,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::Name => n_("Name"),
            Self::Modified => n_("Date modified"),
            Self::Type => n_("Type"),
            Self::Size => n_("Size"),
            Self::Tags => n_("Tags"),
        }
    }
    fn sort(self) -> SortColumn {
        match self {
            Self::Name => SortColumn::Name,
            Self::Modified => SortColumn::Modified,
            Self::Type => SortColumn::Extension,
            Self::Size => SortColumn::Size,
            Self::Tags => SortColumn::Tags,
        }
    }
    /// Narrowest file list that still shows this column.
    fn min_list_width(self) -> f32 {
        match self {
            Self::Name | Self::Size => 0.0,
            // Tags are this app's core metadata, so they outlast the date and type columns.
            Self::Tags => 420.0,
            Self::Modified => 560.0,
            Self::Type => 720.0,
        }
    }
    fn spec(self) -> egui_extras::Column {
        match self {
            Self::Name => egui_extras::Column::remainder().at_least(140.0),
            Self::Modified => egui_extras::Column::initial(132.0).at_least(80.0),
            Self::Type => egui_extras::Column::initial(112.0).at_least(60.0),
            Self::Size => egui_extras::Column::initial(88.0).at_least(56.0),
            Self::Tags => egui_extras::Column::initial(120.0).at_least(56.0),
        }
        .clip(true)
    }
}

#[allow(clippy::too_many_arguments)]
fn detail_cell(
    ui: &mut egui::Ui,
    column: DetailColumn,
    file: &FileItem,
    selected: &HashSet<String>,
    config: &FileConfig,
    sizes: &HashMap<PathBuf, FolderSize>,
    clipboard: Option<&ClipboardPayload>,
    is_global: bool,
) {
    match column {
        DetailColumn::Name => {
            ui.label(
                egui::RichText::new(icon_for_extension(&file.extension, file.is_directory))
                    .size(16.0)
                    .color(icon_color_for_extension(&file.extension, file.is_directory)),
            );
            let name = egui::Label::new(name_text(ui, file, selected, config, clipboard))
                .truncate()
                .selectable(false);
            if !is_global {
                ui.add(name);
                return;
            }
            ui.vertical(|ui| {
                ui.add(name);
                ui.add(
                    egui::Label::new(egui::RichText::new(parent_label(file)).small().weak())
                        .truncate()
                        .selectable(false),
                );
            });
        }
        DetailColumn::Modified => {
            ui.add(
                egui::Label::new(egui::RichText::new(timestamp_label(file.modified_at)).weak())
                    .selectable(false),
            );
        }
        DetailColumn::Type => {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(type_label_for_extension(
                        &file.extension,
                        file.is_directory,
                    ))
                    .weak(),
                )
                .truncate()
                .selectable(false),
            );
        }
        DetailColumn::Size => {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let exact = !file.is_directory
                    || sizes
                        .get(&file.path)
                        .is_some_and(|size| size.state == SizeState::Complete);
                let text = egui::RichText::new(size_label(file, sizes));
                let response = ui.add(
                    egui::Label::new(if exact { text } else { text.weak() }).selectable(false),
                );
                if !exact {
                    response.on_hover_text(tr(
                        "Folder sizes are measured in the background. ≥ means some items could not be counted; ~ means the cached size may be outdated.",
                    ));
                }
            });
        }
        DetailColumn::Tags => {
            for tag in file.tags.iter().take(3) {
                tag_chip(ui, tag);
            }
            if file.tags.len() > 3 {
                ui.weak(format!("+{}", file.tags.len() - 3));
            }
        }
    }
}

fn file_button(
    ui: &mut egui::Ui,
    file: &FileItem,
    selected: &HashSet<String>,
    config: &FileConfig,
    can_paste: bool,
    is_global: bool,
    clipboard: Option<&ClipboardPayload>,
) -> Option<TableAction> {
    let text = name_text(ui, file, selected, config, clipboard);
    let text = egui::RichText::new(format!(
        "{} {}",
        icon_for_extension(&file.extension, file.is_directory),
        text.text()
    ))
    .color(if is_cut(file, clipboard) {
        ui.visuals().weak_text_color()
    } else {
        ui.visuals().text_color()
    });
    let response = ui
        .add(
            egui::Button::new(text)
                .frame(false)
                .selected(selected.contains(&file.id))
                .truncate()
                .min_size(egui::vec2(ui.available_width(), 24.0)),
        )
        .on_hover_text(file.path.display().to_string());
    let action = interact(&response, file, selected, can_paste);
    if is_global {
        ui.add(egui::Label::new(egui::RichText::new(parent_label(file)).weak()).truncate());
    }
    action
}

fn is_cut(file: &FileItem, clipboard: Option<&ClipboardPayload>) -> bool {
    clipboard.is_some_and(|payload| payload.is_cut && payload.paths.contains(&file.path))
}

fn parent_label(file: &FileItem) -> String {
    file.path
        .parent()
        .unwrap_or(&file.path)
        .display()
        .to_string()
}

fn name_text(
    ui: &egui::Ui,
    file: &FileItem,
    selected: &HashSet<String>,
    config: &FileConfig,
    clipboard: Option<&ClipboardPayload>,
) -> egui::RichText {
    let name = if config.show_extensions || file.is_directory {
        file.name.clone()
    } else {
        file.path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    };
    let checkbox = match (config.checkboxes, selected.contains(&file.id)) {
        (false, _) => "",
        (true, true) => "☑ ",
        (true, false) => "☐ ",
    };
    egui::RichText::new(format!("{checkbox}{name}")).color(if is_cut(file, clipboard) {
        ui.visuals().weak_text_color()
    } else {
        ui.visuals().strong_text_color()
    })
}

/// Fixed-size coloured chip; painted directly so tall rows do not stretch it.
fn tag_chip(ui: &mut egui::Ui, tag: &Tag) {
    let color = FilemoTheme::tag_color(tag.color_h, tag.color_s, tag.color_l);
    let font = egui::TextStyle::Small.resolve(ui.style());
    let galley = ui
        .ctx()
        .fonts_mut(|fonts| fonts.layout_no_wrap(tag.name.clone(), font, color));
    let size = galley.size() + egui::vec2(12.0, 4.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    painter.rect(
        rect,
        4.0,
        color.gamma_multiply(0.18),
        egui::Stroke::new(1.0_f32, color.gamma_multiply(0.5)),
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.min + egui::vec2(6.0, 2.0), galley, color);
    response.on_hover_text(&tag.name);
}

/// Shared click / double-click / context-menu handling for a file's row or cell.
fn interact(
    response: &egui::Response,
    file: &FileItem,
    selected: &HashSet<String>,
    can_paste: bool,
) -> Option<TableAction> {
    let mut action = None;
    if response.secondary_clicked() && !selected.contains(&file.id) {
        action = Some(TableAction::Select {
            id: file.id.clone(),
            is_toggle: false,
            is_range: false,
        });
    }
    response.context_menu(|ui| {
        if let Some(command) = selection_menu(
            ui,
            if selected.contains(&file.id) {
                selected.len()
            } else {
                1
            },
            can_paste,
        ) {
            action = Some(TableAction::Command {
                id: file.id.clone(),
                command,
            });
        }
    });
    if response.double_clicked() {
        action = Some(TableAction::Open(file.id.clone()));
    } else if response.clicked() {
        let modifiers = response.ctx.input(|input| input.modifiers);
        action = Some(TableAction::Select {
            id: file.id.clone(),
            is_toggle: modifiers.command,
            is_range: modifiers.shift,
        });
    }
    action
}

/// Folder sizes are measured in the background: `…` while unknown, `≥` when only part of the
/// tree could be counted (a lower bound), `~` when the cached value may be outdated.
pub fn size_label(file: &FileItem, sizes: &HashMap<PathBuf, FolderSize>) -> String {
    if !file.is_directory {
        return format_size(file.size);
    }
    match sizes.get(&file.path) {
        None => "…".into(),
        Some(size) => format!(
            "{}{}",
            match size.state {
                SizeState::Complete => "",
                SizeState::Partial => "≥ ",
                SizeState::Stale => "~",
            },
            format_size(size.bytes)
        ),
    }
}
fn timestamp_label(value: i64) -> String {
    if value == 0 {
        return "Unknown".into();
    }
    chrono::DateTime::from_timestamp(value, 0)
        .map(|date| {
            date.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "Unknown".into())
}

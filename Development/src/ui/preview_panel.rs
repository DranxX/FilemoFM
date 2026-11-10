use crate::shared::i18n::tr;
use crate::shared::models::*;
use crate::shared::utils::reference_nodes;
use crate::ui::table_view::size_label;
use egui::{RichText, TextureHandle};
use egui_phosphor::regular::{ARROW_SQUARE_OUT, CIRCLE, COPY, CURSOR_CLICK, PLUS, TAG, X};
use pulldown_cmark::{Event, Parser, Tag as MarkdownTag, TagEnd};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditorMode {
    Write,
    Preview,
    Split,
}
pub struct ReferenceBinding {
    pub marker: String,
    pub file: FileItem,
    pub label: Option<String>,
}
pub struct DescriptionEditor {
    pub file: FileItem,
    pub draft: String,
    pub saved: String,
    pub bindings: Vec<ReferenceBinding>,
    pub mode: EditorMode,
    pub selection: std::ops::Range<usize>,
    pub picker: bool,
    pub picker_query: String,
    pub picker_index: usize,
    pub link_url: String,
    pub show_link: bool,
    restore_cursor: bool,
    mention_range: Option<std::ops::Range<usize>>,
    relink_index: Option<usize>,
    edit_label: Option<(usize, String)>,
}
impl DescriptionEditor {
    pub fn new(file: FileItem, targets: Vec<FileItem>) -> Self {
        let mut draft = file.description.clone();
        let mut bindings = Vec::new();
        for (range, id, label) in reference_nodes(&file.description).into_iter().rev() {
            if let Some(target) = targets.iter().find(|target| target.id == id) {
                let marker = reference_marker(target, label.as_deref(), &bindings, None);
                draft.replace_range(range, &marker);
                if !bindings.iter().any(|binding| binding.marker == marker) {
                    bindings.push(ReferenceBinding {
                        marker,
                        file: target.clone(),
                        label: label.clone(),
                    });
                }
            }
        }
        Self {
            file,
            draft: draft.clone(),
            saved: draft,
            bindings,
            mode: EditorMode::Write,
            selection: 0..0,
            picker: false,
            picker_query: String::new(),
            picker_index: 0,
            link_url: String::new(),
            show_link: false,
            restore_cursor: false,
            mention_range: None,
            relink_index: None,
            edit_label: None,
        }
    }
    pub fn is_dirty(&self) -> bool {
        self.draft != self.saved
    }
    pub fn source(&self) -> String {
        let replacements = self
            .bindings
            .iter()
            .map(|binding| {
                (
                    binding.marker.clone(),
                    if let Some(label) = &binding.label {
                        format!("@{{file:{}|{label}}}", binding.file.id)
                    } else {
                        format!("@{{file:{}}}", binding.file.id)
                    },
                )
            })
            .collect::<Vec<_>>();
        crate::shared::utils::replace_marked_text(&self.draft, &replacements)
    }
    fn replace_binding(&mut self, index: usize, file: FileItem, label: Option<String>) {
        let Some(binding) = self.bindings.get(index) else {
            return;
        };
        let marker = reference_marker(&file, label.as_deref(), &self.bindings, Some(index));
        self.draft = crate::shared::utils::replace_marked_text(
            &self.draft,
            &[(binding.marker.clone(), marker.clone())],
        );
        if self
            .bindings
            .iter()
            .enumerate()
            .any(|(other, binding)| other != index && binding.marker == marker)
        {
            self.bindings.remove(index);
        } else {
            self.bindings[index] = ReferenceBinding {
                marker,
                file,
                label,
            };
        }
    }
    fn remove_binding(&mut self, index: usize, keep_text: bool) {
        let Some(binding) = self.bindings.get(index) else {
            return;
        };
        let replacement = if keep_text {
            binding
                .label
                .clone()
                .unwrap_or_else(|| binding.file.path.display().to_string())
        } else {
            String::new()
        };
        self.draft = crate::shared::utils::replace_marked_text(
            &self.draft,
            &[(binding.marker.clone(), replacement)],
        );
        self.bindings.remove(index);
    }
    fn wrap(&mut self, prefix: &str, suffix: &str) {
        let chars = self.draft.chars().collect::<Vec<_>>();
        let start = self.selection.start.min(chars.len());
        let end = self.selection.end.min(chars.len());
        self.draft = format!(
            "{}{}{}{}{}",
            chars[..start].iter().collect::<String>(),
            prefix,
            chars[start..end].iter().collect::<String>(),
            suffix,
            chars[end..].iter().collect::<String>()
        );
        self.selection = start + prefix.chars().count()..end + prefix.chars().count();
        self.restore_cursor = true;
    }
    pub fn insert_reference(&mut self, file: FileItem) {
        if let Some(index) = self.relink_index.take() {
            if let Some(binding) = self.bindings.get(index) {
                self.replace_binding(index, file, binding.label.clone());
            }
            self.picker = false;
            return;
        }
        let marker = reference_marker(&file, None, &self.bindings, None);
        let chars = self.draft.chars().collect::<Vec<_>>();
        let range = self
            .mention_range
            .take()
            .unwrap_or_else(|| self.selection.clone());
        let mut start = range.start.min(chars.len());
        let end = range.end.min(chars.len());
        if start > 0 && chars[start - 1] == '@' {
            start -= 1;
        }
        self.draft = format!(
            "{}{}{}",
            chars[..start].iter().collect::<String>(),
            marker,
            chars[end..].iter().collect::<String>()
        );
        let cursor = start + marker.chars().count();
        self.selection = cursor..cursor;
        self.restore_cursor = true;
        if !self.bindings.iter().any(|binding| binding.marker == marker) {
            self.bindings.push(ReferenceBinding {
                marker,
                file,
                label: None,
            });
        }
        self.picker = false;
    }
}

fn reference_marker(
    file: &FileItem,
    label: Option<&str>,
    bindings: &[ReferenceBinding],
    ignored: Option<usize>,
) -> String {
    let value = label
        .map(str::to_string)
        .unwrap_or_else(|| crate::shared::utils::config_path(&file.path));
    let escaped = value
        .chars()
        .flat_map(|ch| {
            if matches!(ch, '\\' | '"' | '*' | '_' | '`' | '[' | ']' | '<' | '>') {
                vec!['\\', ch]
            } else {
                vec![ch]
            }
        })
        .collect::<String>();
    let mut number = 1;
    loop {
        let marker = if number == 1 {
            format!("@\"{escaped}\"")
        } else {
            format!("@\"{escaped} [{number}]\"")
        };
        let collision = bindings.iter().enumerate().any(|(index, binding)| {
            Some(index) != ignored
                && binding.marker == marker
                && (binding.file.id != file.id || binding.label.as_deref() != label)
        });
        if !collision {
            return marker;
        }
        number += 1;
    }
}

pub enum PreviewAction {
    Open,
    Save,
    Discard,
    AddTag(i64),
    RemoveTag(i64),
    OpenReference(String),
    ReferenceSearch,
    CopyPath,
}

pub fn render_preview_panel(
    ui: &mut egui::Ui,
    file: Option<&FileItem>,
    selected_count: usize,
    tab: InspectorTab,
    preview: Option<&FilePreview>,
    texture: Option<&TextureHandle>,
    editor: &mut Option<DescriptionEditor>,
    tags: &[Tag],
    sizes: &HashMap<std::path::PathBuf, FolderSize>,
    reference_candidates: &[FileItem],
) -> Option<PreviewAction> {
    let mut action = None;
    if selected_count > 1 {
        empty_state(
            ui,
            COPY,
            &crate::tr!("{count} items selected", count = selected_count),
        );
        ui.vertical_centered(|ui| {
            ui.weak(tr(
                "Select a single item to see its details and description",
            ));
        });
        return None;
    }
    let Some(file) = file else {
        empty_state(ui, CURSOR_CLICK, &tr("Select a file to inspect"));
        return None;
    };
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let icon = crate::ui::icons::icon_for_extension(&file.extension, file.is_directory);
        let color = crate::ui::icons::icon_color_for_extension(&file.extension, file.is_directory);
        egui::Frame::new()
            .fill(color.gamma_multiply(0.15))
            .corner_radius(10)
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.label(RichText::new(icon).size(28.0).color(color));
            });
        ui.vertical(|ui| {
            ui.add(egui::Label::new(RichText::new(&file.name).size(16.0).strong()).wrap());
            ui.weak(crate::ui::icons::type_label_for_extension(
                &file.extension,
                file.is_directory,
            ));
        });
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui
            .button(format!("{ARROW_SQUARE_OUT} {}", tr("Open")))
            .clicked()
        {
            action = Some(PreviewAction::Open);
        }
        if ui
            .button(format!("{COPY} {}", tr("Copy path")))
            .on_hover_text(file.path.display().to_string())
            .clicked()
        {
            action = Some(PreviewAction::CopyPath);
        }
    });
    ui.add_space(6.0);
    if tab == InspectorTab::Preview {
        egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .corner_radius(8)
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                match preview {
                    None if file.is_directory => {
                        ui.weak(crate::tr!(
                            "Folder · {size}",
                            size = size_label(file, sizes)
                        ));
                    }
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.weak(tr("Loading preview"));
                        });
                    }
                    Some(FilePreview::Image { width, height, .. }) => {
                        if let Some(texture) = texture {
                            let factor = (ui.available_width() / (*width as f32))
                                .min(500.0 / (*height as f32))
                                .min(1.0);
                            ui.vertical_centered(|ui| {
                                ui.add(egui::Image::new(texture).fit_to_exact_size(
                                    egui::vec2(*width as f32, *height as f32) * factor,
                                ));
                            });
                            ui.weak(format!("{width} × {height}"));
                        }
                    }
                    Some(FilePreview::Text(text)) => {
                        egui::ScrollArea::both()
                            .max_height((ui.available_height() - 8.0).max(200.0))
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(RichText::new(text).monospace())
                                        .selectable(true),
                                );
                            });
                    }
                    Some(FilePreview::None) => {
                        ui.weak(tr("No preview for this file type"));
                    }
                    Some(FilePreview::TooLarge) => {
                        ui.weak(tr("File is too large to preview"));
                    }
                    Some(FilePreview::UnsupportedEncoding) => {
                        ui.weak(tr("Text encoding is not supported"));
                    }
                    Some(FilePreview::DecodeFailed(error)) => {
                        ui.colored_label(ui.visuals().error_fg_color, tr(error));
                    }
                }
            });
        return action;
    }
    caption(ui, "File information");
    let timestamp = |value: i64| {
        if value == 0 {
            tr("Unknown").into_owned()
        } else {
            chrono::DateTime::from_timestamp(value, 0)
                .map(|date| {
                    date.with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M")
                        .to_string()
                })
                .unwrap_or_else(|| tr("Unknown").into_owned())
        }
    };
    egui::Grid::new(("file-information", &file.id))
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            for (label, value) in [
                (
                    "Type",
                    crate::ui::icons::type_label_for_extension(&file.extension, file.is_directory),
                ),
                ("Size", size_label(file, sizes)),
                ("Modified", timestamp(file.modified_at)),
                ("Created", timestamp(file.created_at)),
            ] {
                ui.weak(tr(label));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(value);
                });
                ui.end_row();
            }
        });
    caption(ui, "Location");
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(file.path.display().to_string())
                            .monospace()
                            .small(),
                    )
                    .wrap(),
                );
            });
        });
    caption(ui, "Tags");
    ui.horizontal_wrapped(|ui| {
        for tag in &file.tags {
            let color =
                crate::ui::theme::FilemoTheme::tag_color(tag.color_h, tag.color_s, tag.color_l);
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(format!("{} {X}", tag.name))
                            .small()
                            .color(color),
                    )
                    .fill(color.gamma_multiply(0.15))
                    .stroke(egui::Stroke::new(1.0_f32, color.gamma_multiply(0.5))),
                )
                .on_hover_text(tr("Remove tag"))
                .clicked()
            {
                action = Some(PreviewAction::RemoveTag(tag.id));
            }
        }
        ui.menu_button(
            RichText::new(format!("{PLUS} {}", tr("Add tag"))).small(),
            |ui| {
                let mut any = false;
                for tag in tags {
                    if !file.tags.iter().any(|existing| existing.id == tag.id) {
                        any = true;
                        let color = crate::ui::theme::FilemoTheme::tag_color(
                            tag.color_h,
                            tag.color_s,
                            tag.color_l,
                        );
                        if ui
                            .button(RichText::new(format!("{TAG} {}", tag.name)).color(color))
                            .clicked()
                        {
                            action = Some(PreviewAction::AddTag(tag.id));
                            ui.close();
                        }
                    }
                }
                if !any {
                    ui.weak(tr("Create tags with + in the sidebar"));
                }
            },
        );
    });
    caption(ui, "Description");
    let Some(editor) = editor.as_mut().filter(|editor| editor.file.id == file.id) else {
        ui.spinner();
        ui.weak("Loading description");
        return action;
    };
    if editor.is_dirty() {
        ui.colored_label(
            crate::ui::theme::FilemoTheme::WARNING,
            format!("{CIRCLE} {}", tr("Unsaved changes · Ctrl+S")),
        );
    }
    ui.horizontal(|ui| {
        ui.selectable_value(&mut editor.mode, EditorMode::Write, "Write");
        ui.selectable_value(&mut editor.mode, EditorMode::Preview, "Preview");
        if ui.available_width() > 100.0 {
            ui.selectable_value(&mut editor.mode, EditorMode::Split, "Split");
        }
    });
    if editor.mode != EditorMode::Preview {
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(RichText::new("B").strong())
                .on_hover_text("Bold · Ctrl+B")
                .clicked()
            {
                editor.wrap("**", "**");
            }
            if ui
                .button(RichText::new("I").italics())
                .on_hover_text("Italic · Ctrl+I")
                .clicked()
            {
                editor.wrap("*", "*");
            }
            if ui.button("Code").clicked() {
                editor.wrap("`", "`");
            }
            if ui.button("Link").clicked() {
                editor.show_link = !editor.show_link;
            }
            if ui.button("@ File").clicked() {
                editor.picker = !editor.picker;
            }
            ui.menu_button("Format", |ui| {
                for (label, prefix, suffix) in [
                    ("Heading", "\n## ", "\n"),
                    ("List", "\n- ", "\n"),
                    ("Quote", "\n> ", "\n"),
                    ("Code block", "\n```\n", "\n```\n"),
                ] {
                    if ui.button(label).clicked() {
                        editor.wrap(prefix, suffix);
                        ui.close();
                    }
                }
            });
        });
        if editor.show_link {
            ui.text_edit_singleline(&mut editor.link_url);
            if ui.button("Insert link").clicked()
                && reqwest::Url::parse(&editor.link_url)
                    .is_ok_and(|url| matches!(url.scheme(), "https" | "http"))
            {
                let suffix = format!("]({})", editor.link_url);
                editor.wrap("[", &suffix);
                editor.show_link = false;
            }
        }
        let editor_id = egui::Id::new(("description-input", &editor.file.id));
        if editor.restore_cursor {
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), editor_id) {
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(editor.selection.start),
                        egui::text::CCursor::new(editor.selection.end),
                    )));
                state.store(ui.ctx(), editor_id);
            }
            ui.memory_mut(|memory| memory.request_focus(editor_id));
            editor.restore_cursor = false;
        }
        let output = egui::TextEdit::multiline(&mut editor.draft)
            .id(editor_id)
            .font(egui::TextStyle::Monospace)
            .desired_width(f32::INFINITY)
            .desired_rows(10)
            .char_limit(crate::shared::constants::MAX_DESCRIPTION_BYTES)
            .show(ui);
        if let Some(range) = output.cursor_range {
            editor.selection = range.as_sorted_char_range();
        }
        if output.response.has_focus() {
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::B)) {
                editor.wrap("**", "**");
            }
            if ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::I)) {
                editor.wrap("*", "*");
            }
            if output.response.changed() {
                let prefix = editor
                    .draft
                    .chars()
                    .take(editor.selection.end)
                    .collect::<String>();
                if let Some(start) = prefix.rfind('@') {
                    let before = &prefix[..start];
                    let tail = &prefix[start + 1..];
                    let mut code = false;
                    let mut is_text = false;
                    for (event, range) in Parser::new(&editor.draft).into_offset_iter() {
                        match event {
                            Event::Start(MarkdownTag::CodeBlock(_)) => code = true,
                            Event::End(TagEnd::CodeBlock) => code = false,
                            Event::Text(_) if !code && range.contains(&start) => is_text = true,
                            _ => {}
                        }
                    }
                    if is_text
                        && before.chars().last().is_none_or(char::is_whitespace)
                        && !tail.contains('\n')
                    {
                        editor.mention_range = Some(before.chars().count()..prefix.chars().count());
                        editor.picker = true;
                        editor.picker_query = tail.trim_matches('"').to_string();
                        editor.picker_index = 0;
                        action = Some(PreviewAction::ReferenceSearch);
                    }
                }
            }
        }
        if !editor.selection.is_empty() && output.response.has_focus() {
            egui::Area::new(egui::Id::new("selection-format-toolbar"))
                .order(egui::Order::Foreground)
                .fixed_pos(output.response.rect.left_bottom())
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if ui.button("Bold").clicked() {
                                editor.wrap("**", "**");
                            }
                            if ui.button("Italic").clicked() {
                                editor.wrap("*", "*");
                            }
                            if ui.button("Code").clicked() {
                                editor.wrap("`", "`");
                            }
                        });
                    });
                });
        }
    }
    if editor.picker {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label("Reference an indexed file");
            if ui.text_edit_singleline(&mut editor.picker_query).changed()
                || ui.button("Search indexed files").clicked()
            {
                action = Some(PreviewAction::ReferenceSearch);
            }
            if ui.input(|input| input.key_pressed(egui::Key::ArrowDown)) {
                editor.picker_index =
                    (editor.picker_index + 1).min(reference_candidates.len().saturating_sub(1));
            }
            if ui.input(|input| input.key_pressed(egui::Key::ArrowUp)) {
                editor.picker_index = editor.picker_index.saturating_sub(1);
            }
            let mut chosen = None;
            egui::ScrollArea::vertical()
                .id_salt("reference-candidates")
                .max_height(150.0)
                .show(ui, |ui| {
                    for (index, target) in reference_candidates.iter().take(20).enumerate() {
                        if ui
                            .selectable_label(
                                index == editor.picker_index,
                                format!(
                                    "{} · {}",
                                    target.name,
                                    target.path.parent().unwrap_or(&target.path).display()
                                ),
                            )
                            .clicked()
                        {
                            chosen = Some(target.clone());
                        }
                    }
                });
            if ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                chosen = reference_candidates.get(editor.picker_index).cloned();
            }
            if let Some(file) = chosen {
                editor.insert_reference(file);
            }
            if reference_candidates.is_empty() {
                ui.weak("No indexed matches. Search or select a file in the list.");
            }
            if ui.button("Close picker").clicked()
                || ui.input(|input| input.key_pressed(egui::Key::Escape))
            {
                editor.picker = false;
                editor.mention_range = None;
                editor.relink_index = None;
            }
        });
    }
    if editor.mode != EditorMode::Write {
        egui::Frame::group(ui.style()).show(ui, |ui| render_markdown(ui, &editor.draft));
    }
    let mut reference_action = None;
    for (index, binding) in editor.bindings.iter().enumerate() {
        let title = format!(
            "@ {}{}",
            binding.label.as_deref().unwrap_or(&binding.file.name),
            if binding.file.is_missing {
                " · unavailable"
            } else {
                ""
            }
        );
        ui.horizontal_wrapped(|ui| {
            if ui
                .link(title)
                .on_hover_text(binding.file.path.display().to_string())
                .clicked()
            {
                action = Some(PreviewAction::OpenReference(binding.file.id.clone()));
            }
            ui.push_id(index, |ui| {
                ui.menu_button("Reference", |ui| {
                    if ui.button("Copy path").clicked() {
                        ui.ctx()
                            .copy_text(crate::shared::utils::config_path(&binding.file.path));
                        ui.close();
                    }
                    for (label, operation) in [
                        ("Edit label", 0),
                        ("Use current file name", 1),
                        ("Relink to another file", 2),
                        ("Keep as plain text", 3),
                        ("Remove reference", 4),
                    ] {
                        if ui.button(label).clicked() {
                            reference_action = Some((index, operation));
                            ui.close();
                        }
                    }
                });
            });
        });
    }
    if let Some((index, operation)) = reference_action {
        match operation {
            0 => {
                editor.edit_label = editor.bindings.get(index).map(|binding| {
                    (
                        index,
                        binding
                            .label
                            .clone()
                            .unwrap_or_else(|| binding.file.name.clone()),
                    )
                })
            }
            1 => {
                if let Some(binding) = editor.bindings.get(index) {
                    editor.replace_binding(index, binding.file.clone(), None);
                }
            }
            2 => {
                editor.relink_index = Some(index);
                editor.picker = true;
                editor.picker_query.clear();
                action = Some(PreviewAction::ReferenceSearch);
            }
            3 => editor.remove_binding(index, true),
            4 => editor.remove_binding(index, false),
            _ => {}
        }
    }
    let mut label_choice = None;
    if let Some((index, label)) = &mut editor.edit_label {
        ui.horizontal(|ui| {
            ui.label("Reference label");
            ui.text_edit_singleline(label);
            let valid = !label.trim().is_empty()
                && label.len() <= crate::shared::constants::MAX_REFERENCE_LABEL_BYTES
                && !label.chars().any(char::is_control)
                && !label.contains(['|', '}']);
            if ui
                .add_enabled(valid, egui::Button::new("Apply label"))
                .clicked()
            {
                label_choice = Some(Some((*index, label.clone())));
            }
            if ui.button("Cancel label").clicked() {
                label_choice = Some(None);
            }
        });
    }
    if let Some(choice) = label_choice {
        editor.edit_label = None;
        if let Some((index, label)) = choice
            && let Some(binding) = editor.bindings.get(index)
        {
            editor.replace_binding(index, binding.file.clone(), Some(label));
        }
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(editor.is_dirty(), egui::Button::new("Save description"))
            .clicked()
        {
            action = Some(PreviewAction::Save);
        }
        if ui
            .add_enabled(editor.is_dirty(), egui::Button::new("Discard"))
            .clicked()
        {
            action = Some(PreviewAction::Discard);
        }
    });
    action
}

pub fn render_markdown(ui: &mut egui::Ui, source: &str) {
    let mut strong = false;
    let mut emphasis = false;
    let mut code = false;
    let mut heading = false;
    let mut link = None::<String>;
    let mut blocks = Vec::<Vec<(RichText, Option<String>)>>::new();
    let mut spans = Vec::new();
    for event in Parser::new(source) {
        match event {
            Event::Start(MarkdownTag::Strong) => strong = true,
            Event::End(TagEnd::Strong) => strong = false,
            Event::Start(MarkdownTag::Emphasis) => emphasis = true,
            Event::End(TagEnd::Emphasis) => emphasis = false,
            Event::Start(MarkdownTag::CodeBlock(_)) => code = true,
            Event::Start(MarkdownTag::Heading { .. }) => heading = true,
            Event::Start(MarkdownTag::Link { dest_url, .. }) => link = Some(dest_url.into_string()),
            Event::End(TagEnd::Link) => link = None,
            Event::Start(MarkdownTag::Item) => spans.push((RichText::new("• "), None)),
            event @ (Event::Text(_) | Event::Code(_)) => {
                let inline_code = matches!(event, Event::Code(_));
                let text = match event {
                    Event::Text(text) | Event::Code(text) => text,
                    _ => unreachable!(),
                };
                let mut text = RichText::new(text.as_ref());
                if strong || heading {
                    text = text.strong();
                }
                if emphasis {
                    text = text.italics();
                }
                if code || inline_code {
                    text = text.monospace();
                }
                if heading {
                    text = text.size(ui.text_style_height(&egui::TextStyle::Heading));
                }
                spans.push((text, link.clone()));
            }
            Event::End(
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Item,
            )
            | Event::HardBreak => {
                if !spans.is_empty() {
                    blocks.push(std::mem::take(&mut spans));
                }
                code = false;
                heading = false;
            }
            Event::SoftBreak => spans.push((RichText::new(" "), None)),
            Event::Rule => {
                if !spans.is_empty() {
                    blocks.push(std::mem::take(&mut spans));
                }
                blocks.push(Vec::new());
            }
            _ => {}
        }
    }
    if !spans.is_empty() {
        blocks.push(spans);
    }
    for block in blocks {
        if block.is_empty() {
            ui.separator();
            continue;
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (text, link) in block {
                if let Some(url) = link.filter(|url| {
                    reqwest::Url::parse(url)
                        .is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
                }) {
                    ui.hyperlink_to(text, url);
                } else {
                    ui.label(text);
                }
            }
        });
    }
}

fn caption(ui: &mut egui::Ui, text: &str) {
    ui.add_space(10.0);
    ui.label(
        RichText::new(tr(text).to_uppercase())
            .small()
            .strong()
            .color(ui.visuals().weak_text_color()),
    );
    ui.add_space(2.0);
}

fn empty_state(ui: &mut egui::Ui, icon: &str, text: &str) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(icon)
                .size(32.0)
                .color(ui.visuals().weak_text_color()),
        );
        ui.add_space(6.0);
        ui.label(RichText::new(text).color(ui.visuals().weak_text_color()));
    });
}

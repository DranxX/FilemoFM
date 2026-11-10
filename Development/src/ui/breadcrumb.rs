use crate::shared::i18n::tr;
use egui_phosphor::regular::*;
use std::path::{Path, PathBuf};

pub struct NavigationState {
    pub address: String,
    pub is_editing: bool,
    pub focus_requested: bool,
    pub can_back: bool,
    pub can_forward: bool,
}
pub enum BreadcrumbAction {
    NavigateTo(PathBuf),
    Submit(String),
    Back,
    Forward,
    Up,
    Refresh,
    PickFolder,
}

/// Breadcrumb segments as `(label, path)`. The deepest known root (drive, mount, WSL distro)
/// becomes one labelled segment, so `C:\` reads "Local Disk (C:)" and never shows `\\?\`.
fn segments(path: &Path, roots: &[(String, PathBuf)]) -> Vec<(String, PathBuf)> {
    let root = roots
        .iter()
        .filter(|(_, root)| path.starts_with(root))
        .max_by_key(|(_, root)| root.components().count());
    let mut segments = Vec::new();
    let (mut accumulated, relative) = match root {
        Some((label, root)) => {
            segments.push((label.clone(), root.clone()));
            (
                root.clone(),
                path.strip_prefix(root).unwrap_or(Path::new("")),
            )
        }
        None => (PathBuf::new(), path),
    };
    let mut prefix = String::new();
    for component in relative.components() {
        accumulated.push(component.as_os_str());
        match component {
            std::path::Component::Prefix(value) => {
                prefix = value.as_os_str().to_string_lossy().into_owned();
            }
            std::path::Component::RootDir => {
                let label = if prefix.is_empty() {
                    std::path::MAIN_SEPARATOR_STR.to_string()
                } else {
                    format!("{prefix}{}", std::path::MAIN_SEPARATOR)
                };
                segments.push((label, accumulated.clone()));
                prefix.clear();
            }
            _ => segments.push((
                component.as_os_str().to_string_lossy().into_owned(),
                accumulated.clone(),
            )),
        }
    }
    if !prefix.is_empty() {
        segments.push((prefix, accumulated));
    }
    segments
}

pub fn render_breadcrumb(
    ui: &mut egui::Ui,
    path: &Path,
    roots: &[(String, PathBuf)],
    state: &mut NavigationState,
    reserve_right: f32,
) -> Option<BreadcrumbAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        let nav = |ui: &mut egui::Ui, enabled: bool, icon: &str, hint: String| {
            ui.add_enabled(
                enabled,
                egui::Button::new(egui::RichText::new(icon).size(16.0)).frame(false),
            )
            .on_hover_text(hint)
            .clicked()
        };
        if nav(ui, state.can_back, ARROW_LEFT, format!("{} · Alt+←", tr("Back"))) {
            action = Some(BreadcrumbAction::Back);
        }
        if nav(
            ui,
            state.can_forward,
            ARROW_RIGHT,
            format!("{} · Alt+→", tr("Forward")),
        ) {
            action = Some(BreadcrumbAction::Forward);
        }
        if nav(
            ui,
            path.parent().is_some(),
            ARROW_UP,
            format!("{} · Alt+↑ / Backspace", tr("Up")),
        ) {
            action = Some(BreadcrumbAction::Up);
        }
        if nav(ui, true, ARROWS_CLOCKWISE, format!("{} · F5", tr("Refresh"))) {
            action = Some(BreadcrumbAction::Refresh);
        }
        ui.add_space(4.0);
        let available = (ui.available_width() - reserve_right).max(120.0);
        let frame = egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .corner_radius(6)
            .inner_margin(egui::Margin::symmetric(6, 3));
        if state.is_editing {
            frame.show(ui, |ui| {
                ui.set_width(available - 12.0);
                ui.horizontal(|ui| {
                    ui.weak(FOLDER_OPEN);
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut state.address)
                            .id(egui::Id::new("address-input"))
                            .frame(egui::Frame::NONE)
                            .desired_width(ui.available_width())
                            .font(egui::TextStyle::Monospace)
                            .hint_text(tr(
                                "Type a path: C:\\Users or C:/Users, \\\\server\\share, ~/Documents",
                            )),
                    );
                    if state.focus_requested {
                        response.request_focus();
                        if let Some(mut text) = egui::TextEdit::load_state(ui.ctx(), response.id) {
                            text.cursor
                                .set_char_range(Some(egui::text::CCursorRange::two(
                                    egui::text::CCursor::new(0),
                                    egui::text::CCursor::new(state.address.chars().count()),
                                )));
                            text.store(ui.ctx(), response.id);
                        }
                        state.focus_requested = false;
                    }
                    let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
                    if ui.input(|input| input.key_pressed(egui::Key::Escape))
                        || (response.lost_focus() && !enter)
                    {
                        state.is_editing = false;
                        state.address = crate::shared::utils::config_path(path);
                    } else if response.lost_focus() {
                        action = Some(BreadcrumbAction::Submit(state.address.clone()));
                    }
                });
            });
            return;
        }
        frame.show(ui, |ui| {
            ui.set_width(available - 12.0);
            ui.horizontal(|ui| {
                ui.menu_button(egui::RichText::new(HARD_DRIVES).weak(), |ui| {
                    for (label, root) in roots {
                        let is_current = path.starts_with(root);
                        if ui
                            .selectable_label(is_current, format!("{HARD_DRIVE} {label}"))
                            .on_hover_text(root.display().to_string())
                            .clicked()
                        {
                            action = Some(BreadcrumbAction::NavigateTo(root.clone()));
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui
                        .button(format!("{FOLDER_OPEN} {}", tr("Open folder…")))
                        .clicked()
                    {
                        action = Some(BreadcrumbAction::PickFolder);
                        ui.close();
                    }
                })
                .response
                .on_hover_text(tr("Drives and locations"));
                egui::ScrollArea::horizontal()
                    .id_salt("breadcrumb-scroll")
                    .stick_to_right(true)
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .max_width((ui.available_width() - 24.0).max(40.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            let parts = segments(path, roots);
                            let last = parts.len().saturating_sub(1);
                            for (index, (label, target)) in parts.into_iter().enumerate() {
                                if index > 0 {
                                    ui.label(egui::RichText::new(CARET_RIGHT).small().weak());
                                }
                                let text = if index == last {
                                    egui::RichText::new(label).strong()
                                } else {
                                    egui::RichText::new(label)
                                };
                                if ui
                                    .add(egui::Button::new(text).frame(false))
                                    .on_hover_text(target.display().to_string())
                                    .clicked()
                                {
                                    action = Some(BreadcrumbAction::NavigateTo(target));
                                }
                            }
                        });
                    });
                // The empty part of the bar starts editing, like clicking Explorer's address bar.
                let filler = ui.allocate_response(
                    egui::vec2(
                        (ui.available_width() - 24.0).max(0.0),
                        ui.spacing().interact_size.y,
                    ),
                    egui::Sense::click(),
                );
                let pencil = ui
                    .add(egui::Button::new(egui::RichText::new(PENCIL_SIMPLE).weak()).frame(false))
                    .on_hover_text(format!("{} · Ctrl+L", tr("Edit address")));
                if filler.clicked() || pencil.clicked() {
                    state.is_editing = true;
                    state.focus_requested = true;
                    state.address = crate::shared::utils::config_path(path);
                }
                filler.on_hover_cursor(egui::CursorIcon::Text);
            });
        });
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labelled_root_becomes_one_segment() {
        let roots = vec![("File System".to_string(), PathBuf::from("/"))];
        let parts = segments(Path::new("/home/example"), &roots);
        assert_eq!(
            parts.iter().map(|part| part.0.as_str()).collect::<Vec<_>>(),
            ["File System", "home", "example"]
        );
        assert_eq!(parts[2].1, PathBuf::from("/home/example"));
    }
}

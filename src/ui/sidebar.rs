use crate::shared::i18n::{n_, tr};
use crate::shared::models::Tag;
use crate::ui::icons::sidebar_icon_for_label;
use crate::ui::theme::FilemoTheme;
use egui::{Color32, Response, Sense, Ui};
use egui_phosphor::regular::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct SidebarState {
    pub locations: Vec<(String, PathBuf)>,
    pub drives: Vec<(String, PathBuf)>,
    pub children: HashMap<PathBuf, Result<Vec<PathBuf>, String>>,
}
pub enum SidebarAction {
    Navigate(PathBuf),
    Expand(PathBuf),
    Tag(String),
    CreateTag,
    DeleteTag(i64),
    Unpin(PathBuf),
    PinCurrent,
    System,
    Settings,
}

const ROW_HEIGHT: f32 = 28.0;
const TREE_DEPTH_LIMIT: usize = 12;

pub struct SidebarContext<'a> {
    pub current: &'a Path,
    pub favorites: &'a [String],
    pub frequent: &'a [PathBuf],
    pub shares: &'a [String],
    pub tags: &'a [Tag],
    pub is_system: bool,
}

pub fn render_sidebar(
    ui: &mut Ui,
    state: &SidebarState,
    context: SidebarContext<'_>,
    closed: &mut std::collections::BTreeSet<String>,
) -> Option<SidebarAction> {
    let mut action = None;
    let current = context.current;
    let footer_height = ROW_HEIGHT + 12.0;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .max_height((ui.available_height() - footer_height).max(ROW_HEIGHT))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            section(
                ui,
                n_("Quick access"),
                None,
                closed,
                &mut action,
                |ui, action| {
                    for (label, path) in &state.locations {
                        if row(
                            ui,
                            sidebar_icon_for_label(label),
                            None,
                            &tr(label),
                            current == path,
                            0.0,
                        )
                        .on_hover_text(path.display().to_string())
                        .clicked()
                        {
                            *action = Some(SidebarAction::Navigate(path.clone()));
                        }
                    }
                },
            );
            if !context.frequent.is_empty() {
                section(ui, n_("Recent"), None, closed, &mut action, |ui, action| {
                    for path in context.frequent.iter().take(6) {
                        if row(
                            ui,
                            CLOCK_COUNTER_CLOCKWISE,
                            None,
                            &display_name(path),
                            current == path,
                            0.0,
                        )
                        .on_hover_text(path.display().to_string())
                        .clicked()
                        {
                            *action = Some(SidebarAction::Navigate(path.clone()));
                        }
                    }
                });
            }
            section(
                ui,
                n_("Favorites"),
                Some((
                    PLUS,
                    n_("Pin the current folder"),
                    SidebarAction::PinCurrent,
                )),
                closed,
                &mut action,
                |ui, action| {
                    for value in context.favorites {
                        let Ok(path) = crate::shared::utils::parse_config_path(value) else {
                            continue;
                        };
                        let response =
                            row(ui, STAR, None, &display_name(&path), current == path, 0.0)
                                .on_hover_text(path.display().to_string());
                        if response.clicked() {
                            *action = Some(SidebarAction::Navigate(path.clone()));
                        }
                        response.context_menu(|ui| {
                            if ui
                                .button(format!("{PUSH_PIN_SLASH} {}", tr("Unpin")))
                                .clicked()
                            {
                                *action = Some(SidebarAction::Unpin(path.clone()));
                                ui.close();
                            }
                        });
                    }
                    if context.favorites.is_empty() {
                        hint(ui, &tr("Use + to pin the current folder"));
                    }
                },
            );
            section(
                ui,
                n_("Tags"),
                Some((PLUS, n_("Create tag"), SidebarAction::CreateTag)),
                closed,
                &mut action,
                |ui, action| {
                    for tag in context.tags {
                        let color = FilemoTheme::tag_color(tag.color_h, tag.color_s, tag.color_l);
                        let response = row(ui, TAG, Some(color), &tag.name, false, 0.0);
                        if response.clicked() {
                            *action = Some(SidebarAction::Tag(tag.name.clone()));
                        }
                        response.context_menu(|ui| {
                            if ui
                                .button(format!("{TRASH} {}", tr("Delete tag…")))
                                .clicked()
                            {
                                *action = Some(SidebarAction::DeleteTag(tag.id));
                                ui.close();
                            }
                        });
                    }
                    if context.tags.is_empty() {
                        hint(ui, &tr("Use + to create a tag"));
                    }
                },
            );
            section(
                ui,
                n_("This PC"),
                None,
                closed,
                &mut action,
                |ui, action| {
                    for (label, drive) in &state.drives {
                        tree(ui, state, current, drive, label, 0, action);
                    }
                },
            );
            if !context.shares.is_empty() {
                section(
                    ui,
                    n_("Network"),
                    None,
                    closed,
                    &mut action,
                    |ui, action| {
                        for share in context.shares {
                            let Ok(path) = crate::shared::utils::parse_config_path(share) else {
                                continue;
                            };
                            if row(ui, NETWORK, None, share, current == path, 0.0).clicked() {
                                *action = Some(SidebarAction::Navigate(path));
                            }
                        }
                    },
                );
            }
        });
    ui.separator();
    ui.horizontal(|ui| {
        if row_with_width(
            ui,
            HARD_DRIVES,
            None,
            &tr("System & transfers"),
            context.is_system,
            0.0,
            (ui.available_width() - 36.0).max(60.0),
        )
        .clicked()
        {
            action = Some(SidebarAction::System);
        }
        if ui
            .add(egui::Button::new(GEAR).frame(false))
            .on_hover_text(tr("Settings"))
            .clicked()
        {
            action = Some(SidebarAction::Settings);
        }
    });
    action
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn hint(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(30.0);
        ui.label(egui::RichText::new(text).small().weak());
    });
    ui.add_space(2.0);
}

/// Collapsible section with an uppercase caption and an optional header button.
fn section(
    ui: &mut Ui,
    label: &str,
    button: Option<(&str, &str, SidebarAction)>,
    closed: &mut std::collections::BTreeSet<String>,
    action: &mut Option<SidebarAction>,
    body: impl FnOnce(&mut Ui, &mut Option<SidebarAction>),
) {
    let is_open = !closed.contains(label);
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let caption = egui::RichText::new(tr(label).to_uppercase())
            .small()
            .strong()
            .color(ui.visuals().weak_text_color());
        let response = ui
            .add(
                egui::Label::new(caption)
                    .sense(Sense::click())
                    .selectable(false),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let caret = if is_open { CARET_DOWN } else { CARET_RIGHT };
            let toggled = ui
                .add(egui::Button::new(egui::RichText::new(caret).small().weak()).frame(false))
                .clicked();
            if let Some((icon, hint, value)) = button
                && ui
                    .add(egui::Button::new(egui::RichText::new(icon).small()).frame(false))
                    .on_hover_text(tr(hint))
                    .clicked()
            {
                *action = Some(value);
            }
            if toggled || response.clicked() {
                if is_open {
                    closed.insert(label.to_string());
                } else {
                    closed.remove(label);
                }
            }
        });
    });
    if is_open {
        body(ui, action);
    }
}

fn row(
    ui: &mut Ui,
    icon: &str,
    icon_color: Option<Color32>,
    label: &str,
    selected: bool,
    indent: f32,
) -> Response {
    let width = ui.available_width();
    row_with_width(ui, icon, icon_color, label, selected, indent, width)
}

/// Full-width navigation row: icon, truncated label, hover fill and an accent bar when active.
fn row_with_width(
    ui: &mut Ui,
    icon: &str,
    icon_color: Option<Color32>,
    label: &str,
    selected: bool,
    indent: f32,
    width: f32,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let painter = ui.painter();
        if selected {
            painter.rect_filled(rect, 6.0, visuals.selection.bg_fill);
            painter.rect_filled(
                egui::Rect::from_min_size(
                    rect.min + egui::vec2(0.0, 6.0),
                    egui::vec2(3.0, rect.height() - 12.0),
                ),
                2.0,
                visuals.selection.stroke.color,
            );
        } else if response.hovered() {
            painter.rect_filled(rect, 6.0, visuals.widgets.hovered.weak_bg_fill);
        }
        let text_color = if selected {
            visuals.strong_text_color()
        } else {
            visuals.text_color()
        };
        let icon_color = icon_color.unwrap_or(if selected {
            visuals.selection.stroke.color
        } else {
            visuals.weak_text_color()
        });
        let font = egui::TextStyle::Body.resolve(ui.style());
        let left = rect.left() + 10.0 + indent;
        painter.text(
            egui::pos2(left + 8.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            icon,
            font.clone(),
            icon_color,
        );
        let text_left = left + 24.0;
        let mut job = egui::text::LayoutJob::simple_singleline(label.to_owned(), font, text_color);
        job.wrap =
            egui::text::TextWrapping::truncate_at_width((rect.right() - text_left - 6.0).max(8.0));
        let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
        painter.galley(
            egui::pos2(text_left, rect.center().y - galley.size().y / 2.0),
            galley,
            text_color,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Lazy folder tree: a caret expands (and loads children on first open), the label navigates.
fn tree(
    ui: &mut Ui,
    state: &SidebarState,
    current: &Path,
    path: &Path,
    label: &str,
    depth: usize,
    action: &mut Option<SidebarAction>,
) {
    let id = ui.make_persistent_id(("sidebar-tree", path));
    let mut is_open = ui.data(|data| data.get_temp::<bool>(id).unwrap_or(false));
    let indent = depth as f32 * 14.0;
    let icon = if depth == 0 { HARD_DRIVE } else { FOLDER };
    let caret_width = 18.0;
    let response = row(
        ui,
        icon,
        None,
        label,
        current == path,
        indent + caret_width - 6.0,
    )
    .on_hover_text(path.display().to_string());
    let caret_rect = egui::Rect::from_min_size(
        response.rect.min + egui::vec2(4.0 + indent, 0.0),
        egui::vec2(caret_width, response.rect.height()),
    );
    let is_ancestor = current.starts_with(path) && current != path;
    ui.painter().text(
        caret_rect.center(),
        egui::Align2::CENTER_CENTER,
        if is_open { CARET_DOWN } else { CARET_RIGHT },
        egui::TextStyle::Small.resolve(ui.style()),
        if is_ancestor {
            ui.visuals().selection.stroke.color
        } else {
            ui.visuals().weak_text_color()
        },
    );
    if response.clicked() {
        let on_caret = response
            .interact_pointer_pos()
            .is_some_and(|position| position.x <= caret_rect.right());
        if on_caret {
            is_open = !is_open;
            ui.data_mut(|data| data.insert_temp(id, is_open));
            if is_open && !state.children.contains_key(path) {
                *action = Some(SidebarAction::Expand(path.to_path_buf()));
            }
        } else {
            *action = Some(SidebarAction::Navigate(path.to_path_buf()));
        }
    }
    if !is_open || depth >= TREE_DEPTH_LIMIT {
        return;
    }
    match state.children.get(path) {
        None => {
            ui.horizontal(|ui| {
                ui.add_space(indent + 40.0);
                ui.spinner();
            });
        }
        Some(Ok(children)) => {
            for child in children {
                tree(
                    ui,
                    state,
                    current,
                    child,
                    &display_name(child),
                    depth + 1,
                    action,
                );
            }
            if children.is_empty() {
                hint(ui, &tr("No subfolders"));
            }
        }
        Some(Err(error)) => {
            ui.horizontal(|ui| {
                ui.add_space(indent + 30.0);
                ui.label(egui::RichText::new(tr(error)).small().weak());
                if ui.small_button(tr("Retry")).clicked() {
                    *action = Some(SidebarAction::Expand(path.to_path_buf()));
                }
            });
        }
    }
}

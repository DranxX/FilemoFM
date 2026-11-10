use crate::shared::i18n::tr;
use crate::shared::models::{SearchMode, SearchScope};
use egui_phosphor::regular::*;

pub struct SearchBarState {
    pub query: String,
    pub mode: SearchMode,
    pub scope: SearchScope,
    pub focus_requested: bool,
    pub error: Option<String>,
    pub is_searching: bool,
}
pub enum SearchAction {
    Execute,
    Changed,
    Clear,
}
pub fn render_search_bar(
    ui: &mut egui::Ui,
    state: &mut SearchBarState,
    location: &str,
) -> Option<SearchAction> {
    let mut action = None;
    let old = (state.mode, state.scope);
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.weak(MAGNIFYING_GLASS);
                let trailing = if state.query.is_empty() { 28.0 } else { 56.0 }
                    + if state.is_searching { 22.0 } else { 0.0 };
                let response = ui
                    .add(
                        egui::TextEdit::singleline(&mut state.query)
                            .id(egui::Id::new("search-input"))
                            .frame(egui::Frame::NONE)
                            .desired_width((ui.available_width() - trailing).max(60.0))
                            .hint_text(match state.scope {
                                SearchScope::CurrentFolder => {
                                    crate::tr!("Search {location}", location = location)
                                }
                                SearchScope::IndexedFiles => {
                                    tr("Search indexed files").into_owned()
                                }
                                SearchScope::SelectedRoots => {
                                    tr("Search selected roots").into_owned()
                                }
                            }),
                    )
                    .on_hover_text(crate::tr!(
                        "Ctrl+F · e.g. {example}",
                        example = "type:xlsx tag:work"
                    ));
                if state.focus_requested {
                    response.request_focus();
                    state.focus_requested = false;
                }
                if response.changed() {
                    action = Some(SearchAction::Changed);
                }
                if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                    action = Some(SearchAction::Execute);
                }
                if state.is_searching {
                    ui.spinner();
                }
                if !state.query.is_empty()
                    && ui
                        .add(egui::Button::new(X).frame(false))
                        .on_hover_text(tr("Clear search"))
                        .clicked()
                {
                    state.query.clear();
                    action = Some(SearchAction::Clear);
                }
                ui.menu_button(FUNNEL_SIMPLE, |ui| {
                    ui.weak(tr("Scope"));
                    ui.radio_value(
                        &mut state.scope,
                        SearchScope::CurrentFolder,
                        tr("Current folder"),
                    );
                    ui.radio_value(
                        &mut state.scope,
                        SearchScope::IndexedFiles,
                        tr("Indexed files"),
                    );
                    ui.radio_value(
                        &mut state.scope,
                        SearchScope::SelectedRoots,
                        tr("Selected roots (fff)"),
                    );
                    ui.separator();
                    ui.weak(tr("Mode"));
                    for mode in [
                        SearchMode::Keyword,
                        SearchMode::Semantic,
                        SearchMode::Hybrid,
                    ] {
                        ui.radio_value(&mut state.mode, mode, tr(&format!("{mode:?}")));
                    }
                    ui.separator();
                    ui.weak(tr("Filters (combined with AND)"));
                    for example in [
                        "intitle:\"monthly report\"",
                        "type:document  type:xlsx",
                        "tag:\"AI Agent\"",
                        "date:2026-10-01..2026-10-15",
                    ] {
                        ui.monospace(example);
                    }
                })
                .response
                .on_hover_text(crate::tr!(
                    "Scope and mode · {mode}",
                    mode = format!("{:?}", state.mode)
                ));
            });
        });
    if old != (state.mode, state.scope) {
        action = Some(SearchAction::Changed);
    }
    action
}

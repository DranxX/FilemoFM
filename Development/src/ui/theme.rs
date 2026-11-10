use crate::shared::models::GeneralConfig;
use egui::{Color32, FontId, Stroke, Style, Visuals};

pub struct FilemoTheme;
impl FilemoTheme {
    pub const SUCCESS: Color32 = Color32::from_rgb(34, 197, 94);
    pub const DANGER: Color32 = Color32::from_rgb(239, 68, 68);
    pub const WARNING: Color32 = Color32::from_rgb(234, 179, 8);
    pub fn apply(ctx: &egui::Context, config: &GeneralConfig) {
        ctx.set_theme(match config.theme.as_str() {
            "light" => egui::ThemePreference::Light,
            "system" => egui::ThemePreference::System,
            _ => egui::ThemePreference::Dark,
        });
        ctx.set_zoom_factor(config.ui_scale);
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            let dark = theme == egui::Theme::Dark;
            let mut style = Style {
                visuals: if dark {
                    Visuals::dark()
                } else {
                    Visuals::light()
                },
                ..Default::default()
            };
            if dark {
                style.visuals.panel_fill = Color32::from_rgb(22, 24, 29);
                style.visuals.window_fill = Color32::from_rgb(30, 33, 39);
                style.visuals.faint_bg_color = Color32::from_rgb(30, 33, 39);
                style.visuals.extreme_bg_color = Color32::from_rgb(15, 17, 21);
                style.visuals.override_text_color = Some(Color32::from_rgb(229, 231, 235));
                style.visuals.weak_text_color = Some(Color32::from_rgb(156, 163, 175));
            }
            let accent = if dark {
                Color32::from_rgb(96, 165, 250)
            } else {
                Color32::from_rgb(29, 78, 216)
            };
            style.visuals.hyperlink_color = accent;
            style.visuals.selection.bg_fill = if dark {
                Color32::from_rgb(35, 56, 87)
            } else {
                Color32::from_rgb(219, 234, 254)
            };
            style.visuals.selection.stroke = Stroke::new(1.0_f32, accent);
            style.visuals.widgets.active.bg_fill = style.visuals.selection.bg_fill;
            style.visuals.widgets.active.fg_stroke = Stroke::new(
                1.0_f32,
                if dark {
                    Color32::WHITE
                } else {
                    Color32::from_rgb(15, 23, 42)
                },
            );
            for widget in [
                &mut style.visuals.widgets.inactive,
                &mut style.visuals.widgets.hovered,
                &mut style.visuals.widgets.active,
            ] {
                widget.corner_radius = egui::CornerRadius::same(6);
            }
            style.visuals.window_corner_radius = egui::CornerRadius::same(8);
            style.visuals.window_shadow = egui::epaint::Shadow::NONE;
            style.spacing.item_spacing = egui::vec2(8.0, 4.0);
            style.spacing.button_padding = egui::vec2(8.0, 5.0);
            style.interaction.selectable_labels = true;
            for (kind, size) in [
                (egui::TextStyle::Body, config.font_size),
                (egui::TextStyle::Button, config.font_size),
                (egui::TextStyle::Small, config.font_size - 1.0),
                (egui::TextStyle::Heading, config.font_size + 2.0),
            ] {
                style.text_styles.insert(kind, FontId::proportional(size));
            }
            style.text_styles.insert(
                egui::TextStyle::Monospace,
                FontId::monospace(config.font_size),
            );
            ctx.set_style_of(theme, style);
        }
    }
    pub fn tag_color(h: f32, s: f32, l: f32) -> Color32 {
        let h = h.rem_euclid(360.0) / 60.0;
        let chroma = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let x = chroma * (1.0 - (h % 2.0 - 1.0).abs());
        let m = l - chroma / 2.0;
        let (r, g, b) = match h as u8 {
            0 => (chroma, x, 0.0),
            1 => (x, chroma, 0.0),
            2 => (0.0, chroma, x),
            3 => (0.0, x, chroma),
            4 => (x, 0.0, chroma),
            _ => (chroma, 0.0, x),
        };
        Color32::from_rgb(
            ((r + m) * 255.0) as u8,
            ((g + m) * 255.0) as u8,
            ((b + m) * 255.0) as u8,
        )
    }
}

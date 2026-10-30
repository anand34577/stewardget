//! Themes, accent colours, fonts and the egui style built from them.

use std::sync::Arc;

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Visuals,
};
use stewardget_core::{Accent, Category, Status, Theme};

/// Height shared by buttons, text fields, combo boxes and sliders, so rows of
/// mixed controls line up.
pub const CONTROL_H: f32 = 32.0;

/// Type scale (points). Body text is `MD`; everything else steps from it.
pub mod size {
    pub const SM: f32 = 12.5;
    pub const MD: f32 = 14.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 20.0;
    pub const XXL: f32 = 26.0;
    pub const DISPLAY: f32 = 34.0;
}

/// Corner radii.
pub mod radius {
    pub const SM: u8 = 6;
    pub const MD: u8 = 9;
    pub const LG: u8 = 12;
    pub const XL: u8 = 16;
}

/// Letter spacing for short upper-case labels (about 7% of their size).
pub const CAPS_TRACKING: f32 = 0.8;

#[derive(Clone, Copy)]
pub struct Palette {
    /// The canvas: the rounded surface pages are drawn on.
    pub bg: Color32,
    /// The window shell around the canvas: sidebar, header and status bar.
    pub sidebar: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub border: Color32,
    /// Outline of inputs and other controls; stronger than `border`.
    pub control_border: Color32,
    pub input_bg: Color32,
    pub track: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    /// Text and icons drawn on top of `accent`.
    pub on_accent: Color32,
    pub accent2: Color32,
    pub green: Color32,
    pub yellow: Color32,
    pub red: Color32,
    pub dark: bool,
}

impl Palette {
    /// `system_dark` resolves [`Theme::System`].
    pub fn new(theme: Theme, accent: Accent, system_dark: bool) -> Self {
        let theme = match theme {
            Theme::System if system_dark => Theme::Dark,
            Theme::System => Theme::Light,
            t => t,
        };
        let rgb = Color32::from_rgb;
        let mut p = match theme {
            Theme::Light | Theme::System => Self {
                bg: rgb(252, 252, 253),
                sidebar: rgb(241, 242, 246),
                card: rgb(255, 255, 255),
                card_hover: rgb(243, 244, 248),
                border: rgb(228, 230, 237),
                control_border: rgb(208, 212, 223),
                input_bg: rgb(255, 255, 255),
                track: rgb(232, 234, 241),
                text: rgb(15, 17, 26),
                text_dim: rgb(86, 94, 114),
                accent: Color32::TRANSPARENT,
                on_accent: Color32::WHITE,
                accent2: Color32::TRANSPARENT,
                // Dark enough for small status text on the light background (4.5:1).
                green: rgb(4, 120, 87),
                yellow: rgb(180, 83, 9),
                red: rgb(220, 38, 38),
                dark: false,
            },
            Theme::Dark => Self {
                bg: rgb(16, 18, 24),
                sidebar: rgb(10, 11, 15),
                card: rgb(22, 25, 33),
                card_hover: rgb(30, 34, 45),
                border: rgb(36, 40, 52),
                control_border: rgb(54, 60, 77),
                input_bg: rgb(12, 14, 19),
                track: rgb(37, 42, 55),
                text: rgb(237, 239, 245),
                text_dim: rgb(150, 157, 176),
                ..Self::dark_base()
            },
            Theme::Midnight => Self {
                bg: rgb(9, 9, 11),
                sidebar: rgb(0, 0, 0),
                card: rgb(16, 16, 19),
                card_hover: rgb(25, 25, 30),
                border: rgb(32, 32, 38),
                control_border: rgb(52, 52, 60),
                input_bg: rgb(5, 5, 7),
                track: rgb(31, 31, 37),
                text: rgb(238, 238, 242),
                text_dim: rgb(145, 145, 157),
                ..Self::dark_base()
            },
            Theme::Ocean => Self {
                bg: rgb(11, 21, 37),
                sidebar: rgb(7, 14, 26),
                card: rgb(16, 29, 50),
                card_hover: rgb(23, 39, 65),
                border: rgb(30, 48, 77),
                control_border: rgb(46, 68, 104),
                input_bg: rgb(8, 17, 31),
                track: rgb(29, 46, 73),
                text: rgb(229, 237, 249),
                text_dim: rgb(143, 161, 189),
                ..Self::dark_base()
            },
        };
        let (a, a2) = accent_colors(accent, p.dark);
        p.accent = a;
        p.accent2 = a2;
        p.on_accent = if luminance(a) > 0.55 { rgb(17, 20, 28) } else { Color32::WHITE };
        p
    }

    fn dark_base() -> Self {
        Self {
            bg: Color32::BLACK,
            sidebar: Color32::BLACK,
            card: Color32::BLACK,
            card_hover: Color32::BLACK,
            border: Color32::BLACK,
            control_border: Color32::BLACK,
            input_bg: Color32::BLACK,
            track: Color32::BLACK,
            text: Color32::WHITE,
            text_dim: Color32::GRAY,
            accent: Color32::TRANSPARENT,
            on_accent: Color32::WHITE,
            accent2: Color32::TRANSPARENT,
            green: Color32::from_rgb(52, 211, 153),
            yellow: Color32::from_rgb(251, 191, 36),
            red: Color32::from_rgb(248, 113, 113),
            dark: true,
        }
    }

    pub fn status(&self, status: Status) -> Color32 {
        match status {
            Status::Queued => self.text_dim,
            Status::Connecting | Status::Downloading => self.accent2,
            Status::Verifying => self.accent,
            Status::Paused => self.yellow,
            Status::Completed => self.green,
            Status::Failed => self.red,
        }
    }

    pub fn category(&self, category: Category) -> Color32 {
        let rgb = Color32::from_rgb;
        match (category, self.dark) {
            (Category::Video, true) => rgb(244, 114, 182),
            (Category::Video, false) => rgb(219, 39, 119),
            (Category::Audio, true) => rgb(52, 211, 153),
            (Category::Audio, false) => rgb(5, 150, 105),
            (Category::Image, true) => rgb(251, 146, 60),
            (Category::Image, false) => rgb(234, 88, 12),
            (Category::Document, true) => rgb(96, 165, 250),
            (Category::Document, false) => rgb(37, 99, 235),
            (Category::Archive, true) => rgb(250, 204, 21),
            (Category::Archive, false) => rgb(180, 120, 4),
            (Category::Program, true) => rgb(167, 139, 250),
            (Category::Program, false) => rgb(124, 58, 237),
            (Category::Other, _) => self.text_dim,
            // Custom categories take turns through a set of hues the built-in ones don't use.
            (Category::Custom(id), dark) => {
                const DARK: [(u8, u8, u8); 6] =
                    [(56, 189, 248), (232, 121, 249), (163, 230, 53), (251, 113, 133), (45, 212, 191), (253, 186, 116)];
                const LIGHT: [(u8, u8, u8); 6] =
                    [(2, 132, 199), (192, 38, 211), (77, 124, 15), (225, 29, 72), (15, 118, 110), (194, 65, 12)];
                let (r, g, b) = if dark { DARK } else { LIGHT }[id as usize % 6];
                rgb(r, g, b)
            }
        }
    }

    /// Soft background for something drawn in `color`: icon tiles, pills, badges.
    pub fn tint(&self, color: Color32) -> Color32 {
        color.gamma_multiply(if self.dark { 0.16 } else { 0.11 })
    }

    /// The signature gradient: the accent leaning towards its partner colour,
    /// kept close enough to the accent that `on_accent` text stays readable.
    pub fn gradient(&self) -> (Color32, Color32) {
        let mix = |a: Color32, b: Color32, t: f32| {
            let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
            Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
        };
        // Shaded away from the label colour, so the label keeps its contrast across the button.
        let away = if self.on_accent == Color32::WHITE { Color32::BLACK } else { Color32::WHITE };
        (mix(self.accent, away, 0.06), mix(mix(self.accent, self.accent2, 0.35), away, 0.14))
    }

    /// Text drawn on a solid fill of `color`.
    pub fn on_tile(&self, color: Color32) -> Color32 {
        if luminance(color) > 0.55 { Color32::from_rgb(17, 20, 28) } else { Color32::WHITE }
    }

    /// Fill of the selected row or navigation entry.
    pub fn selected_fill(&self) -> Color32 {
        self.accent.gamma_multiply(if self.dark { 0.17 } else { 0.10 })
    }

    /// Dimmed backdrop behind modal dialogs.
    pub fn backdrop(&self) -> Color32 {
        if self.dark { Color32::from_black_alpha(165) } else { Color32::from_rgba_unmultiplied(15, 17, 26, 90) }
    }

    pub fn shadow(&self, offset: i8, blur: u8) -> Shadow {
        let alpha = if self.dark { 120 } else { 22 };
        Shadow { offset: [0, offset], blur, spread: 0, color: Color32::from_black_alpha(alpha) }
    }

    /// Coloured halo under primary buttons and live progress.
    pub fn glow(&self, color: Color32, blur: u8) -> Shadow {
        Shadow { offset: [0, 3], blur, spread: 0, color: color.gamma_multiply(if self.dark { 0.35 } else { 0.28 }) }
    }

    pub fn apply(&self, ctx: &egui::Context) {
        let mut visuals = if self.dark { Visuals::dark() } else { Visuals::light() };
        visuals.dark_mode = self.dark;
        visuals.panel_fill = self.sidebar;
        visuals.window_fill = self.card;
        visuals.window_stroke = Stroke::new(1.0, self.border);
        visuals.window_corner_radius = CornerRadius::same(radius::XL);
        visuals.menu_corner_radius = CornerRadius::same(radius::LG);
        visuals.window_shadow = self.shadow(16, 48);
        visuals.popup_shadow = self.shadow(8, 24);
        visuals.extreme_bg_color = self.input_bg;
        visuals.text_edit_bg_color = Some(self.input_bg);
        visuals.code_bg_color = self.card_hover;
        visuals.faint_bg_color = self.card_hover;
        visuals.hyperlink_color = self.accent2;
        visuals.warn_fg_color = self.yellow;
        visuals.error_fg_color = self.red;
        visuals.selection.bg_fill = self.accent.gamma_multiply(if self.dark { 0.45 } else { 0.25 });
        visuals.selection.stroke = Stroke::new(1.5, self.accent);
        visuals.override_text_color = None;
        visuals.slider_trailing_fill = true;
        visuals.handle_shape = egui::style::HandleShape::Circle;
        visuals.striped = false;

        let radius = CornerRadius::same(radius::MD);
        let w = &mut visuals.widgets;
        w.noninteractive.bg_fill = self.card;
        w.noninteractive.weak_bg_fill = self.card;
        w.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
        w.noninteractive.fg_stroke = Stroke::new(1.0, self.text);
        w.noninteractive.corner_radius = radius;
        let fills = [
            (&mut w.inactive, self.card_hover, self.control_border, self.text),
            (&mut w.hovered, self.track, self.accent.gamma_multiply(0.8), self.text),
            (&mut w.active, self.accent.gamma_multiply(0.35), self.accent, self.text),
            (&mut w.open, self.card_hover, self.accent, self.text),
        ];
        for (state, fill, stroke, fg) in fills {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.corner_radius = radius;
            state.bg_stroke = Stroke::new(1.0, stroke);
            state.fg_stroke = Stroke::new(1.5, fg);
            state.expansion = 0.0;
        }
        // Slider rails and checkbox boxes use the inactive fill.
        w.inactive.bg_fill = self.track;

        // Both slots: `set_theme` only takes effect on a later frame, so the
        // "current" theme may still be the other one right now.
        ctx.set_visuals_of(egui::Theme::Dark, visuals.clone());
        ctx.set_visuals_of(egui::Theme::Light, visuals);
        ctx.all_styles_mut(|style| {
            style.text_styles = [
                (TextStyle::Heading, FontId::new(size::XL, semi())),
                (TextStyle::Body, FontId::new(size::MD, FontFamily::Proportional)),
                (TextStyle::Button, FontId::new(size::MD, FontFamily::Proportional)),
                (TextStyle::Small, FontId::new(size::SM, FontFamily::Proportional)),
                (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
            ]
            .into();
            let s = &mut style.spacing;
            s.item_spacing = egui::vec2(8.0, 6.0);
            s.button_padding = egui::vec2(12.0, 5.0);
            s.interact_size = egui::vec2(40.0, CONTROL_H);
            s.combo_height = 320.0;
            s.window_margin = Margin::same(20);
            s.menu_margin = Margin::same(6);
            s.slider_width = 180.0;
            s.icon_width = 18.0;
            s.icon_width_inner = 10.0;
            s.icon_spacing = 8.0;
            s.scroll = egui::style::ScrollStyle::floating();
            style.interaction.selectable_labels = false;
        });
    }
}

fn accent_colors(accent: Accent, dark: bool) -> (Color32, Color32) {
    let rgb = Color32::from_rgb;
    match (accent, dark) {
        (Accent::Violet, true) => (rgb(139, 108, 255), rgb(34, 211, 238)),
        (Accent::Violet, false) => (rgb(109, 74, 255), rgb(8, 145, 178)),
        (Accent::Blue, true) => (rgb(84, 142, 255), rgb(45, 212, 191)),
        (Accent::Blue, false) => (rgb(37, 99, 235), rgb(13, 148, 136)),
        (Accent::Teal, true) => (rgb(45, 212, 191), rgb(96, 165, 250)),
        (Accent::Teal, false) => (rgb(13, 148, 136), rgb(37, 99, 235)),
        (Accent::Green, true) => (rgb(74, 222, 128), rgb(45, 212, 191)),
        (Accent::Green, false) => (rgb(22, 163, 74), rgb(13, 148, 136)),
        (Accent::Amber, true) => (rgb(251, 191, 36), rgb(251, 146, 60)),
        (Accent::Amber, false) => (rgb(217, 119, 6), rgb(234, 88, 12)),
        (Accent::Rose, true) => (rgb(251, 113, 133), rgb(192, 132, 252)),
        (Accent::Rose, false) => (rgb(225, 29, 72), rgb(147, 51, 234)),
    }
}

fn luminance(c: Color32) -> f32 {
    (0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32) / 255.0
}

/// The semibold Inter family, for titles and emphasis.
pub fn semi() -> FontFamily {
    FontFamily::Name("semibold".into())
}

/// The icon font family.
pub fn icon_family() -> FontFamily {
    FontFamily::Name("icons".into())
}

/// Installs Inter (UI text) and the Lucide icon font. egui's bundled fonts stay
/// as fallbacks for scripts Inter doesn't cover.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    };
    add(&mut fonts, "inter", include_bytes!("../assets/Inter-Regular.ttf"));
    add(&mut fonts, "inter-semibold", include_bytes!("../assets/Inter-SemiBold.ttf"));
    add(&mut fonts, "lucide", include_bytes!("../assets/lucide-subset.ttf"));

    let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut proportional = vec!["inter".to_owned()];
    proportional.extend(fallbacks.iter().cloned());
    let mut semibold = vec!["inter-semibold".to_owned()];
    semibold.extend(fallbacks);
    fonts.families.insert(FontFamily::Proportional, proportional);
    fonts.families.insert(semi(), semibold);
    fonts.families.insert(icon_family(), vec!["lucide".to_owned()]);
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: Color32) -> f32 {
        let f = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// Body text must be clearly readable and dimmed text at least AA-large readable
    /// on every surface it sits on, in every theme and accent.
    #[test]
    fn text_is_readable_in_every_theme() {
        for theme in [Theme::Light, Theme::Dark, Theme::Midnight, Theme::Ocean] {
            for accent in Accent::ALL {
                let p = Palette::new(theme, accent, false);
                for (name, bg) in [("bg", p.bg), ("card", p.card), ("sidebar", p.sidebar)] {
                    let body = contrast(p.text, bg);
                    let dim = contrast(p.text_dim, bg);
                    assert!(body >= 7.0, "{theme:?}/{accent:?}: text on {name} is {body:.1}:1");
                    assert!(dim >= 3.5, "{theme:?}/{accent:?}: dim text on {name} is {dim:.1}:1");
                }
                let on = contrast(p.on_accent, p.accent);
                assert!(on >= 3.0, "{theme:?}/{accent:?}: text on the accent is {on:.1}:1");
                // Primary buttons run into the gradient's lighter end.
                let end = contrast(p.on_accent, p.gradient().1);
                assert!(end >= 3.0, "{theme:?}/{accent:?}: text on the gradient's end is {end:.1}:1");
            }
        }
    }
}

//! Zentrale Theme-Konstanten der UI (Sidebar + Viewport-Overlays).
//!
//! Alle Farben, Schriftgrößen und Abstände liegen hier — nirgends sonst
//! stehen harte Pixel- oder Farbwerte.
//!
//! Hell/Dunkel wird über das egui-Theme aufgelöst: Die UI folgt automatisch
//! der Systemeinstellung (macOS Appearance / Windows Hell/Dunkel) und wechselt
//! bei Änderung des OS-Looks live zwischen [`ThemeColors::DARK`] und [`ThemeColors::LIGHT`].

use eframe::egui::{self, Color32};

/// Cyan der Schattenkontur-Vorschau (wie `renderer::SHADOW_PREVIEW`).
pub const PREVIEW: Color32 = Color32::from_rgb(40, 225, 255);

// ---------------------------------------------------------
// Farbwelt: dunkle Leiste, Karten-Stufen, gedämpfte Akzente
// --------------------------------------------------------

/// Eine komplette Farbpalette für einen Theme-Modus (Hell oder Dunkel).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeColors {
    /// Grundfläche der Sidebar.
    pub bg: Color32,
    /// Träger hinter Karten (Chips, Werkzeugleiste).
    pub surface: Color32,
    /// Kartenfläche.
    pub card: Color32,
    /// Uhrenzeile/Trenner.
    pub stroke: Color32,
    /// Primärtext.
    pub text: Color32,
    /// Sekundärtext (Hinweise, Fußnoten).
    pub text_muted: Color32,
    /// Akzent — nur für Rohteil/Ergebnis (Orange, entspricht der Rohteil-Box).
    pub accent: Color32,
    /// Text auf Akzentfläche (dunkles Kontrast-Orange).
    pub accent_text: Color32,
    /// OK-Status (grün).
    pub ok: Color32,
    /// Hinweis/Inaktiv (gelblich).
    pub warn: Color32,
    /// Fehler (rot).
    pub error: Color32,
    /// Cyan der Schattenkontur-Vorschau (wie `renderer::SHADOW_PREVIEW`).
    pub preview: Color32,
    /// Achsen-Gizmo: konventionelle Achsenfarben (X rot, Y grün, Z blau).
    pub axis_x: Color32,
    pub axis_y: Color32,
    pub axis_z: Color32,
    pub axis_origin: Color32,
    /// Legende im Viewport (Farben wie die Szene).
    pub legend_stock: Color32,
    pub legend_part: Color32,
    pub legend_clamp: Color32,
    /// Hintergrund des 3D-Fensters (hell, wie bisher — Szene bleibt drauf lesbar).
    pub viewport_bg: Color32,
    /// Drag-&-Drop-Hinweis (Datei ins Fenster ziehen): Abdunkelung, Rahmen, Text.
    pub dnd_fill: Color32,
    pub dnd_stroke: Color32,
    pub dnd_text: Color32,
    /// Hinweistext, wenn keine Geometrie geladen ist.
    pub empty_text: Color32,
}

impl ThemeColors {
    /// Eigene Dark-Palette (wie bisher).
    pub const DARK: Self = Self {
        bg: Color32::from_rgb(22, 25, 31),
        surface: Color32::from_rgb(30, 34, 42),
        card: Color32::from_rgb(38, 43, 53),
        stroke: Color32::from_rgb(58, 66, 78),
        text: Color32::from_rgb(226, 230, 238),
        text_muted: Color32::from_rgb(148, 156, 168),
        accent: Color32::from_rgb(255, 160, 45),
        accent_text: Color32::from_rgb(30, 22, 10),
        ok: Color32::from_rgb(96, 200, 124),
        warn: Color32::from_rgb(222, 168, 60),
        error: Color32::from_rgb(226, 96, 82),
        preview: Color32::from_rgb(40, 225, 255),
        axis_x: Color32::from_rgb(220, 70, 70),
        axis_y: Color32::from_rgb(60, 170, 90),
        axis_z: Color32::from_rgb(80, 130, 240),
        axis_origin: Color32::from_gray(140),
        legend_stock: Color32::from_rgb(255, 199, 69),
        legend_part: Color32::from_rgb(52, 62, 74),
        legend_clamp: Color32::from_rgb(229, 89, 89),
        viewport_bg: Color32::from_rgb(210, 216, 226),
        dnd_fill: Color32::from_rgba_unmultiplied_const(20, 30, 45, 180),
        dnd_stroke: Color32::from_rgb(120, 200, 255),
        dnd_text: Color32::from_rgb(220, 240, 255),
        empty_text: Color32::GRAY,
    };

    /// Eigene Light-Palette.
    pub const LIGHT: Self = Self {
        bg: Color32::from_rgb(243, 246, 250),
        surface: Color32::from_rgb(230, 234, 241),
        card: Color32::from_rgb(255, 255, 255),
        stroke: Color32::from_rgb(209, 217, 229),
        text: Color32::from_rgb(31, 41, 55),
        text_muted: Color32::from_rgb(100, 116, 139),
        accent: Color32::from_rgb(255, 160, 45),
        accent_text: Color32::from_rgb(30, 22, 10),
        ok: Color32::from_rgb(96, 200, 124),
        warn: Color32::from_rgb(222, 168, 60),
        error: Color32::from_rgb(226, 96, 82),
        preview: Color32::from_rgb(40, 225, 255),
        axis_x: Color32::from_rgb(220, 70, 70),
        axis_y: Color32::from_rgb(60, 170, 90),
        axis_z: Color32::from_rgb(80, 130, 240),
        axis_origin: Color32::from_gray(140),
        legend_stock: Color32::from_rgb(255, 199, 69),
        legend_part: Color32::from_rgb(148, 163, 184),
        legend_clamp: Color32::from_rgb(229, 89, 89),
        viewport_bg: Color32::from_rgb(250, 251, 253),
        dnd_fill: Color32::from_rgba_unmultiplied_const(20, 30, 45, 180),
        dnd_stroke: Color32::from_rgb(120, 200, 255),
        dnd_text: Color32::from_rgb(220, 240, 255),
        empty_text: Color32::from_rgb(110, 118, 134),
    };
}

/// Farben für das übergebene egui-Theme (Hell oder Dunkel).
pub fn colors_for(theme: egui::Theme) -> ThemeColors {
    match theme {
        egui::Theme::Dark => ThemeColors::DARK,
        egui::Theme::Light => ThemeColors::LIGHT,
    }
}

/// Aktuelle Farbpalette (Hell/Dunkel) aus dem egui-Context.
pub fn colors(ctx: &egui::Context) -> ThemeColors {
    colors_for(ctx.theme())
}

// ---------------------------------------------------------
// Schriftgrößen (pt)
// --------------------------------------------------------

pub const SIZE_TITLE: f32 = 16.0;
pub const SIZE_SECTION: f32 = 13.0;
pub const SIZE_BODY: f32 = 12.0;
pub const SIZE_HINT: f32 = 11.0;

// ---------------------------------------------------------
// Abstände / Formen
// --------------------------------------------------------

/// Abstand zwischen Karten.
pub const SPACING_CARD: f32 = 8.0;
/// Innenabstand einer Karte.
pub const PAD_CARD: f32 = 6.0;
/// Eckradius von Karten und Chips (px).
pub const CARD_ROUNDING: f32 = 6.0;

// Sidebar
pub const SIDEBAR_MIN: f32 = 280.0;
pub const SIDEBAR_DEFAULT: f32 = 320.0;
pub const SIDEBAR_MAX: f32 = 420.0;

/// Setzt die Schriftgrößen der Sidebar zentral.
pub(crate) fn apply_text_styles(ui: &mut eframe::egui::Ui) {
    use eframe::egui::{FontId, TextStyle};
    ui.style_mut().text_styles = [
        (TextStyle::Heading, FontId::proportional(SIZE_TITLE)),
        (TextStyle::Body, FontId::proportional(SIZE_BODY)),
        (TextStyle::Button, FontId::proportional(SIZE_BODY)),
        (TextStyle::Monospace, FontId::monospace(SIZE_BODY)),
        (TextStyle::Small, FontId::proportional(SIZE_HINT)),
    ]
    .into_iter()
    .collect();
    ui.visuals_mut().override_text_color = Some(colors(ui.ctx()).text);
}

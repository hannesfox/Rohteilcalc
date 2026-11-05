//! Zentrale Theme-Konstanten der UI (Sidebar + Viewport-Overlays).
//!
//! Alle Farben, Schriftgrößen und Abstände liegen hier — nirgends sonst
//! stehen harte Pixel- oder Farbwerte.

use eframe::egui::Color32;

// ---------------------------------------------------------
// Farbwelt: dunkle Leiste, Karten-Stufen, gedämpfte Akzente
// ---------------------------------------------------------

/// Grundfläche der Sidebar.
pub const BG: Color32 = Color32::from_rgb(22, 25, 31);
/// Träger hinter Karten (Chips, Werkzeugleiste).
pub const SURFACE: Color32 = Color32::from_rgb(30, 34, 42);
/// Kartenfläche.
pub const CARD: Color32 = Color32::from_rgb(38, 43, 53);
/// Uhrenzeile/Trenner.
pub const STROKE: Color32 = Color32::from_rgb(58, 66, 78);
/// Primärtext.
pub const TEXT: Color32 = Color32::from_rgb(226, 230, 238);
/// Sekundärtext (Hinweise, Fußnoten).
pub const TEXT_MUTED: Color32 = Color32::from_rgb(148, 156, 168);

/// Akzent — nur für Rohteil/Ergebnis (Orange, entspricht der Rohteil-Box).
pub const ACCENT: Color32 = Color32::from_rgb(255, 160, 45);
/// Text auf Akzentfläche (dunkles Kontrast-Orange).
pub const ACCENT_TEXT: Color32 = Color32::from_rgb(30, 22, 10);

/// OK-Status (grün).
pub const OK: Color32 = Color32::from_rgb(96, 200, 124);
/// Hinweis/Inaktiv (gelblich).
pub const WARN: Color32 = Color32::from_rgb(222, 168, 60);
/// Fehler (rot).
pub const ERROR: Color32 = Color32::from_rgb(226, 96, 82);

/// Cyan der Schattenkontur-Vorschau (wie `renderer::SHADOW_PREVIEW`).
pub const PREVIEW: Color32 = Color32::from_rgb(40, 225, 255);

/// Achsen-Gizmo: konventionelle Achsenfarben (X rot, Y grün, Z blau).
pub const AXIS_X: Color32 = Color32::from_rgb(220, 70, 70);
pub const AXIS_Y: Color32 = Color32::from_rgb(60, 170, 90);
pub const AXIS_Z: Color32 = Color32::from_rgb(80, 130, 240);
/// Achsen-Gizmo: Ursprungspunkt.
pub const AXIS_ORIGIN: Color32 = Color32::from_gray(140);

// Legende im Viewport (Farben wie die Szene).
/// Rohteil-Box (Orange, wie `renderer::STOCK_FILL`).
pub const LEGEND_STOCK: Color32 = Color32::from_rgb(255, 199, 69);
/// Bauteilkörper (dunkel).
pub const LEGEND_PART: Color32 = Color32::from_rgb(52, 62, 74);
/// Spannfläche/Z-Null (rot, wie `renderer::STOCK_CLAMP`).
pub const LEGEND_CLAMP: Color32 = Color32::from_rgb(229, 89, 89);

/// Hintergrund des 3D-Fensters (hell, wie bisher — Szene bleibt drauf lesbar).
pub const VIEWPORT_BG: Color32 = Color32::from_rgb(210, 216, 226);

/// Drag-&-Drop-Hinweis (Datei ins Fenster ziehen): Abdunkelung, Rahmen, Text.
pub const DND_FILL: Color32 = Color32::from_rgba_unmultiplied_const(20, 30, 45, 180);
pub const DND_STROKE: Color32 = Color32::from_rgb(120, 200, 255);
pub const DND_TEXT: Color32 = Color32::from_rgb(220, 240, 255);
/// Hinweistext, wenn keine Geometrie geladen ist.
pub const EMPTY_TEXT: Color32 = Color32::GRAY;

// ---------------------------------------------------------
// Schriftgrößen (pt)
// ---------------------------------------------------------

pub const SIZE_TITLE: f32 = 16.0;
pub const SIZE_SECTION: f32 = 13.0;
pub const SIZE_BODY: f32 = 12.0;
pub const SIZE_HINT: f32 = 11.0;

// ---------------------------------------------------------
// Abstände / Formen
// ---------------------------------------------------------

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
    ui.visuals_mut().override_text_color = Some(TEXT);
}

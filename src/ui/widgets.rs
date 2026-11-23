//! Kleine UI-Bausteine: Karten, Überschriften/Hinweisen, Toggle-Schalter,
//! Segmented Control, primäre Buttons und Statuspunkte.

use eframe::egui::{self, InnerResponse, Margin, Pos2, Response, RichText, Ui, Vec2};

use super::theme;

/// Eine dunkle Karte mit abgerundeten Ecken und festem Innenabstand.
/// Füllt die volle Leuchtbreite.
pub(crate) fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    let colors = theme::colors(ui.ctx());
    ui.set_min_width(ui.available_width());
    egui::Frame::new()
        .fill(colors.card)
        .corner_radius(theme::CARD_ROUNDING as u8)
        .inner_margin(Margin::same(theme::PAD_CARD as i8))
        .show(ui, add)
}

/// Dickere Sektionsüberschrift (13 pt) in einer Karte.
pub(crate) fn section_label(ui: &mut Ui, text: &str) {
    let colors = theme::colors(ui.ctx());
    ui.label(
        RichText::new(text)
            .strong()
            .size(theme::SIZE_SECTION)
            .color(colors.text),
    );
}

/// Graue kleine Zusatzzeile (11 pt).
pub(crate) fn hint(ui: &mut Ui, text: &str) {
    let colors = theme::colors(ui.ctx());
    ui.label(
        RichText::new(text)
            .size(theme::SIZE_HINT)
            .color(colors.text_muted),
    );
}

/// Monospace-Zahlenzelle, rechtsbündig im Grid.
pub(crate) fn mono_right(ui: &mut Ui, text: &str) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.label(RichText::new(text).monospace());
    });
}

/// Primärer, gefüllter Button (Akzent-Orange) — für die Fußleiste.
pub(crate) fn button_primary(ui: &mut Ui, text: &str) -> egui::Button<'static> {
    let colors = theme::colors(ui.ctx());
    egui::Button::new(RichText::new(text).color(colors.accent_text).strong())
        .fill(colors.accent)
        .corner_radius(theme::CARD_ROUNDING as u8)
}

/// Toggle-Schalter ( McGregor ohne egui-Basis-Widget).
pub(crate) fn toggle_switch(ui: &mut Ui, checked: &mut bool) -> Response {
    let colors = theme::colors(ui.ctx());
    let size = Vec2::new(30.0, 16.0);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *checked = !*checked;
        response.mark_changed();
    }
    let painter = ui.painter();
    let fill = if *checked { colors.ok } else { colors.stroke };
    painter.rect_filled(
        rect.shrink2(Vec2::new(1.0, 1.0)),
        (rect.height() * 0.5) as u8,
        fill,
    );
    let r = rect.height() * 0.5 - 2.0;
    let knob_x = if *checked {
        rect.right() - r - 3.0
    } else {
        rect.left() + r + 3.0
    };
    painter.circle_filled(Pos2::new(knob_x, rect.center().y), r, colors.text);
    response
}

/// Segmented Control: gerade Button-Fahne, Auswahl highlighted.
/// Liefert true, wenn `current` verändert wurde.
pub(crate) fn segmented<T: Copy + PartialEq>(
    ui: &mut Ui,
    current: &mut T,
    options: &[(T, &str)],
) -> bool {
    let colors = theme::colors(ui.ctx());
    let mut changed = false;
    ui.horizontal(|ui| {
        for (i, (value, label)) in options.iter().enumerate() {
            if i > 0 {
                ui.add_space(2.0);
            }
            let selected = *current == *value;
            let text = RichText::new(*label).color(if selected {
                colors.accent_text
            } else {
                colors.text
            });
            let button = egui::Button::new(text)
                .fill(if selected {
                    colors.accent
                } else {
                    colors.surface
                })
                .corner_radius(theme::CARD_ROUNDING as u8);
            if ui.add(button).clicked() && !selected {
                *current = *value;
                changed = true;
            }
        }
    });
    changed
}

/// Kleiner Statuspunkt (z. B. für Statuszeilen mit Icon).
pub(crate) fn status_dot(ui: &mut Ui, color: eframe::egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(10.0, 12.0), egui::Sense::hover());
    ui.painter()
        .circle_filled(Pos2::new(rect.center().x, rect.center().y), 3.5, color);
}

/// Ein (i)-Info-Icon mit Tooltip.
pub(crate) fn info_icon(ui: &mut Ui, text: &str) {
    let colors = theme::colors(ui.ctx());
    let (_, response) = ui.allocate_exact_size(Vec2::new(14.0, 12.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.circle_filled(response.rect.center(), 6.0, colors.stroke);
    painter.text(
        response.rect.center(),
        egui::Align2::CENTER_CENTER,
        "i",
        egui::FontId::proportional(theme::SIZE_HINT),
        colors.text,
    );
    response.on_hover_text(text);
}

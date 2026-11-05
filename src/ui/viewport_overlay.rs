//! Overlays im 3D-Fenster: schwebende Ansichten-Leiste oben rechts,
//! Legende unten links und das Achsenkreuz (Kompass) ganz unten links.

use crate::app::AppState;
use crate::camera::StandardView;
use solidcraft_render::Camera as RenderCamera;
use solidcore_record::Vec3 as RcVec3;

use eframe::egui::{self, Align2, FontId, Frame, Margin, Pos2, RichText, Ui, Vec2};

use super::theme;

/// Alle Ansichts-Buttons, in fester Reihenfolge.
/// Tastenkürzel je Standardansicht (None = nur Schaltfläche).
fn hotkey(view: StandardView) -> Option<egui::Key> {
    match view {
        StandardView::Front => Some(egui::Key::Num1),
        StandardView::Top => Some(egui::Key::Num2),
        StandardView::Right => Some(egui::Key::Num3),
        StandardView::Iso => Some(egui::Key::Num4),
        _ => None,
    }
}

/// Zeigt die schwebende Ansichten-Leiste (oben rechts).
pub(crate) fn toolbar(app: &mut AppState, ui: &mut Ui) {
    let active = app.active_view;
    Frame::new()
        .fill(theme::BG.gamma_multiply(0.55))
        .corner_radius(theme::CARD_ROUNDING as u8)
        .inner_margin(Margin::same(6))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for view in StandardView::ALL {
                    let label = view.label();
                    let mut tooltip = label.to_string();
                    if let Some(key) = hotkey(view) {
                        tooltip.push_str(&format!("  (Taste {key:?})"));
                    }
                    let selected = active == Some(view);
                    let text =
                        RichText::new(label).color(if selected { theme::ACCENT_TEXT } else { theme::TEXT });
                    let button = egui::Button::new(text)
                        .fill(if selected {
                            theme::ACCENT
                        } else {
                            theme::SURFACE
                        })
                        .corner_radius(theme::CARD_ROUNDING as u8);
                    if ui.add(button).on_hover_text(tooltip).clicked() {
                        app.camera.set_standard_view(view);
                        app.active_view = Some(view);
                    }
                }
                ui.separator();
                // Einpassen / Kamera zurücksetzen
                if ui
                    .button("⤢")
                    .on_hover_text("Ansicht einpassen  (Taste F)")
                    .clicked()
                {
                    app.camera.fit_view();
                    app.active_view = None;
                }
                if ui.button("⟲").on_hover_text("Kamera zurücksetzen (Iso)").clicked() {
                    app.camera_reset();
                }
            });
        })
        .response
        .on_hover_cursor(egui::CursorIcon::Default);
}

/// Legende unten links: orange = Rohteil, dunkel = Bauteil, rot = Z-Null.
pub(crate) fn legend(ui: &mut Ui) {
    Frame::new()
        .fill(theme::BG.gamma_multiply(0.55))
        .corner_radius(theme::CARD_ROUNDING as u8)
        .inner_margin(Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                let row = |ui: &mut Ui, color: eframe::egui::Color32, label: &str| {
                    ui.horizontal(|ui| {
                        let (_, response) =
                            ui.allocate_exact_size(Vec2::new(8.0, 12.0), egui::Sense::hover());
                        ui.painter().circle_filled(
                            Pos2::new(response.rect.center().x, response.rect.center().y),
                            3.0,
                            color,
                        );
                        ui.label(
                            RichText::new(label)
                                .size(theme::SIZE_HINT)
                                .color(theme::TEXT_MUTED),
                        );
                    });
                };
                row(ui, theme::LEGEND_STOCK, "Rohteil (Box)");
                row(ui, theme::LEGEND_PART, "Bauteil");
                row(ui, theme::LEGEND_CLAMP, "Spannfläche / Z-Null");
            });
        });
}

/// Achsen-Kompass unten links: Weltachsen auf die Basis der Renderkamera
/// projiziert (inklusive Roll — zeigt genau die Ansicht).
pub(crate) fn axes_gizmo(ui: &mut Ui, rect: egui::Rect, cam: &RenderCamera) {
    let (right, up, _back) = cam.basis();
    let anchor = Pos2::new(rect.left() + 54.0, rect.bottom() - 54.0);
    for (axis, color, label) in [
        (RcVec3::new(1.0, 0.0, 0.0), theme::AXIS_X, "X"),
        (RcVec3::new(0.0, 1.0, 0.0), theme::AXIS_Y, "Y"),
        (RcVec3::new(0.0, 0.0, 1.0), theme::AXIS_Z, "Z"),
    ] {
        // Bildschirm-x nach rechts (egui-y nach unten), Welt-y nach oben.
        // Skalarprodukte von Hand (RcVec3-`dot` hängt am Vec3Geom-Trait).
        let dx = axis.x * right.x + axis.y * right.y + axis.z * right.z;
        let dy = axis.x * up.x + axis.y * up.y + axis.z * up.z;
        let end = anchor + Vec2::new(dx as f32, -(dy as f32)) * 36.0;
        ui.painter()
            .line_segment([anchor, end], egui::Stroke::new(2.0_f32, color));
        ui.painter().text(
            Pos2::new(end.x + 4.0, end.y - 4.0),
            Align2::LEFT_BOTTOM,
            label,
            FontId::monospace(theme::SIZE_HINT),
            color,
        );
    }
    ui.painter()
        .circle_filled(anchor, 2.5, theme::AXIS_ORIGIN);
}

/// Fügt alle Viewport-Overlays in das 3D-Fenster ein (Kompass bleibt
/// unten links, Legende darüber, Leiste oben rechts).
pub(crate) fn show(app: &mut AppState, ui: &mut Ui, cam: &RenderCamera) {
    let rect = ui.max_rect();
    // Ansichten-Leiste oben rechts.
    egui::Area::new(egui::Id::new("views_toolbar"))
        .anchor(Align2::RIGHT_TOP, [-8.0, 8.0])
        .order(egui::Order::Middle)
        .show(ui.ctx(), |ui| toolbar(app, ui));
    // Legende über dem Kompass unten links.
    egui::Area::new(egui::Id::new("legend"))
        .anchor(Align2::LEFT_BOTTOM, [10.0, -108.0])
        .order(egui::Order::Middle)
        .show(ui.ctx(), legend);
    axes_gizmo(ui, rect, cam);
}

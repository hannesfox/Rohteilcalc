//! Die linke Leiste: Header mit Status-Chip, Karten (Datei, Ergebnis,
//! Maße, Aufmaße), einklappbare Sektionen (Schattenkontur, Darstellung,
//! Steuerung) und die fixe Fußleiste.

use crate::app::AppState;
use crate::renderer::ViewMode;
use crate::step::Quality;
use crate::stock::StockParams;

use eframe::egui::{self, Grid, RichText, Ui};

use super::{theme, widgets};

/// Inhalt der Leiste (innerhalb der ScrollArea).
pub(crate) fn content(app: &mut AppState, ui: &mut Ui) {
    theme::apply_text_styles(ui);

    header(app, ui);
    ui.add_space(theme::SPACING_CARD);
    widgets::card(ui, |ui| file_card(app, ui));
    ui.add_space(theme::SPACING_CARD);
    widgets::card(ui, |ui| result_card(app, ui));
    ui.add_space(theme::SPACING_CARD);
    widgets::card(ui, |ui| dims_card(app, ui));
    ui.add_space(theme::SPACING_CARD);
    widgets::card(ui, |ui| allowances_card(app, ui));
    ui.add_space(theme::SPACING_CARD);
    shadow_section(app, ui);
    ui.add_space(theme::SPACING_CARD);
    display_section(app, ui);
    ui.add_space(theme::SPACING_CARD);
    help_section(app, ui);
}

/// Fixe Fußleiste — außerhalb der ScrollArea, immer sichtbar.
pub(crate) fn footer(app: &mut AppState, ui: &mut Ui) {
    ui.separator();
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        let has_contour = app.shadow.is_some();
        let dxf = ui.add_enabled(has_contour, widgets::button_primary("💾 DXF exportieren"));
        if has_contour {
            if dxf.clicked() {
                app.save_shadow_dxf();
            }
        } else {
            dxf.on_disabled_hover_text(
                "Erst eine Schattenkontur erzeugen — siehe Sektion „Schattenkontur (DXF)“.",
            );
        }
        let copy = ui.add_enabled(
            app.model.is_some(),
            egui::Button::new("⧉ Kopieren"),
        );
        if app.model.is_some() {
            if copy.clicked() {
                if let Some(text) = result_string(app.model.as_deref()) {
                    ui.ctx().copy_text(text);
                }
            }
            copy.on_hover_text("Rohteilmaße in die Zwischenablage kopieren");
        } else {
            copy.on_disabled_hover_text("Kein Bauteil geladen");
        }
    });
}

// ---------------------------------------------------------
// Header mit Status-Chip
// ---------------------------------------------------------

fn header(app: &mut AppState, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("📦 CNC Rohteil")
                .strong()
                .size(theme::SIZE_TITLE),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            status_chip(app, ui);
        });
    });
    // Fehler direkt lesbar; Erfolgsmeldungen stecken im Chip-Tooltip.
    if app.status_is_error && !app.status.is_empty() {
        ui.label(
            RichText::new(&app.status)
                .size(theme::SIZE_HINT)
                .color(theme::ERROR),
        );
    }
    if app.loading.is_some() {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(12.0));
            widgets::hint(ui, "Lade …");
        });
    }
}

/// Status-Chip: „🧱 Demo" bzw. „📄 Dateiname"; die lange Meldung steckt im Tooltip.
fn status_chip(app: &mut AppState, ui: &mut Ui) {
    let label = app
        .file_path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| format!("📄 {}", n.to_string_lossy()))
        .unwrap_or_else(|| "🧱 Demo".to_string());
    let text = RichText::new(label)
        .size(theme::SIZE_HINT)
        .color(if app.status_is_error { theme::ERROR } else { theme::TEXT });
    let chip = egui::Button::new(text)
        .fill(if app.status_is_error {
            theme::ERROR.gamma_multiply(0.25)
        } else {
            theme::SURFACE
        })
        .corner_radius(theme::CARD_ROUNDING as u8);
    let response = ui.add(chip);
    if app.status.is_empty() {
        response.on_hover_text("Kein Bauteil geladen — Demo oder STEP-Datei öffnen.");
    } else {
        response.on_hover_text(&app.status);
    }
}

// ---------------------------------------------------------
// Karte „Datei"
// ---------------------------------------------------------

fn file_card(app: &mut AppState, ui: &mut Ui) {
    widgets::section_label(ui, "Datei");
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        if ui.button("📂 STEP öffnen…").clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("STEP CAD", &["step", "stp", "Step", "STP", "SteP"])
                .pick_file()
            {
                app.start_load(path, true);
            }
        }
        if ui.button("🧱 Demo").clicked() {
            app.load_demo();
        }
    });
    ui.horizontal(|ui| {
        ui.label("Qualität:");
        let before = app.quality;
        egui::ComboBox::from_id_salt("quality_combo")
            .selected_text(app.quality.label())
            .show_ui(ui, |ui| {
                for q in Quality::ALL {
                    ui.selectable_value(&mut app.quality, q, q.label());
                }
            });
        if app.quality != before {
            app.start_rettesellate();
        }
    });
    let path_text = app
        .file_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "Demo-Bauteil (110 × 42 × 18 mm)".to_string());
    widgets::hint(ui, &path_text);
}

// ---------------------------------------------------------
// Ergebnis-Karte (Akzent Orange) + Volumen
// ---------------------------------------------------------

fn result_card(app: &mut AppState, ui: &mut Ui) {
    let Some(model) = app.model.as_ref() else {
        widgets::hint(ui, "Kein Rohteil berechenbar — erst Bauteil laden.");
        return;
    };
    widgets::section_label(ui, "Rohteil");
    ui.label(
        RichText::new(
            result_string(app.model.as_deref()).unwrap_or_default(),
        )
        .monospace()
        .strong()
        .size(theme::SIZE_TITLE)
        .color(theme::ACCENT),
    );
    // Volumen/Materialverhältnis, falls das Bauteilvolumen berechenbar ist.
    let part_vol = model.volume_mm3;
    if part_vol > 0.0 {
        let stock_vol = f64::from(model.stock_size.x)
            * f64::from(model.stock_size.y)
            * f64::from(model.stock_size.z);
        widgets::hint(
            ui,
            &format!(
                "Bauteil {:.1} cm³ · Rohteil {:.1} cm³ · Material {:.0} %",
                part_vol / 1000.0,
                stock_vol / 1000.0,
                (part_vol / stock_vol * 100.0).min(100.0),
            ),
        );
    }
    ui.add_space(2.0);
    let copy_card = ui.button("⧉ Rohteil kopieren");
    if copy_card.clicked() {
        if let Some(text) = result_string(app.model.as_deref()) {
            ui.ctx().copy_text(text);
        }
    }
    copy_card.on_hover_text("Rohteilmaße in die Zwischenablage kopieren");
}

/// Rohteilmaße als kopierbare Zeile, z. B. „115.0 × 50.0 × 30.0 mm".
fn result_string(model: Option<&crate::renderer::ScModel>) -> Option<String> {
    let m = model?;
    Some(format!(
        "{:.1} × {:.1} × {:.1} mm",
        m.stock_size.x, m.stock_size.y, m.stock_size.z
    ))
}

// ---------------------------------------------------------
// Karte „Maße & Aufmaße" (Grid: Achse | Bauteil | Aufmaß | Rohteil)
// ---------------------------------------------------------

fn dims_card(app: &mut AppState, ui: &mut Ui) {
    widgets::section_label(ui, "Maße & Aufmaße");
    let Some(model) = app.model.as_ref() else {
        widgets::hint(ui, "Kein Bauteil geladen.");
        return;
    };
    let part = model.part_size;
    let stock = model.stock_size;

    ui.add_space(2.0);
    Grid::new("dims_grid")
        .num_columns(4)
        .spacing([10.0, 3.0])
        .show(ui, |ui| {
            let head = |ui: &mut Ui, t: &str| {
                ui.label(
                    RichText::new(t)
                        .size(theme::SIZE_HINT)
                        .color(theme::TEXT_MUTED),
                );
            };
            head(ui, "Achse");
            head(ui, "Bauteil");
            head(ui, "Aufmaß");
            head(ui, "Rohteil (mm)");
            ui.end_row();

            let axis_x = ui.label("X");
            widgets::mono_right(ui, &format!("{:.2}", part.x));
            widgets::mono_right(ui, &format!("+{:.1}", stock.x - part.x));
            widgets::mono_right(ui, &format!("{:.2}", stock.x));
            ui.end_row();
            axis_x.on_hover_text("X: Sägeaufmaß — einstellbar in der Karte „Aufmaße & Spannung“.");

            let axis_y = ui.label("Y");
            widgets::mono_right(ui, &format!("{:.2}", part.y));
            widgets::mono_right(ui, "–");
            widgets::mono_right(ui, &format!("{:.2}", stock.y));
            ui.end_row();
            axis_y.on_hover_text(
                "Y: Halbzeug-Staffel — bis 25 mm in 5er-, ab 30 mm in 10er-Schritten (je +1 mm Mindestspiel).",
            );

            let axis_z = ui.label("Z");
            widgets::mono_right(
                ui,
                &format!("{:.2}", part.z),
            );
            widgets::mono_right(
                ui,
                &format!(
                    "+{:.1} unten / +{:.1} oben",
                    model.clamp_mm, model.top_allow_mm
                ),
            );
            widgets::mono_right(ui, &format!("{:.2}", stock.z));
            ui.end_row();
            axis_z.on_hover_text(
                "Z: Rohteilplatten in 10-mm-Stufen — Spanntiefe unten, Planfräs-Aufmaß oben (unten einstellbar).",
            );
        });
}

// ---------------------------------------------------------
// Karte „Aufmaße & Spannung" (editierbar)
// ---------------------------------------------------------

fn allowances_card(app: &mut AppState, ui: &mut Ui) {
    ui.horizontal(|ui| {
        widgets::section_label(ui, "Aufmaße & Spannung");
        widgets::info_icon(
            ui,
            "Änderungen berechnen das Rohteil sofort neu:\n\
             Sägeaufmaß: Zulängen auf X.\n\
             Spanntiefe: Schraubstock-Zugriff von unten.\n\
             Planfräs: Aufmaß oben (Plattenstärke rundet auf 10 mm).",
        );
    });
    ui.add_space(2.0);
    let mut changed = false;
    drag_row(ui, "Sägeaufmaß (X):", &mut app.allowances.saw_x_mm, 0.0..=20.0, &mut changed);
    drag_row(
        ui,
        "Spanntiefe (Z unten):",
        &mut app.allowances.clamp_z_mm,
        0.0..=20.0,
        &mut changed,
    );
    drag_row(
        ui,
        "Planfräs (Z oben):",
        &mut app.allowances.plan_top_mm,
        0.0..=20.0,
        &mut changed,
    );
    ui.horizontal(|ui| {
        if ui.button("↺ Zurücksetzen").clicked() {
            app.allowances = StockParams::default();
            app.rebuild();
        }
    });
    if changed {
        app.rebuild(); // Rohteil mit den neuen Aufmaßen neu berechnen
    }
}

fn drag_row(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    changed: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        let before = *value;
        ui.add(
            egui::DragValue::new(value)
                .speed(0.5)
                .range(range)
                .suffix(" mm"),
        );
        if *value != before {
            *changed = true;
        }
    });
}

// ---------------------------------------------------------
// Einklappbare Sektionen
// ---------------------------------------------------------

fn shadow_section(app: &mut AppState, ui: &mut Ui) {
    egui::CollapsingHeader::new("🎯 Schattenkontur (DXF)")
        .id_salt("sidebar_shadow")
        .default_open(false)
        .show(ui, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label("Abstand:");
                ui.add_enabled(
                    app.model.is_some(),
                    egui::DragValue::new(&mut app.shadow_expand)
                        .speed(0.5)
                        .range(0.0..=50.0)
                        .suffix(" mm"),
                );
                if ui
                    .add_enabled(
                        app.model.is_some(),
                        egui::Button::new("Kontur erzeugen"),
                    )
                    .clicked()
                {
                    app.build_shadow();
                }
            });
            ui.horizontal(|ui| {
                widgets::info_icon(
                    ui,
                    "Projektion des Bauteils auf die Grundfläche (Schatten von oben):\n\
                     nur die Außenkontur, nach außen parallel um den Abstand vergrößert.\n\
                     Ursprung der DXF: untere linke Ecke des Bauteils (0,0), Einheit mm.",
                );
                shadow_status_line(app, ui);
            });
            if let Some(s) = app.shadow.clone() {
                if (s.expand - app.shadow_expand).abs() > 1e-4 {
                    ui.label(
                        RichText::new("Abstand geändert — bitte neu erzeugen.")
                            .size(theme::SIZE_HINT)
                            .color(theme::WARN),
                    );
                }
                widgets::hint(
                    ui,
                    &format!(
                        "Umfang {:.1} mm · Fläche {:.0} mm² · Genauigkeit {:.2} mm",
                        s.perimeter, s.area, s.res
                    ),
                );
                ui.horizontal(|ui| {
                    if widgets::toggle_switch(ui, &mut app.shadow_preview).changed() {
                        app.refresh_preview();
                    }
                    ui.label("Vorschau in der 3D-Ansicht");
                });
            }
        });
}

/// Betreffzeile der Schattenkontur: grau (nie erzeugt), grün (erzeugt) oder
/// gelb (Abstand geändert).
fn shadow_status_line(app: &mut AppState, ui: &mut Ui) {
    match app.shadow.clone() {
        Some(s) if (s.expand - app.shadow_expand).abs() <= 1e-4 => {
            widgets::status_dot(ui, theme::OK);
            widgets::hint(ui, &format!("Kontur erzeugt – {} Punkte", s.polygon.len()));
        }
        Some(_) => {
            widgets::status_dot(ui, theme::WARN);
            widgets::hint(ui, "Abstand geändert");
        }
        None => {
            widgets::status_dot(ui, theme::TEXT_MUTED);
            widgets::hint(ui, "Noch keine Kontur");
        }
    }
}

fn display_section(app: &mut AppState, ui: &mut Ui) {
    egui::CollapsingHeader::new("🎨 Darstellung")
        .id_salt("sidebar_display")
        .default_open(false)
        .show(ui, |ui| {
            ui.add_space(2.0);
            let modes: Vec<(ViewMode, &'static str)> = ViewMode::ALL
                .iter()
                .map(|m| (*m, m.label()))
                .collect();
            if widgets::segmented(ui, &mut app.mode, &modes) {
                app.rebuild();
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if widgets::toggle_switch(ui, &mut app.show_stock).changed() {
                    app.rebuild();
                }
                ui.label("Rohteil-Box anzeigen");
            });

            // Einzelne Körper ein-/ausblenden (nur bei STEP mit mehreren).
            let bodies = app
                .model
                .as_ref()
                .filter(|m| m.bodies.len() > 1)
                .map(|m| m.bodies.clone());
            if let Some(bodies) = bodies {
                ui.add_space(4.0);
                widgets::hint(ui, "Körper:");
                for body in &bodies {
                    let mut visible = body.visible;
                    let name = body.name.clone();
                    ui.horizontal(|ui| {
                        if widgets::toggle_switch(ui, &mut visible).changed() {
                            app.set_body_visible(&name, visible);
                        }
                        ui.label(name);
                    });
                }
            }
        });
}

fn help_section(app: &mut AppState, ui: &mut Ui) {
    egui::CollapsingHeader::new("ℹ Steuerung & Info")
        .id_salt("sidebar_help")
        .default_open(false)
        .show(ui, |ui| {
            ui.add_space(2.0);
            Grid::new("help_grid")
                .num_columns(2)
                .spacing([10.0, 3.0])
                .show(ui, |ui| {
                    let row = |ui: &mut Ui, action: &str, keys: &str| {
                        widgets::hint(ui, action);
                        ui.label(
                            RichText::new(keys)
                                .monospace()
                                .size(theme::SIZE_HINT)
                                .color(theme::TEXT),
                        );
                        ui.end_row();
                    };
                    row(ui, "Drehen", "Linke Maustaste");
                    row(ui, "Verschieben", "Rechte Maustaste / Shift");
                    row(ui, "Zoomen", "Mausrad");
                    row(ui, "Einpassen", "F / Doppelklick");
                    row(ui, "Ansichten", "1 Vorne · 2 Oben · 3 Rechts · 4 Iso");
                    row(ui, "Laden", "STEP-Datei ins Fenster ziehen");
                });
            if let Some(model) = app.model.as_ref() {
                ui.add_space(2.0);
                widgets::hint(
                    ui,
                    &format!("Kanten {} · Dreiecke {}", model.edge_count, model.triangle_count),
                );
            }
        });
}

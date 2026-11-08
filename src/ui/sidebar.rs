//! Die linke Leiste: Header mit Status-Chip, Karten (Datei, Ergebnis,
//! Maße, Aufmaße), einklappbare Sektionen (Schattenkontur, Darstellung,
//! Steuerung) und die fixe Fußleiste.

use crate::app::{AppState, DxfVariant};
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
    widgets::card(ui, |ui| alignment_card(app, ui));
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
        if copy.clicked() {
            if let Some(text) = copy_string(app.model.as_deref()) {
                ui.ctx().copy_text(text);
            }
        }
        match app
            .model
            .as_deref()
            .map(|m| m.round)
        {
            Some(true) => {
                copy.on_hover_text("Rohteilmaße als „Ø38*32“ in die Zwischenablage kopieren")
            }
            _ => copy.on_hover_text("Rohteilmaße als „40*30*41“ in die Zwischenablage kopieren"),
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
        // Rundmaterial: Zylindervolumen π/4 · Ø² · Länge statt Box.
        let stock_vol = if model.round {
            let axial = if model.round_axis == Some(2) {
                f64::from(model.stock_size.z)
            } else {
                f64::from(model.stock_size.x)
            };
            let d = f64::from(model.stock_size.y);
            std::f64::consts::FRAC_PI_4 * d * d * axial
        } else {
            f64::from(model.stock_size.x)
                * f64::from(model.stock_size.y)
                * f64::from(model.stock_size.z)
        };
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
        if let Some(text) = copy_string(app.model.as_deref()) {
            ui.ctx().copy_text(text);
        }
    }
    copy_card.on_hover_text(if app
        .model
        .as_deref()
        .is_some_and(|m| m.round)
    {
        "Rohteilmaße als „Ø38*32“ in die Zwischenablage kopieren"
    } else {
        "Rohteilmaße als „40*30*41“ in die Zwischenablage kopieren"
    });
}

/// Maschinenformat für die Zwischenablage: Y*Z*X gerundet mit `*` als
/// Trenner, z. B. „40*30*41“ für die Anzeige „41 × 40 × 30 mm“.
/// Rundmaterial: „Ø38*32“ (Ø immer in Y; axial Z bei stehender Scheibe,
/// sonst X).
fn copy_string(model: Option<&crate::renderer::ScModel>) -> Option<String> {
    let m = model?;
    if m.round {
        let axial = if m.round_axis == Some(2) {
            m.stock_size.z
        } else {
            m.stock_size.x
        };
        return Some(format!("Ø{:.0}*{:.0}", m.stock_size.y, axial));
    }
    Some(format!(
        "{:.0}*{:.0}*{:.0}",
        m.stock_size.y, m.stock_size.z, m.stock_size.x
    ))
}

/// Rohteilmaße als kopierbare Zeile — Rechteck „115.0 × 50.0 × 30.0 mm",
/// Rundmaterial „Ø 55,0 × 125,0 mm" (Ø vor der Länge).
fn result_string(model: Option<&crate::renderer::ScModel>) -> Option<String> {
    let m = model?;
    if m.round {
        let axial = if m.round_axis == Some(2) {
            m.stock_size.z
        } else {
            m.stock_size.x
        };
        return Some(format!("Ø {:.1} × {:.1} mm", m.stock_size.y, axial));
    }
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

    if model.round {
        // Rundmaterial: Ø-Zeile (stets Y) und Längen-Zeile (Z bei stehender
        // Scheibe, sonst X) statt der Rechteck-Zeilen.
        widgets::hint(ui, &format!("Rundmaterial: Ø {:.1} mm — Katalogrundung unter 30 mm in 2er-, ab 30 mm in 5er-Schritten; kein Spann-/Planfräsmaß.", stock.y));
    }
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

            if model.round {
                let axial = if model.round_axis == Some(2) {
                    ("Länge (Z)", part.z, stock.z, "Z: Bauteillänge + Sägeaufmaß — der Stab läuft axial ins Spannfutter (kein Spann-/Planaufmaß).")
                } else {
                    ("Länge (X)", part.x, stock.x, "X: Bauteillänge + Sägeaufmaß — der Stab läuft axial ins Spannfutter (kein Spann-/Planaufmaß).")
                };
                let dia_parts = part.y.max(part.z);
                let axis_d = ui.label("Ø");
                widgets::mono_right(ui, &format!("{:.2}", dia_parts));
                widgets::mono_right(ui, &format!("+{:.1}", stock.y - dia_parts));
                widgets::mono_right(ui, &format!("{:.2}", stock.y));
                ui.end_row();
                axis_d.on_hover_text("Ø: Bauteil-Ø + Ø-Aufmaß, gerundet auf Katalogstufen — Ø-Aufmaß einstellbar in der Karte „Aufmaße & Spannung“.");
                let axis_len = ui.label(axial.0);
                widgets::mono_right(ui, &format!("{:.2}", axial.1));
                widgets::mono_right(ui, &format!("+{:.1}", axial.2 - axial.1));
                widgets::mono_right(ui, &format!("{:.2}", axial.2));
                ui.end_row();
                axis_len.on_hover_text(axial.3);
                return;
            }

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
                "Z: Platte bis 30 mm in 5-mm-, darüber in 10-mm-Stufen — Spanntiefe unten, Planfräs-Aufmaß oben (unten einstellbar).",
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
             Materialform: Auto erkennt rund, „Rund“/„Rechteck“ übersteuert.\n\
             Sägeaufmaß: Zulängen auf X (rund zusätzlich Ø-Aufmaß).\n\
             Spanntiefe: Schraubstock-Zugriff von unten (rund: 0, Stab axial).\n\
             Planfräs: Aufmaß oben (Platte rundet bis 30 mm auf 5 mm, darüber auf 10 mm).",
        );
    });
    ui.add_space(2.0);
    // Materialform: automatisch (Erkennung kreisförmiger Querschnitt) oder
    // als Rundmaterial/Rechteck übersteuern.
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Materialform:");
        if widgets::segmented(
            ui,
            &mut app.allowances.round_stock,
            &[
                (None, "Auto"),
                (Some(true), "Rund"),
                (Some(false), "Rechteck"),
            ],
        ) {
            app.rebuild(); // Rohteil in der neuen Materialform neu berechnen
        }
    });
    let form_hint = match app.allowances.round_stock {
        None => "Auto: kreisförmiger Querschnitt wird als Rundmaterial erkannt (liegender\nStab entlang X oder stehende Scheibe entlang Z).",
        Some(true) => "Rundmaterial: Rohteil ist ein Rundstab (Ø gerundet auf Katalogstufen).",
        Some(false) => "Rechteckplatte wie gehabt (Y-Staffel, Z-Plattenstufen).",
    };
    widgets::hint(ui, form_hint);
    drag_row(
        ui,
        "Sägeaufmaß (X):",
        &mut app.allowances.saw_x_mm,
        0.0..=20.0,
        &mut changed,
    );
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
    if app.allowances.round_stock == Some(true) {
        drag_row(
            ui,
            "Ø-Aufmaß:",
            &mut app.allowances.round_add_mm,
            0.0..=20.0,
            &mut changed,
        );
    }
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

/// Ausrichtung des Bauteils: automatisch oder über Fläche „unten“.
fn alignment_card(app: &mut AppState, ui: &mut Ui) {
    widgets::section_label(ui, "Ausrichtung");
    let status = if app.align_down.is_some() {
        "Manuell: gewählte Fläche unten"
    } else {
        "Automatisch (längste Achse → X)"
    };
    widgets::hint(ui, status);
    ui.horizontal(|ui| {
        let label = if app.align_picking {
            "⤾ Abbrechen"
        } else {
            "🎯 Neu ausrichten"
        };
        let pick = ui.add_enabled(app.model.is_some(), egui::Button::new(label));
        if pick.clicked() {
            app.toggle_align_picking();
        }
        pick.on_hover_text(
            "Fläche im 3D-Fenster anklicken, die unten liegen soll (z. B. eine\n\
             andere Ansatzfläche für die Aufspannung) — Rohteil wird neu\n\
             ausgerichtet und berechnet. Esc bricht ab.",
        );
        if app.align_down.is_some() {
            let auto = ui.button("Automatisch");
            if auto.clicked() {
                app.reset_alignment();
            }
            auto.on_hover_text("Zurück zur automatischen Ausrichtung (längste Achse → X)");
        }
    });
    if app.align_picking {
        ui.label(
            RichText::new("Fläche im 3D-Fenster anklicken …")
                .size(theme::SIZE_HINT)
                .color(theme::ACCENT_TEXT),
        );
    }
}

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
            // Dateiname des DXF-Exports: Stammname der STEP-Datei plus
            // Verfahrenskürzel.
            ui.horizontal(|ui| {
                ui.label("Verfahren:");
                widgets::segmented(
                    ui,
                    &mut app.dxf_variant,
                    &DxfVariant::ALL.iter().map(|v| (*v, v.label())).collect::<Vec<_>>(),
                );
            });
            let stem = app
                .file_path
                .as_ref()
                .and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "schattenkontur".to_string());
            widgets::hint(
                ui,
                &format!("Export-Name: {stem}_{}.dxf", app.dxf_variant.suffix()),
            );
            ui.horizontal(|ui| {
                ui.label("Text:");
                ui.label("S=");
                let edit = ui.add_enabled(
                    app.model.is_some(),
                    egui::TextEdit::singleline(&mut app.dxf_label_value)
                        .desired_width(52.0)
                        .hint_text("70"),
                );
                edit.on_hover_text(
                    "Nur die Zahl ist änderbar (z. B. 70 oder 70.5).\n\
                     In DXF/Ansicht wird daraus „S=<Zahl>mm“. Vorschlag aus Rohteil-Z-Höhe.",
                );
                ui.label("mm");
                if ui
                    .add_enabled(app.model.is_some(), egui::Button::new("↺"))
                    .on_hover_text("Zahl aus aktueller Rohteil-Z-Höhe übernehmen")
                    .clicked()
                {
                    app.propose_dxf_label();
                }
            });
            ui.horizontal(|ui| {
                widgets::info_icon(
                    ui,
                    "Projektion des Bauteils auf die Grundfläche (Schatten von oben):\n\
                     nur die Außenkontur, nach außen parallel um den Abstand vergrößert.\n\
                     Ursprung der DXF: Bauteilmitte (X/Y symmetrisch um 0,0), Einheit mm.",
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

                ui.add_space(4.0);
                widgets::section_label(ui, "Innenkonturen (DXF)");
                widgets::hint(
                    ui,
                    "Automatisch alle Innenkonturen erkennen (nur ≥ 40×40 mm\n\
                     oder Ø ≥ 40 mm), oder per Flächenklick hinzufügen — auch\n\
                     Nut-/Wandflächen: es wird die Kontur der Öffnung genommen,\n\
                     die der Klickstelle am nächsten liegt.",
                );
                ui.horizontal(|ui| {
                    ui.label("Versatz nach innen:");
                    ui.add(
                        egui::DragValue::new(&mut app.inner_offset_mm)
                            .speed(0.1)
                            .range(0.0..=20.0)
                            .suffix(" mm"),
                    );
                });
                ui.horizontal(|ui| {
                    if ui.button("Alle erkennen (≥40 mm)").clicked() {
                        app.auto_detect_inner_contours();
                    }
                    let label = if app.inner_pick_active {
                        "Abbrechen"
                    } else {
                        "Fläche wählen"
                    };
                    if ui.button(label).clicked() {
                        app.toggle_inner_pick();
                    }
                    let n = s.inner_polygons.len();
                    if n > 0 {
                        if ui.button("Löschen").clicked() {
                            app.clear_inner_contours();
                        }
                    }
                });
                {
                    let n = s.inner_polygons.len();
                    if n > 0 {
                        widgets::hint(ui, &format!("{n} Innenkontur(en) übernommen"));
                    }
                }
                if app.inner_pick_active {
                    ui.label(
                        RichText::new("3D-Ansicht: Fläche anklicken … (Esc bricht ab)")
                            .size(theme::SIZE_HINT)
                            .color(theme::ACCENT_TEXT),
                    );
                }
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
                ui.label("Projektion:");
                ui.add_space(4.0);
                let was = app.camera.perspective;
                if widgets::segmented(
                    ui,
                    &mut app.camera.perspective,
                    &[(true, "Perspektivisch"), (false, "Orthografisch")],
                ) {
                    app.camera.projection_adjust_distance(was);
                }
            });
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
                    row(ui, "Projektion", "Persp / Ortho in der Ansichtsleiste");
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

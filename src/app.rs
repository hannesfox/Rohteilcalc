//! Anwendungsoberfläche: Steuerpanel, Viewport-Interaktion und
//! asynchrones Laden/Tessellieren (Import via SolidCraft).
use crate::camera::{OrbitCamera, StandardView};
use crate::mesh::generate_sample_milled_part;
use crate::renderer::{
    ScModel, ScSession, SceneState, ViewCallback, ViewMode, demo_scene, make_frame,
    render_camera,
};
use crate::step::Quality;
use crate::space_mouse;
use eframe::egui::{self, Color32, Pos2, RichText, Sense, Stroke, Vec2};
use solidcore_record::Vec3 as RcVec3;
use solidcraft_geom::prelude::*;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

struct LoadOk {
    session: Option<ScSession>,
    model: ScModel,
    quality: Quality,
    path: Option<PathBuf>,
    warnings: usize,
}

struct LoadMsg {
    result: Result<LoadOk, String>,
    fit_camera: bool,
}

pub struct AppState {
    camera: OrbitCamera,
    mode: ViewMode,
    show_stock: bool,
    quality: Quality,
    session: Option<ScSession>,
    model: Option<Arc<ScModel>>,
    gpustate: Option<SceneState>,
    revision: u64,
    file_path: Option<PathBuf>,
    status: String,
    status_is_error: bool,
    loading: Option<Receiver<LoadMsg>>,
    /// 3D-Maus (SpaceMouse): Leser-Thread + geteilte Achsen; `None`, wenn
    /// kein Gerät gelesen werden kann oder die app noch nicht begonnen hat.
    spacemouse: Option<space_mouse::SpaceMouse>,
}

impl AppState {
    pub fn new(_cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        let mut app = Self {
            camera: OrbitCamera::default(),
            mode: ViewMode::ShadedEdges,
            show_stock: true,
            quality: Quality::Medium,
            session: None,
            model: None,
            gpustate: None,
            revision: 0,
            file_path: None,
            status: String::new(),
            status_is_error: false,
            loading: None,
            spacemouse: None,
        };
        // Pipelines wurden in main (renderer::install) angelegt; egal wer
        // zuerst — hier fehlt nur der scene upload.
        app.load_demo();
        if let Some(path) = initial {
            app.start_load(path, true);
        }
        app
    }

    fn reset_camera(&mut self) {
        if let Some(model) = &self.model {
            self.camera.fit(model.center, model.radius);
        }
    }

    fn load_demo(&mut self) {
        let mesh = generate_sample_milled_part(110.0, 42.0, 18.0);
        match demo_scene(mesh, self.mode, self.show_stock) {
            Ok(model) => {
                self.session = None;
                self.file_path = None;
                self.set_model(model);
                self.status =
                    "Demo-Bauteil aktiv – STEP-Datei über „Öffnen“ laden oder ins Fenster ziehen."
                        .to_string();
                self.status_is_error = false;
            }
            Err(e) => {
                self.status = format!("Demo-Bauteil: {e}");
                self.status_is_error = true;
            }
        }
        self.reset_camera();
    }

    fn set_model(&mut self, model: ScModel) {
        self.revision += 1;
        self.model = Some(Arc::new(model));
        self.gpustate = None; // Neu-Upload beim nächsten Bild
    }

    /// Lädt eine Datei im Hintergrund, damit die Oberfläche nicht blockiert.
    fn start_load(&mut self, path: PathBuf, fit_camera: bool) {
        if self.loading.is_some() {
            return;
        }
        let quality = self.quality;
        let mode = self.mode;
        let show_stock = self.show_stock;
        let (tx, rx) = mpsc::channel();
        let path_clone = path.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let session = ScSession::open(&path_clone, quality)?;
                let model = session.model(mode, show_stock)?;
                Ok(LoadOk {
                    session: Some(session),
                    model,
                    quality,
                    path: Some(path_clone),
                    warnings: 0,
                })
            }))
            .unwrap_or_else(|_| {
                Err("Beim Verarbeiten der STEP-Geometrie ist ein interner Fehler aufgetreten."
                    .to_string())
            });
            let _ = tx.send(LoadMsg { result, fit_camera });
        });
        self.loading = Some(rx);
        self.status = format!(
            "Lade {} … (Qualität: {})",
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            quality.label()
        );
        self.status_is_error = false;
    }

    /// Öffnet das bereits geladene Dokument mit einer neuen Qualität neu.
    fn start_rettesellate(&mut self) {
        let Some(path) = self.file_path.clone() else {
            return;
        };
        self.start_load(path, false);
    }

    fn poll_loading(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.loading else {
            return;
        };
        match rx.try_recv() {
            Ok(msg) => {
                self.loading = None;
                match msg.result {
                    Ok(ok) => {
                        self.quality = ok.quality;
                        self.file_path = ok.path.clone();
                        self.session = ok.session;
                        self.set_model(ok.model);
                        if msg.fit_camera {
                            self.reset_camera();
                        }
                        if let Some(model) = &self.model {
                            let name = self
                                .file_path
                                .as_ref()
                                .and_then(|p| p.file_name())
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "STEP".to_string());
                            let mut status = format!(
                                "{name}: {} Dreiecke, Bauteil {:.1} × {:.1} × {:.1} mm",
                                model.triangle_count,
                                model.part_size.x,
                                model.part_size.y,
                                model.part_size.z
                            );
                            if ok.warnings > 0 {
                                status.push_str(&format!(" — {} Warnung(en)", ok.warnings));
                            }
                            self.status = status;
                            self.status_is_error = false;
                        }
                    }
                    Err(err) => {
                        self.status = format!("STEP konnte nicht geladen werden: {err}");
                        self.status_is_error = true;
                    }
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.loading = None;
                self.status = "Ladevorgang abgebrochen.".to_string();
                self.status_is_error = true;
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
        }
    }

    fn ui_panel(&mut self, ui: &mut egui::Ui) {
        // Große, gut lesbare Schrift in der Steuerleiste (wie bisher).
        for font_id in ui.style_mut().text_styles.values_mut() {
            font_id.size *= 2.0;
        }

        ui.heading("📦 Rohteil-Kalkulation (CNC)");
        ui.separator();

        ui.horizontal(|ui| {
            if ui.button("📂 STEP-Datei öffnen…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("STEP CAD", &["step", "stp", "Step", "STP", "SteP"])
                    .pick_file()
                {
                    self.start_load(path, true);
                }
            }
            if ui.button("🧱 Demo").clicked() {
                self.load_demo();
            }
        });

        ui.horizontal(|ui| {
            ui.label("Qualität:");
            let before = self.quality;
            egui::ComboBox::from_id_salt("tess_quality")
                .selected_text(self.quality.label())
                .show_ui(ui, |ui| {
                    for q in Quality::ALL {
                        ui.selectable_value(&mut self.quality, q, q.label());
                    }
                });
            if self.quality != before {
                self.start_rettesellate();
            }
        });

        let file_label = self
            .file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Demo".to_string());
        ui.label(format!("Datei: {file_label}"));

        if !self.status.is_empty() {
            let color = if self.status_is_error {
                Color32::from_rgb(220, 90, 80)
            } else if self.loading.is_some() {
                Color32::from_rgb(210, 160, 60)
            } else {
                Color32::from_rgb(50, 130, 80)
            };
            ui.label(RichText::new(&self.status).color(color));
        }
        if self.loading.is_some() {
            ui.add(egui::Spinner::new().size(16.0));
        }

        ui.separator();
        ui.label(RichText::new("1. Bauteilmaße (ausgerichtet)").strong());
        if let Some(model) = &self.model {
            ui.label(format!("X (Länge):  {:.2} mm", model.part_size.x));
            ui.label(format!("Y (Breite): {:.2} mm", model.part_size.y));
            ui.label(format!("Z (Höhe):   {:.2} mm", model.part_size.z));

            ui.add_space(8.0);
            ui.label(
                RichText::new("2. Berechnetes Rohteil")
                    .strong()
                    .color(Color32::from_rgb(180, 130, 20)),
            );
            ui.label(format!(
                "X-Rohmaß: {:.2} mm (+5.0 mm Sägeaufmaß)",
                model.stock_size.x
            ));
            ui.label(format!(
                "Y-Rohmaß: {:.2} mm (Halbzeug-Staffel)",
                model.stock_size.y
            ));
            ui.label(format!(
                "Z-Rohmaß: {:.2} mm (10-mm-Stufen)",
                model.stock_size.z
            ));
        }

        ui.add_space(8.0);
        ui.label(RichText::new("3. Z-Spannsituation").strong());
        ui.label("• Schraubstock-Spanntiefe: 4.0 mm (unten)");
        if let Some(model) = &self.model {
            ui.label(format!(
                "• Planfräs-Aufmaß:         {:.1} mm (oben)",
                (model.stock_size.z - model.part_size.z - 4.0).max(1.0)
            ));
        }

        ui.separator();
        ui.label(RichText::new("Darstellung").strong());
        ui.horizontal(|ui| {
            ui.label("Modus:");
            let before = self.mode;
            egui::ComboBox::from_id_salt("view_mode")
                .selected_text(self.mode.label())
                .show_ui(ui, |ui| {
                    for m in ViewMode::ALL {
                        ui.selectable_value(&mut self.mode, m, m.label());
                    }
                });
            if self.mode != before {
                self.rebuild();
            }
        });
        let before = self.show_stock;
        ui.checkbox(&mut self.show_stock, "Rohteil-Box anzeigen");
        if self.show_stock != before {
            self.rebuild();
        }

        ui.add_space(6.0);
        ui.label("Ansicht:");
        ui.horizontal_wrapped(|ui| {
            for view in StandardView::ALL {
                if ui.small_button(view.label()).clicked() {
                    self.camera.set_standard_view(view);
                }
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Ansicht einpassen").clicked() {
                self.camera.fit_view();
            }
            if ui.button("Kamera zurücksetzen").clicked() {
                self.camera = OrbitCamera::default();
                self.camera.set_standard_view(StandardView::Iso);
                self.reset_camera();
            }
        });

        if let Some(model) = self.model.clone() {
            if model.bodies.len() > 1 {
                ui.add_space(6.0);
                ui.label("Körper:");
                for body in &model.bodies {
                    let mut visible = body.visible;
                    let name = body.name.clone();
                    if ui.checkbox(&mut visible, &name).changed() {
                        self.set_body_visible(&name, visible);
                    }
                }
            }
        }

        ui.separator();
        ui.label(RichText::new("Steuerung Viewport").strong());
        ui.label("• Linke Maustaste: Drehen");
        ui.label("• Rechte Maustaste / Shift: Verschieben");
        ui.label("• Mausrad: Zoomen");
        ui.label("• Doppelklick: Einpassen");
        ui.label("• STEP-Datei ins Fenster ziehen: Laden");

        if let Some(model) = &self.model {
            ui.add_space(4.0);
            ui.label(format!("Kanten: {}", model.edge_count));
            ui.label(format!(
                "GPU-Modus: SolidCraft-Pipelines ({})",
                model.triangle_count
            ));
        }
    }

    /// Szene mit neuen Anzeigeeinstellungen synchron neu bauen.
    fn rebuild(&mut self) {
        if let Some(session) = &self.session {
            match session.model(self.mode, self.show_stock) {
                Ok(model) => self.set_model(model),
                Err(e) => {
                    self.status = format!("Szene neu bauen: {e}");
                    self.status_is_error = true;
                }
            }
        } else {
            // Demo-Darstellung wiederherstellen (ohne die Rohteil-Rechnung
            // erneut zu durchlaufen — Größe bleibt gleich).
            self.load_demo_preserve_camera();
        }
    }

    fn load_demo_preserve_camera(&mut self) {
        let mesh = generate_sample_milled_part(110.0, 42.0, 18.0);
        if let Ok(model) = demo_scene(mesh, self.mode, self.show_stock) {
            self.session = None;
            self.file_path = None;
            self.set_model(model);
        }
    }

    fn set_body_visible(&mut self, name: &str, visible: bool) {
        let Some(session) = &mut self.session else {
            return;
        };
        if let Err(e) = session.set_visible(name, visible) {
            self.status = e;
            self.status_is_error = true;
            return;
        }
        if let Some(session) = &self.session {
            match session.model(self.mode, self.show_stock) {
                Ok(model) => self.set_model(model),
                Err(e) => {
                    self.status = format!("Szene neu bauen: {e}");
                    self.status_is_error = true;
                }
            }
        }
    }

    fn viewport(&mut self, ui: &mut egui::Ui) {
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());

        let moving = response.dragged() || ui.input(|i| i.smooth_scroll_delta.y != 0.0);
        let shift = ui.input(|i| i.modifiers.shift);
        let pan_btn = response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle);

        // 3D-Maus: ein Frame Bewegung, wenn der Deckel gedrückt ist — die
        // Maßtabelle wie in SolidCrafts device_camera (Pan 0.6 Höhen/s,
        // Zoom exp(1.5)/s, Orbit 250 px/s, Roll 2 rad/s), der Drehtisch
        // über orbit_on_screen, so dass er trotz Kantung in Bildschirm-
        // richtung lenkt.
        if let Some(m) = self.spacemouse.as_mut().and_then(|sm| sm.tick(ui.ctx(), rect.height())) {
            self.camera.pan(m.pan_x, m.pan_y, rect.height());
            // m.zoom trägt die Framezeit bereits (Schub·step); SolidCraft
            // rechnet identisch exp(−1.5·Schub·seconds) — hier also nicht
            // noch einmal mit der Zeit multiplizieren.
            self.camera.zoom_factor((-f64::from(m.zoom) * 1.5).exp() as f32);
            self.camera.orbit_on_screen(m.orbit_x, m.orbit_y);
            self.camera.roll_by(m.roll);
        }
        if response.dragged() {
            let delta = response.drag_delta();
            if shift || pan_btn {
                self.camera.pan(delta.x, delta.y, rect.height());
            } else {
                self.camera.orbit(delta.x, delta.y);
            }
        }
        if response.double_clicked() {
            self.camera.fit_view();
        }

        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        let wheel_is_user = self.spacemouse.as_ref().is_none_or(|sm| sm.wheel_is_user());
        if scroll.abs() > 0.0 && response.hovered() && wheel_is_user {
            self.camera.zoom(scroll);
        }

        let Some(model) = self.model.clone() else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Keine Geometrie geladen",
                egui::FontId::proportional(18.0),
                Color32::GRAY,
            );
            return;
        };

        // GPU-Upload je Revision anlegen (Szene → GPU beim ersten Bild).
        let revision = self.revision;
        let state = match &self.gpustate {
            Some(st) if st.key == revision => st,
            _ => {
                let mut st = SceneState::new(&model.scene);
                st.set_key(revision);
                self.gpustate = Some(st);
                self.gpustate.as_mut().unwrap()
            }
        };

        let cam = render_camera(&self.camera);
        let frame = make_frame(
            state,
            &model,
            &cam,
            [rect.width() * ui.ctx().pixels_per_point(), rect.height() * ui.ctx().pixels_per_point()],
            moving,
        );

        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            ViewCallback(frame),
        ));

        // Achsen-Gizmo (2D-Overlay): die Weltachsen auf die Basis der
        // Renderkamera projiziert — inklusive Roll, so dass der Kompass
        // exakt so zeigt wie die Ansicht (vorher Nachbau der Drehungen,
        // in dem Z falsch herum ging und Y bei Pitch fast stand).
        let (right, up, _back) = cam.basis();
        let anchor = Pos2::new(rect.left() + 54.0, rect.bottom() - 54.0);
        for (axis, color, label) in [
            (RcVec3::new(1.0, 0.0, 0.0), Color32::from_rgb(220, 70, 70), "X"),
            (RcVec3::new(0.0, 1.0, 0.0), Color32::from_rgb(60, 170, 90), "Y"),
            (RcVec3::new(0.0, 0.0, 1.0), Color32::from_rgb(80, 130, 240), "Z"),
        ] {
            // Bildschirm x zeigt nach rechts (egui-y nach unten), die Welt-
            // skizze nach oben; Streuung in die Tiefe (back) bleibt unberücksichtigt.
            let end = anchor + Vec2::new(axis.dot(right) as f32, -(axis.dot(up) as f32)) * 36.0;
            ui.painter()
                .line_segment([anchor, end], Stroke::new(2.0_f32, color));
            ui.painter().text(
                end + Vec2::new(4.0, -4.0),
                egui::Align2::LEFT_BOTTOM,
                label,
                egui::FontId::monospace(12.0),
                color,
            );
        }
        ui.painter()
            .circle_filled(anchor, 2.5, Color32::from_gray(140));

        // Drag-&-Drop-Overlay
        let hovering = ui.input(|i| !i.raw.hovered_files.is_empty());
        if hovering {
            let painter = ui.painter_at(rect);
            painter.rect_filled(
                rect,
                0.0,
                Color32::from_rgba_unmultiplied(20, 30, 45, 180),
            );
            painter.rect_stroke(
                rect.shrink(12.0),
                10.0,
                Stroke::new(2.5_f32, Color32::from_rgb(120, 200, 255)),
                egui::StrokeKind::Outside,
            );
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "STEP-Datei hier ablegen",
                egui::FontId::proportional(26.0),
                Color32::from_rgb(220, 240, 255),
            );
        }
    }
}

impl eframe::App for AppState {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // 3D-Maus beim ersten Bild anfangen (braucht den egui-Context).
        if self.spacemouse.is_none() {
            self.spacemouse = space_mouse::start(&ctx);
        }
        self.poll_loading(&ctx);

        let dropped = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .find_map(|file| Some(file.path().to_path_buf()))
        });
        if let Some(path) = dropped {
            self.start_load(path, true);
        }

        // Tastaturkürzel (wie in CAD-Programmen)
        ctx.input(|i| {
            if i.key_pressed(egui::Key::F) {
                self.camera.fit_view();
            }
            if i.key_pressed(egui::Key::Num1) {
                self.camera.set_standard_view(StandardView::Front);
            }
            if i.key_pressed(egui::Key::Num2) {
                self.camera.set_standard_view(StandardView::Top);
            }
            if i.key_pressed(egui::Key::Num3) {
                self.camera.set_standard_view(StandardView::Right);
            }
            if i.key_pressed(egui::Key::Num4) {
                self.camera.set_standard_view(StandardView::Iso);
            }
        });

        egui::Panel::left("control_panel")
            .min_size(360.0)
            .resizable(true)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.ui_panel(ui));
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(Color32::from_rgb(210, 216, 226)),
            )
            .show(ui, |ui| self.viewport(ui));
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        if let Some(rx) = self.loading.take() {
            drop(rx);
        }
    }
}

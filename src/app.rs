//! Anwendungsoberfläche: Steuerpanel, Viewport-Interaktion und
//! asynchrones Laden/Tessellieren.

use crate::camera::{OrbitCamera, StandardView};
use crate::mesh::generate_sample_milled_part;
use crate::renderer::{CameraUniforms, SceneData, ViewMode, ViewportCallback};
use crate::step::{self, Body, Quality, StepDocument};
use crate::stock::{self, StockResult};
use eframe::egui::{self, Color32, Pos2, RichText, Sense, Stroke, Vec2};
use glam::Vec3;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

struct LoadOk {
    document: Option<Arc<StepDocument>>,
    bodies: Vec<Body>,
    stock: StockResult,
    warnings: Vec<String>,
    quality: Quality,
    path: Option<PathBuf>,
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
    document: Option<Arc<StepDocument>>,
    bodies: Vec<Body>,
    stock: StockResult,
    scene: Option<Arc<SceneData>>,
    revision: u64,
    file_path: Option<PathBuf>,
    status: String,
    status_is_error: bool,
    loading: Option<Receiver<LoadMsg>>,
}

impl AppState {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        if let Some(render_state) = cc.wgpu_render_state.as_ref() {
            let renderer = crate::renderer::SceneRenderer::new(
                &render_state.device,
                render_state.target_format,
            );
            render_state
                .renderer
                .write()
                .callback_resources
                .insert(renderer);
        }

        let mut app = Self {
            camera: OrbitCamera::default(),
            mode: ViewMode::ShadedEdges,
            show_stock: true,
            quality: Quality::Medium,
            document: None,
            bodies: Vec::new(),
            stock: StockResult::default(),
            scene: None,
            revision: 0,
            file_path: None,
            status: String::new(),
            status_is_error: false,
            loading: None,
        };
        app.load_demo();
        if let Some(path) = initial {
            app.start_load(path, true);
        }
        app
    }

    fn reset_camera(&mut self) {
        if let Some(scene) = &self.scene {
            self.camera.fit(scene.center, scene.radius);
        }
    }

    fn load_demo(&mut self) {
        let mut mesh = generate_sample_milled_part(110.0, 42.0, 18.0);
        let stock = stock::orient_and_align_mesh(&mut mesh);
        self.bodies = vec![Body {
            name: "Demo-Bauteil".to_string(),
            mesh,
            visible: true,
        }];
        self.stock = stock;
        self.document = None;
        self.file_path = None;
        self.revision += 1;
        self.rebuild_scene();
        self.reset_camera();
        self.status =
            "Demo-Bauteil aktiv – STEP-Datei über „Öffnen“ laden oder ins Fenster ziehen."
                .to_string();
        self.status_is_error = false;
    }

    fn rebuild_scene(&mut self) {
        self.revision += 1;
        let scene = SceneData::build(&self.bodies, &self.stock, self.revision);
        self.scene = Some(Arc::new(scene));
    }

    /// Lädt eine Datei im Hintergrund, damit die Oberfläche nicht blockiert.
    fn start_load(&mut self, path: PathBuf, fit_camera: bool) {
        if self.loading.is_some() {
            return;
        }
        let quality = self.quality;
        let (tx, rx) = mpsc::channel();
        let path_clone = path.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let doc = StepDocument::from_path(&path_clone)?;
                let part = step::tessellate(&doc, quality)?;
                let mut meshes: Vec<crate::mesh::MeshData> =
                    part.bodies.into_iter().map(|b| b.mesh).collect();
                let stock = stock::orient_and_align_bodies(&mut meshes);
                let bodies = meshes
                    .into_iter()
                    .enumerate()
                    .map(|(i, mesh)| Body {
                        name: format!("Körper {}", i + 1),
                        mesh,
                        visible: true,
                    })
                    .collect();
                Ok(LoadOk {
                    document: Some(Arc::new(doc)),
                    bodies,
                    stock,
                    warnings: part.warnings,
                    quality,
                    path: Some(path_clone),
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

    /// Tesselliert das bereits geparste Dokument mit einer neuen Qualität.
    fn start_retessellate(&mut self) {
        let Some(doc) = self.document.clone() else {
            return;
        };
        if self.loading.is_some() {
            return;
        }
        let quality = self.quality;
        let path = self.file_path.clone();
        let (tx, rx) = mpsc::channel();
        let doc_for_thread = doc.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let part = step::tessellate(&doc_for_thread, quality)?;
                let mut meshes: Vec<crate::mesh::MeshData> =
                    part.bodies.into_iter().map(|b| b.mesh).collect();
                let stock = stock::orient_and_align_bodies(&mut meshes);
                let bodies = meshes
                    .into_iter()
                    .enumerate()
                    .map(|(i, mesh)| Body {
                        name: format!("Körper {}", i + 1),
                        mesh,
                        visible: true,
                    })
                    .collect();
                Ok(LoadOk {
                    document: Some(doc_for_thread),
                    bodies,
                    stock,
                    warnings: part.warnings,
                    quality,
                    path,
                })
            }))
            .unwrap_or_else(|_| {
                Err("Beim Neu-Tessellieren ist ein interner Fehler aufgetreten.".to_string())
            });
            let _ = tx.send(LoadMsg {
                result,
                fit_camera: false,
            });
        });
        self.loading = Some(rx);
        self.status = format!("Tesselliere neu … (Qualität: {})", quality.label());
        self.status_is_error = false;
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
                        self.document = ok.document;
                        self.bodies = ok.bodies;
                        self.stock = ok.stock;
                        self.file_path = ok.path;
                        self.quality = ok.quality;
                        self.rebuild_scene();
                        if msg.fit_camera {
                            self.reset_camera();
                        }
                        let tri = self
                            .scene
                            .as_ref()
                            .map(|s| s.triangle_count)
                            .unwrap_or(0);
                        let name = self
                            .file_path
                            .as_ref()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "STEP".to_string());
                        let mut status = format!(
                            "{name}: {tri} Dreiecke, Bauteil {:.1} × {:.1} × {:.1} mm",
                            self.stock.part_size.x,
                            self.stock.part_size.y,
                            self.stock.part_size.z
                        );
                        if let Some(scale) = self.document.as_ref().map(|d| d.unit_scale) {
                            if (scale - 1.0).abs() > f32::EPSILON {
                                status.push_str(&format!(" (Dateieinheit × {scale} → mm)"));
                            }
                        }
                        if !ok.warnings.is_empty() {
                            status.push_str(&format!(" — {} Warnung(en)", ok.warnings.len()));
                        }
                        self.status = status;
                        self.status_is_error = false;
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
            egui::ComboBox::from_id_source("tess_quality")
                .selected_text(self.quality.label())
                .show_ui(ui, |ui| {
                    for q in Quality::ALL {
                        ui.selectable_value(&mut self.quality, q, q.label());
                    }
                });
            if self.quality != before {
                self.start_retessellate();
            }
        });

        let file_label = self
            .file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .or_else(|| self.document.as_ref().map(|d| d.name.clone()))
            .unwrap_or_else(|| "–".to_string());
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
        ui.label(format!("X (Länge):  {:.2} mm", self.stock.part_size.x));
        ui.label(format!("Y (Breite): {:.2} mm", self.stock.part_size.y));
        ui.label(format!("Z (Höhe):   {:.2} mm", self.stock.part_size.z));

        ui.add_space(8.0);
        ui.label(
            RichText::new("2. Berechnetes Rohteil")
                .strong()
                .color(Color32::from_rgb(180, 130, 20)),
        );
        ui.label(format!(
            "X-Rohmaß: {:.2} mm (+5.0 mm Sägeaufmaß)",
            self.stock.stock_size.x
        ));
        ui.label(format!(
            "Y-Rohmaß: {:.2} mm (Halbzeug-Staffel)",
            self.stock.stock_size.y
        ));
        ui.label(format!(
            "Z-Rohmaß: {:.2} mm (10-mm-Stufen)",
            self.stock.stock_size.z
        ));

        ui.add_space(8.0);
        ui.label(RichText::new("3. Z-Spannsituation").strong());
        ui.label(format!(
            "• Schraubstock-Spanntiefe: {:.1} mm (unten)",
            self.stock.clamp_depth_z
        ));
        ui.label(format!(
            "• Planfräs-Aufmaß:         {:.1} mm (oben)",
            self.stock.top_allowance_z
        ));

        ui.separator();
        ui.label(RichText::new("Darstellung").strong());
        ui.horizontal(|ui| {
            ui.label("Modus:");
            egui::ComboBox::from_id_source("view_mode")
                .selected_text(self.mode.label())
                .show_ui(ui, |ui| {
                    for m in ViewMode::ALL {
                        ui.selectable_value(&mut self.mode, m, m.label());
                    }
                });
        });
        ui.checkbox(&mut self.show_stock, "Rohteil-Box anzeigen");

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

        if self.bodies.len() > 1 {
            ui.add_space(6.0);
            ui.label("Körper:");
            let mut changed = false;
            for body in &mut self.bodies {
                changed |= ui.checkbox(&mut body.visible, &body.name).changed();
            }
            if changed {
                self.rebuild_scene();
            }
        }

        ui.separator();
        ui.label(RichText::new("Steuerung Viewport").strong());
        ui.label("• Linke Maustaste: Drehen");
        ui.label("• Rechte Maustaste / Shift: Verschieben");
        ui.label("• Mausrad: Zoomen");
        ui.label("• Doppelklick: Einpassen");
        ui.label("• STEP-Datei ins Fenster ziehen: Laden");

        let tri = self.scene.as_ref().map(|s| s.triangle_count).unwrap_or(0);
        ui.add_space(4.0);
        ui.label(format!("Dreiecke: {tri}"));
        ui.label(format!(
            "GPU-Modus: {}",
            if self.document.is_some() {
                "wgpu / MSAA 4×"
            } else {
                "wgpu / MSAA 4× (Demo)"
            }
        ));
    }

    fn viewport(&mut self, ui: &mut egui::Ui) {
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());

        let shift = ui.input(|i| i.modifiers.shift);
        let pan_btn = response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle);
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

        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll.abs() > 0.0 && response.hovered() {
            self.camera.zoom(scroll);
        }

        let Some(scene) = self.scene.clone() else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Keine Geometrie geladen",
                egui::FontId::proportional(18.0),
                Color32::GRAY,
            );
            return;
        };

        let aspect = rect.width() / rect.height().max(1.0);
        let camera = CameraUniforms {
            view_proj: self.camera.view_projection(aspect),
            eye: self.camera.eye_position(),
        };

        let callback = ViewportCallback::new(
            scene,
            camera,
            rect.size(),
            ui.ctx().pixels_per_point(),
            self.mode,
            self.show_stock,
        );
        ui.painter()
            .add(egui_wgpu::Callback::new_paint_callback(rect, callback));

        // Achsen-Gizmo (2D-Overlay, an die Kamera gekoppelt)
        let rotate = |p: Vec3| -> Vec3 {
            let (sy, cy) = self.camera.yaw.sin_cos();
            let (sp, cp) = self.camera.pitch.sin_cos();
            let x1 = p.x * cy - p.y * sy;
            let y1 = p.x * sy + p.y * cy;
            let z1 = p.z;
            Vec3::new(x1, y1 * cp - z1 * sp, y1 * sp + z1 * cp)
        };
        let anchor = Pos2::new(rect.left() + 54.0, rect.bottom() - 54.0);
        for (axis, color, label) in [
            (Vec3::X, Color32::from_rgb(220, 70, 70), "X"),
            (Vec3::Y, Color32::from_rgb(60, 170, 90), "Y"),
            (Vec3::Z, Color32::from_rgb(80, 130, 240), "Z"),
        ] {
            let v = rotate(axis);
            let end = anchor + Vec2::new(v.x, -v.z) * 36.0;
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
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_loading(ctx);

        let dropped = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .find_map(|file| file.path.clone())
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

        egui::SidePanel::left("control_panel")
            .min_width(360.0)
            .resizable(true)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.ui_panel(ui));
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::from_rgb(210, 216, 226)))
            .show(ctx, |ui| self.viewport(ui));
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        if let Some(rx) = self.loading.take() {
            drop(rx);
        }
    }
}

//! Anwendungsoberfläche: Steuerpanel, Viewport-Interaktion und
//! asynchrones Laden/Tessellieren (Import via SolidCraft).
use crate::camera::{OrbitCamera, StandardView};
use crate::mesh::generate_sample_milled_part;
use crate::renderer::{
    ScModel, ScSession, SceneState, ViewCallback, ViewMode, demo_scene, make_frame,
    render_camera,
};
use crate::step::Quality;
use crate::shadow;
use crate::space_mouse;
use eframe::egui::{self, Sense, Stroke};
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

pub(crate) struct LoadMsg {
    result: Result<LoadOk, String>,
    fit_camera: bool,
}

pub struct AppState {
    pub(crate) camera: OrbitCamera,
    pub(crate) mode: ViewMode,
    pub(crate) show_stock: bool,
    pub(crate) quality: Quality,
    session: Option<ScSession>,
    pub(crate) model: Option<Arc<ScModel>>,
    gpustate: Option<SceneState>,
    pub(crate) revision: u64,
    pub(crate) file_path: Option<PathBuf>,
    pub(crate) status: String,
    pub(crate) status_is_error: bool,
    pub(crate) loading: Option<Receiver<LoadMsg>>,
    /// 3D-Maus (SpaceMouse): Leser-Thread + geteilte Achsen; `None`, wenn
    /// kein Gerät gelesen werden kann oder die app noch nicht begonnen hat.
    spacemouse: Option<space_mouse::SpaceMouse>,
    /// Schattenkontur: gewünschter Außenabstand (mm) und letzte Rechnung.
    pub(crate) shadow_expand: f32,
    pub(crate) shadow: Option<shadow::ShadowResult>,
    /// Vorschau der Kontur im 3D-Fenster (cyanfarbene Linie).
    pub(crate) shadow_preview: bool,
    /// Einstellbare Aufmaße (Säge/Spanntiefe/Planfräs) — Steuerparameter.
    pub(crate) allowances: crate::stock::StockParams,
    /// Zuletzt gewählte Standardansicht (Hervorhebung im Overlay); None
    /// nach freiem Orbit.
    pub(crate) active_view: Option<StandardView>,
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
            shadow_expand: 3.0,
            shadow: None,
            shadow_preview: true,
            allowances: crate::stock::StockParams::default(),
            active_view: None,
        };
        // Pipelines wurden in main (renderer::install) angelegt; egal wer
        // zuerst — hier fehlt nur der scene upload.
        app.load_demo();
        if let Some(path) = initial {
            app.start_load(path, true);
        }
        app
    }

    pub(crate) fn reset_camera(&mut self) {
        if let Some(model) = &self.model {
            self.camera.fit(model.center, model.radius);
        }
    }

    /// Kamera auf Ausgangszustand (Iso) und Einpassen.
    pub(crate) fn camera_reset(&mut self) {
        self.camera = OrbitCamera::default();
        self.camera.set_standard_view(StandardView::Iso);
        self.active_view = Some(StandardView::Iso);
        self.reset_camera();
    }

    /// Szenen-Neu-Upload für die Kontur-Vorschau anstoßen (mit/ohne Linie).
    pub(crate) fn refresh_preview(&mut self) {
        self.revision += 1;
        self.gpustate = None;
    }

    /// Schattenkontur (Projektion auf XY, nach außen vergrößert) erzeugen.
    pub(crate) fn build_shadow(&mut self) {
        let Some(model) = self.model.clone() else {
            self.shadow = None;
            self.status = "Schattenkontur: erst eine STEP-Datei laden.".to_string();
            self.status_is_error = true;
            return;
        };
        let expand = self.shadow_expand.max(0.0);
        match shadow::build(&model, expand) {
            Ok(result) => {
                // Szenen-Neu-Upload anstoßen, so dass die Vorschau sofort
                // erscheint (Viewport hängt an `revision`).
                self.revision += 1;
                self.gpustate = None;
                self.status = format!(
                    "Schattenkontur erzeugt: {} Punkte · Umfang {:.1} mm · Fläche {:.0} mm² · Abstand {:.1} mm (Auflösung {:.2} mm)",
                    result.polygon.len(),
                    result.perimeter,
                    result.area,
                    result.expand,
                    result.res,
                );
                self.status_is_error = false;
                self.shadow = Some(result);
            }
            Err(e) => {
                self.shadow = None;
                self.status = format!("Schattenkontur: {e}");
                self.status_is_error = true;
            }
        }
    }

    /// Die fertige Schattenkontur als DXF (mm) speichern.
    pub(crate) fn save_shadow_dxf(&mut self) {
        let Some(shadow) = &self.shadow else { return };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("DXF (Datenaustausch)", &["dxf"])
            .set_file_name("schattenkontur.dxf")
            .save_file()
        else {
            return;
        };
        match shadow::write_dxf(&path, &shadow.polygon) {
            Ok(()) => {
                self.status = format!(
                    "DXF gespeichert: {} ({} Punkte, Abstand {:.1} mm)",
                    path.display(),
                    shadow.polygon.len(),
                    shadow.expand,
                );
                self.status_is_error = false;
            }
            Err(e) => {
                self.status = format!("DXF: {e}");
                self.status_is_error = true;
            }
        }
    }

    pub(crate) fn load_demo(&mut self) {
        let mesh = generate_sample_milled_part(110.0, 42.0, 18.0);
        match demo_scene(mesh, self.mode, self.show_stock, &self.allowances) {
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
        // Alte Kontur nur behalten, wenn die Projektion unverändert ist
        // (Tessellierung neu, gleiche Geometrie) — sonst verfällt sie.
        let keep_shadow = self
            .shadow
            .as_ref()
            .is_some_and(|s| s.same_footprint(&model.part_positions));
        if !keep_shadow {
            self.shadow = None;
        }
        self.revision += 1;
        self.model = Some(Arc::new(model));
        self.gpustate = None; // Neu-Upload beim nächsten Bild
    }

    /// Lädt eine Datei im Hintergrund, damit die Oberfläche nicht blockiert.
    pub(crate) fn start_load(&mut self, path: PathBuf, fit_camera: bool) {
        if self.loading.is_some() {
            return;
        }
        let quality = self.quality;
        let mode = self.mode;
        let show_stock = self.show_stock;
        let allowances = self.allowances;
        let (tx, rx) = mpsc::channel();
        let path_clone = path.clone();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let session = ScSession::open(&path_clone, quality)?;
                let model = session.model(mode, show_stock, allowances)?;
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
    pub(crate) fn start_rettesellate(&mut self) {
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

    /// Szene mit neuen Anzeigeeinstellungen synchron neu bauen.
    pub(crate) fn rebuild(&mut self) {
        if let Some(session) = &self.session {
            match session.model(self.mode, self.show_stock, self.allowances) {
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

    pub(crate) fn load_demo_preserve_camera(&mut self) {
        let mesh = generate_sample_milled_part(110.0, 42.0, 18.0);
        if let Ok(model) = demo_scene(mesh, self.mode, self.show_stock, &self.allowances) {
            self.session = None;
            self.file_path = None;
            self.set_model(model);
        }
    }

    pub(crate) fn set_body_visible(&mut self, name: &str, visible: bool) {
        let Some(session) = &mut self.session else {
            return;
        };
        if let Err(e) = session.set_visible(name, visible) {
            self.status = e;
            self.status_is_error = true;
            return;
        }
        if let Some(session) = &self.session {
            match session.model(self.mode, self.show_stock, self.allowances) {
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
            if m.orbit_x != 0.0 || m.orbit_y != 0.0 || m.roll != 0.0 {
                self.active_view = None; // freie Fahrt — keine Standardansicht aktiv
            }
            self.camera.orbit_on_screen(m.orbit_x, m.orbit_y);
            self.camera.roll_by(m.roll);
        }
        if response.dragged() {
            let delta = response.drag_delta();
            if shift || pan_btn {
                self.camera.pan(delta.x, delta.y, rect.height());
            } else {
                self.active_view = None; // Freier Orbit — Auswahl entfällt
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
                crate::ui::theme::EMPTY_TEXT,
            );
            return;
        };

        // GPU-Upload je Revision anlegen (Szene → GPU beim ersten Bild);
        // mit Schattenkontur-Vorschau kommt die cyanfarbene Konturlinie mit.
        let revision = self.revision;
        let state = match &self.gpustate {
            Some(st) if st.key == revision => st,
            _ => {
                let mut st = match (&self.shadow, self.shadow_preview) {
                    (Some(s), true) => SceneState::new(&crate::renderer::scene_with_shadow_contour(
                        &model.scene,
                        s,
                        model.part_size.z + 0.25,
                    )),
                    _ => SceneState::new(&model.scene),
                };
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

        // Overlays: Ansichten-Leiste oben rechts, Legende und Kompass unten
        // links (der Kompass projiziert die Weltachsen auf die Renderkamera-
        // Basis — inklusive Roll, so dass er exakt so zeigt wie die Ansicht).
        crate::ui::viewport_overlay::show(self, ui, &cam);

        // Drag-&-Drop-Overlay
        let hovering = ui.input(|i| !i.raw.hovered_files.is_empty());
        if hovering {
            let painter = ui.painter_at(rect);
            painter.rect_filled(
                rect,
                0.0,
                crate::ui::theme::DND_FILL,
            );
            painter.rect_stroke(
                rect.shrink(12.0),
                10.0,
                Stroke::new(2.5_f32, crate::ui::theme::DND_STROKE),
                egui::StrokeKind::Outside,
            );
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "STEP-Datei hier ablegen",
                egui::FontId::proportional(26.0),
                crate::ui::theme::DND_TEXT,
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
                .map(|file| file.path().to_path_buf())
                .next()
        });
        if let Some(path) = dropped {
            self.start_load(path, true);
        }

        // Tastaturkürzel (wie in CAD-Programmen) — auch die Hervorhebung im
        // Ansichten-Overlay folgt mit.
        ctx.input(|i| {
            if i.key_pressed(egui::Key::F) {
                self.camera.fit_view();
                self.active_view = None;
            }
            if i.key_pressed(egui::Key::Num1) {
                self.camera.set_standard_view(StandardView::Front);
                self.active_view = Some(StandardView::Front);
            }
            if i.key_pressed(egui::Key::Num2) {
                self.camera.set_standard_view(StandardView::Top);
                self.active_view = Some(StandardView::Top);
            }
            if i.key_pressed(egui::Key::Num3) {
                self.camera.set_standard_view(StandardView::Right);
                self.active_view = Some(StandardView::Right);
            }
            if i.key_pressed(egui::Key::Num4) {
                self.camera.set_standard_view(StandardView::Iso);
                self.active_view = Some(StandardView::Iso);
            }
        });

        // Linke Leiste: dunkles Theme, feste Standardbreite (320 px,
        // min 280, max 420), Inhalt scrollbar, Fußleiste fix unten.
        egui::Panel::left("sidebar")
            .default_size(crate::ui::theme::SIDEBAR_DEFAULT)
            .min_size(crate::ui::theme::SIDEBAR_MIN)
            .max_size(crate::ui::theme::SIDEBAR_MAX)
            .resizable(true)
            .frame(
                egui::Frame::new()
                    .fill(crate::ui::theme::BG)
                    .inner_margin(egui::Margin::same(8)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("sidebar_scroll")
                    .show(ui, |ui| crate::ui::sidebar::content(self, ui));
                ui.add_space(4.0);
                crate::ui::sidebar::footer(self, ui);
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(crate::ui::theme::VIEWPORT_BG))
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

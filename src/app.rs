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
use glam::Vec3;
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
    /// Ausrichtungs-Übersteuerung („Neu ausrichten“): Normale der
    /// angeklickten Fläche in Datei-Koordinaten, die unten (−Z) liegen
    /// soll; `None` = automatische Ausrichtung.
    pub(crate) align_down: Option<Vec3>,
    /// Pick-Modus: der nächste Flächenklick im Viewport übernimmt die
    /// „unten“-Richtung.
    pub(crate) align_picking: bool,
    /// Verfahrenskürzel für den DXF-Dateinamen (Stammname + `_WASSER` usw.).
    pub(crate) dxf_variant: DxfVariant,
    /// Beschriftung im DXF als `S=<Wert>mm`: nur der Wert (z. B. „70“) ist
    /// editierbar, Präfix/Suffix sind fest. `dxf_label_auto_value` merkt den
    /// letzten Vorschlag, damit manuelle Änderungen bei Modell-Updates erhalten
    /// bleiben.
    pub(crate) dxf_label_value: String,
    dxf_label_auto_value: String,
    /// Innenkonturen (Bohrung/Tasche) für DXF, per Flächenklick hinzugefügt.
    pub(crate) inner_pick_active: bool,
    /// Versatz nach innen (mm) für neu hinzugefügte Innenkonturen.
    pub(crate) inner_offset_mm: f32,
}

/// Name-Vorlage für den DXF-Export (Fußleiste): Stammname der geladenen
/// STEP-Datei plus Verfahrenskürzel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DxfVariant {
    #[default]
    Wasser,
    Laser,
    Brenn,
}

impl DxfVariant {
    pub(crate) fn suffix(self) -> &'static str {
        match self {
            Self::Wasser => "WASSER",
            Self::Laser => "LASER",
            Self::Brenn => "BRENN",
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Wasser => "Wasser",
            Self::Laser => "Laser",
            Self::Brenn => "Brennen",
        }
    }
    pub(crate) const ALL: [Self; 3] = [Self::Wasser, Self::Laser, Self::Brenn];
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
            align_down: None,
            align_picking: false,
            dxf_variant: DxfVariant::default(),
            dxf_label_value: String::new(),
            dxf_label_auto_value: String::new(),
            inner_pick_active: false,
            inner_offset_mm: 1.0,
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
        let keep_inners = self
            .shadow
            .as_ref()
            .map(|s| (s.inner_polygons.clone(), s.inner_offset))
            .unwrap_or_default();
        match shadow::build(&model, expand, 0.0, 0.0) {
            Ok(mut result) => {
                if !keep_inners.0.is_empty() {
                    result.inner_polygons = keep_inners.0;
                    result.inner_offset = keep_inners.1;
                }
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

    /// Die fertige Schattenkontur als DXF (mm) speichern. Der vorgeschlagene
    /// Dateiname ist der Stammname der geladenen STEP-Datei plus
    /// Verfahrenskürzel (z. B. `deckel_WASSER.dxf`), Auswahl über die
    /// Sektion „Schattenkontur (DXF)“.
    pub(crate) fn save_shadow_dxf(&mut self) {
        let Some(shadow) = &self.shadow else { return };
        let stem = self
            .file_path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "schattenkontur".to_string());
        let suggested = format!("{stem}_{}.dxf", self.dxf_variant.suffix());
        let Some(path) = rfd::FileDialog::new()
            .add_filter("DXF (Datenaustausch)", &["dxf"])
            .set_file_name(&suggested)
            .save_file()
        else {
            return;
        };
        let label = self.dxf_label_text();
        let label = if label.is_empty() { None } else { Some(label.as_str()) };
        match shadow::write_dxf(&path, &shadow.polygon, &shadow.inner_polygons, label) {
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
        self.align_down = None;
        self.align_picking = false;
        self.inner_pick_active = false;
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

    fn dxf_label_proposal_value(model: &ScModel) -> String {
        let z = model.stock_size.z as f64;
        if (z - z.round()).abs() < 0.05 {
            format!("{:.0}", z.round())
        } else {
            format!("{:.1}", z)
        }
    }

    /// Vollständiger DXF-Beschriftungstext: `S=<Wert>mm` (oder leer).
    pub(crate) fn dxf_label_text(&self) -> String {
        let v = self.dxf_label_value.trim();
        if v.is_empty() {
            String::new()
        } else {
            format!("S={v}mm")
        }
    }

    /// Vorschlag für die DXF-Beschriftung aus dem aktuellen Rohteil übernehmen.
    pub(crate) fn propose_dxf_label(&mut self) {
        if let Some(model) = &self.model {
            let proposal = Self::dxf_label_proposal_value(model);
            self.dxf_label_value = proposal.clone();
            self.dxf_label_auto_value = proposal;
        }
    }

    fn set_model(&mut self, model: ScModel) {
        // DXF-Beschriftung: Vorschlag aus Rohteil-Z aktualisieren, solange
        // der Wert noch dem letzten Auto-Vorschlag entspricht (manuelle
        // Änderungen bleiben erhalten).
        let proposal = Self::dxf_label_proposal_value(&model);
        if self.dxf_label_value.trim().is_empty() || self.dxf_label_value == self.dxf_label_auto_value {
            self.dxf_label_value = proposal.clone();
        }
        self.dxf_label_auto_value = proposal;

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
        // Neue Datei = neue Geometrie: Ausrichtungs-Übersteuerung verfällt.
        self.align_down = None;
        self.align_picking = false;
        self.inner_pick_active = false;
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
            match session.model_with_down(self.mode, self.show_stock, self.allowances, self.align_down)
            {
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

    /// „Neu ausrichten“ ein-/ausschalten: im Pick-Modus übernimmt der
    /// nächste Flächenklick im Viewport die „unten“-Richtung (Esc bricht ab).
    pub(crate) fn toggle_align_picking(&mut self) {
        self.align_picking = !self.align_picking;
        if self.align_picking {
            self.inner_pick_active = false;
            self.status =
                "Neu ausrichten: Fläche im 3D-Fenster anklicken — sie liegt danach unten (Esc bricht ab)."
                    .to_string();
            self.status_is_error = false;
        }
    }

    /// Innenkontur-Pickmodus (Bohrung/Tasche) umschalten.
    pub(crate) fn toggle_inner_pick(&mut self) {
        if self.shadow.is_none() {
            self.status = "Innenkontur: erst eine Schattenkontur erzeugen.".to_string();
            self.status_is_error = true;
            self.inner_pick_active = false;
            return;
        }
        self.inner_pick_active = !self.inner_pick_active;
        if self.inner_pick_active {
            self.align_picking = false;
            self.status =
                "Innenkontur: Fläche einer Bohrung/Tasche im 3D-Fenster anklicken (Esc bricht ab)."
                    .to_string();
            self.status_is_error = false;
        }
    }

    /// Alle hinzugefügten Innenkonturen entfernen.
    pub(crate) fn clear_inner_contours(&mut self) {
        if let Some(s) = &mut self.shadow {
            s.inner_polygons.clear();
        }
        self.inner_pick_active = false;
        self.refresh_preview();
        self.status = "Innenkonturen entfernt.".to_string();
        self.status_is_error = false;
    }

    /// Innenkonturen automatisch aus der Silhouette erkennen: alle Löcher/
    /// Konturen, die >= 40×40 mm (Bounding-Box) oder Durchmesser >= 40 mm
    /// sind, werden mit aktuellem Versatz nach innen übernommen.
    pub(crate) fn auto_detect_inner_contours(&mut self) {
        let Some(model) = self.model.clone() else {
            self.status = "Innenkonturen: erst eine STEP-Datei laden.".to_string();
            self.status_is_error = true;
            return;
        };
        let expand = self.shadow_expand.max(0.0);
        let offset = self.inner_offset_mm.max(0.0);
        match shadow::build(&model, expand, 40.0, offset) {
            Ok(result) => {
                let n = result.inner_polygons.len();
                self.revision += 1;
                self.gpustate = None;
                self.shadow = Some(result);
                self.inner_pick_active = false;
                self.status = if n == 0 {
                    "Keine Innenkonturen ≥ 40×40 mm / Ø 40 mm erkannt.".to_string()
                } else {
                    format!("Automatisch {n} Innenkontur(en) erkannt (≥ 40×40 mm / Ø 40 mm, Versatz {:.1} mm).", offset)
                };
                self.status_is_error = false;
            }
            Err(e) => {
                self.status = format!("Innenkonturen automatisch erkennen: {e}");
                self.status_is_error = true;
            }
        }
    }

    /// Ausrichtungs-Übersteuerung zurücknehmen (automatische Ausrichtung)
    /// und Rohteil neu berechnen.
    pub(crate) fn reset_alignment(&mut self) {
        self.align_down = None;
        self.align_picking = false;
        self.inner_pick_active = false;
        let had_shadow = self.shadow.is_some();
        self.rebuild();
        if had_shadow && self.shadow.is_none() {
            self.build_shadow();
        }
        // Innenkonturen beziehen sich auf die vorige Ausrichtung — verwerfen.
        if let Some(s) = &mut self.shadow {
            s.inner_polygons.clear();
        }
        self.refresh_preview();
    }

    /// Flächenklick im Pick-Modus auswerten: Normale der getroffenen Fläche
    /// (Anzeige-Koordinaten) zurück in Datei-Koordinaten drehen, als
    /// „unten“-Richtung übernehmen und Rohteil neu berechnen.
    pub(crate) fn apply_align_pick(&mut self, pos: [f32; 2], rect: eframe::egui::Rect) {
        self.align_picking = false;
        self.inner_pick_active = false;
        let Some(model) = self.model.clone() else {
            return;
        };
        let Some(normal) = crate::pick::face_normal(&model, &self.camera, rect, pos) else {
            self.status = "Neu ausrichten: kein Bauteil getroffen — erneut klicken oder abbrechen."
                .to_string();
            self.status_is_error = true;
            return;
        };
        // Angezeigte (ausgerichtete) Richtung zurück in Datei-Koordinaten.
        let down_file = model.align_rot.transpose() * normal;
        self.align_down = Some(down_file);
        let had_shadow = self.shadow.is_some();
        self.rebuild();
        if had_shadow && self.shadow.is_none() {
            // Kontur mit gleicher Zulage für die neue Lage mit erzeugen.
            self.build_shadow();
        }
        // Innenkonturen beziehen sich auf die vorige Ausrichtung — verwerfen.
        if let Some(s) = &mut self.shadow {
            s.inner_polygons.clear();
        }
        self.refresh_preview();
        if let Some(model) = &self.model {
            self.status = format!(
                "Neu ausgerichtet: gewählte Fläche unten — Rohteil {:.1} × {:.1} × {:.1} mm",
                model.stock_size.x, model.stock_size.y, model.stock_size.z
            );
            self.status_is_error = false;
        }
    }

    /// Innenkontur aus einer angeklickten Fläche erkennen (Bohrung/Tasche)
    /// und als DXF-Innenkontur mit Versatz nach innen hinzufügen.
    pub(crate) fn apply_inner_pick(&mut self, pos: [f32; 2], rect: eframe::egui::Rect) {
        self.inner_pick_active = false;
        let (origin, offset_mm) = match &self.shadow {
            Some(s) => (s.origin, self.inner_offset_mm.max(0.0)),
            None => {
                self.status = "Innenkontur: erst eine Schattenkontur erzeugen.".to_string();
                self.status_is_error = true;
                return;
            }
        };
        let Some(model) = self.model.clone() else { return; };
        let Some((mesh, tri_face, hit)) = crate::pick::pick_part_face(&model, &self.camera, rect, pos)
        else {
            self.status = "Innenkontur: keine Bauteilfläche getroffen.".to_string();
            self.status_is_error = true;
            return;
        };
        if tri_face == u32::MAX {
            self.status = "Innenkontur: Fläche hat keine B-rep-Zuordnung.".to_string();
            self.status_is_error = true;
            return;
        }
        let mut loops = crate::shadow::extract_face_xy_loops(&mesh, tri_face, origin);
        if loops.is_empty() {
            if let Some(cyl) = crate::shadow::extract_cylinder_xy(&mesh, tri_face, origin) {
                loops.push(cyl);
            }
        }
        if loops.is_empty() {
            // Nut-/Wand-Fallback: Der Wandzug selbst umschließt keine Fläche,
            // aber die Wand gehört zu einer Öffnung, die in der Draufsicht als
            // Loch in der Silhouette erscheint. Wir nehmen den Innenzug, der
            // dem Trefferpunkt in XY am nächsten liegt.
            let Ok((cands, probe_origin, probe_res)) = crate::shadow::silhouette_inner_loops(
                &model.part_positions,
                &model.part_triangles,
                self.shadow_expand.max(0.0),
            ) else {
                self.status = "Innenkontur: keine Kontur erkannt — bitte Taschenboden/-rand oder Bohrungswand anklicken.".to_string();
                self.status_is_error = true;
                return;
            };
            let h = [hit[0] - probe_origin[0], hit[1] - probe_origin[1]];
            let mut best: Option<(f32, Vec<[f32; 2]>)> = None;
            for poly in &cands {
                let d = crate::shadow::point_poly_dist(h, poly);
                if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                    best = Some((d, poly.clone()));
                }
            }
            let Some((d0, raw)) = best.filter(|(d, _)| *d < 10.0) else {
                self.status = "Innenkontur: Wand gehört zu keiner Öffnung — bei Taschen bitte Taschenboden/-rand anklicken.".to_string();
                self.status_is_error = true;
                return;
            };
            let Some(contour) = crate::shadow::prepare_inner(&raw, offset_mm, probe_res) else {
                self.status = format!(
                    "Innenkontur: Versatz {:.1} mm frisst diese Kontur auf — kleineren Versatz wählen.",
                    offset_mm
                );
                self.status_is_error = true;
                return;
            };
            if let Some(s) = &mut self.shadow {
                s.inner_polygons.push(contour);
                s.inner_offset = offset_mm;
            }
            self.refresh_preview();
            self.status = format!(
                "Innenkontur (Nut/Wand, Abstand zum Klick {:.2} mm) — Versatz {:.1} mm nach innen.",
                d0, offset_mm
            );
            self.status_is_error = false;
            return;
        }
        // Mehrere Loops: größte Kontur ist meist der Außenrand der Fläche,
        // kleinere sind Bohrungen/Taschen — diese bevorzugen. Eine einzelne
        // Kontur (z. B. Taschen-/Randkontur oder erkannter Bohrkreis) wird
        // direkt übernommen.
        let mut candidates = loops;
        candidates.sort_by(|a, b| {
            crate::shadow::area_abs(b)
                .partial_cmp(&crate::shadow::area_abs(a))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if candidates.len() > 1 {
            candidates.remove(0);
        }
        let mut added = 0usize;
        let mut seen_centers: Vec<[f32; 2]> = Vec::new();
        for poly in candidates {
            let c = if poly.is_empty() {
                [0.0, 0.0]
            } else {
                let mut sx = 0.0_f32;
                let mut sy = 0.0_f32;
                for p in &poly {
                    sx += p[0];
                    sy += p[1];
                }
                [sx / poly.len() as f32, sy / poly.len() as f32]
            };
            if seen_centers
                .iter()
                .any(|s| (s[0] - c[0]).hypot(s[1] - c[1]) < 0.5)
            {
                continue;
            }
            seen_centers.push(c);
            let contour = if offset_mm > 1e-6 {
                let off = crate::shadow::offset_polygon_inward(&poly, offset_mm);
                if off.len() >= 3 { off } else { poly }
            } else {
                poly
            };
            if let Some(s) = &mut self.shadow {
                s.inner_polygons.push(contour);
                s.inner_offset = offset_mm;
            }
            added += 1;
        }
        if added == 0 {
            self.status = "Innenkontur: erkannte Kontur konnte nicht übernommen werden.".to_string();
            self.status_is_error = true;
            return;
        }
        self.refresh_preview();
        self.status = format!(
            "Innenkontur hinzugefügt: {} Kontur(en), Versatz {:.1} mm nach innen.",
            added, offset_mm
        );
        self.status_is_error = false;
    }

    pub(crate) fn load_demo_preserve_camera(&mut self) {
        self.align_down = None;
        self.align_picking = false;
        self.inner_pick_active = false;
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

        // Pick-Modi: einfacher Klick (kein Orbit-Drag), Esc bricht ab.
        if self.align_picking || self.inner_pick_active {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.align_picking = false;
                self.inner_pick_active = false;
                self.status = "Pick-Modus abgebrochen.".to_string();
                self.status_is_error = false;
            } else if response.clicked() && !response.double_clicked() {
                let pos = response
                    .interact_pointer_pos()
                    .or_else(|| ui.input(|i| i.pointer.latest_pos()))
                    .map(|p| [p.x, p.y])
                    .unwrap_or([0.0, 0.0]);
                if self.inner_pick_active {
                    self.apply_inner_pick(pos, rect);
                } else {
                    self.apply_align_pick(pos, rect);
                }
            }
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

        // DXF-Beschriftung in der Vorschau anzeigen (Mitte der Kontur, XY).
        if self.shadow_preview {
            if let (Some(shadow), Some(model)) = (&self.shadow, &self.model) {
                let text = self.dxf_label_text();
                if !text.is_empty() && !shadow.polygon.is_empty() {
                    let (mut min_x, mut min_y) = (f32::INFINITY, f32::INFINITY);
                    let (mut max_x, mut max_y) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
                    for p in &shadow.polygon {
                        min_x = min_x.min(p[0]);
                        min_y = min_y.min(p[1]);
                        max_x = max_x.max(p[0]);
                        max_y = max_y.max(p[1]);
                    }
                    let z = model.part_size.z + 0.25;
                    let wx = shadow.origin[0] + (min_x + max_x) * 0.5;
                    let wy = shadow.origin[1] + (min_y + max_y) * 0.5;
                    let world = solidcore_record::Vec3::new(wx as f64, wy as f64, z as f64);
                    let radius = (model.scene.radius.max((model.radius as f64) * 1.05)).max(1.0);
                    if let Some((sx, sy, _depth)) = cam.to_screen(world, rect.width() as f64, rect.height() as f64, radius) {
                        let pos = egui::pos2(rect.left() + sx as f32, rect.top() + sy as f32);
                        let painter = ui.painter_at(rect);
                        let font = egui::FontId::proportional(14.0);
                        let galley = painter.layout_no_wrap(text.to_string(), font, crate::ui::theme::PREVIEW);
                        let text_pos = pos - galley.size() * 0.5;
                        painter.rect_filled(
                            egui::Rect::from_min_size(text_pos - egui::vec2(4.0, 2.0), galley.size() + egui::vec2(8.0, 4.0)),
                            3.0,
                            crate::ui::theme::BG.gamma_multiply(0.65),
                        );
                        painter.galley(text_pos, galley, crate::ui::theme::PREVIEW);
                    }
                }
            }
        }

        // Pick-Hinweise: über allem sichtbar, solange ein Modus aktiv ist.
        if self.align_picking {
            let painter = ui.painter_at(rect);
            painter.text(
                egui::pos2(rect.center().x, rect.top() + 46.0),
                egui::Align2::CENTER_TOP,
                "Neu ausrichten: Fläche anklicken, die unten liegen soll — Esc bricht ab.",
                egui::FontId::proportional(15.0),
                crate::ui::theme::ACCENT,
            );
        } else if self.inner_pick_active {
            let painter = ui.painter_at(rect);
            painter.text(
                egui::pos2(rect.center().x, rect.top() + 46.0),
                egui::Align2::CENTER_TOP,
                "Innenkontur (DXF): Fläche einer Bohrung/Tasche anklicken — Esc bricht ab.",
                egui::FontId::proportional(15.0),
                crate::ui::theme::ACCENT,
            );
        }

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

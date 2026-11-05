//! Viewport und Rendering genau auf SolidCrafts Art.
//!
//! SolidCraft (MIT OR Apache-2.0) wird über Path-Dependencies eingebunden:
//! - STEP-Import über `solidcraft-engine` (`Session` + `doc.open` — derselbe
//!   STEP-Reader wie in der SolidCraft-App),
//! - Anzeigemodell als `solidcraft_render::Scene` (Netze in Farben, Kanten,
//!   Hintergrund, Radius) — derselbe Szenenbegriff wie bei `snapshot`,
//! - Zeichnen mit den Pipelines von `solidcraft-gpu` (PBR, Silhouetten, Glass)
//!   im egui-Viewport bzw. via `solidcraft_gpu::capture` für `--render`.
//!
//! Rohteil-Ebene: die Szene wird rigide auf die Rohteil-Ausrichtung gedreht
//! (längste Achse → X, unten Z = 0); die Rohteil-Box erscheint als glasiger
//! Körper samt Spannlinie, das Bauteil sitzt auf einem Bodenraster.
use std::path::Path;
use std::sync::{Arc, Mutex};

use solidcraft_geom as _;
use solidcraft_gpu::view::{Frame, GpuScene, GpuTarget, Renderer, analysis_uniform};
use solidcraft_engine::Session;
use solidcraft_engine::view::Palette;
use solidcraft_render::Camera as RenderCamera;
use solidcraft_render::SceneMesh;
use solidcraft_geom::prelude::*;
use solidcore_record::Vec3 as RcVec3;

use glam::Vec3;
use crate::camera::OrbitCamera;
use crate::mesh::MeshData;
use crate::stock::{StockParams, StockResult};

// ---------------------------------------------------------
// Anzeigemodi
// ---------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    /// Schattiert mit den sichtbaren scharfen Kanten (SolidCraft-Standard).
    ShadedEdges,
    Shaded,
    /// Körper glasig, alle Kanten sichtbar.
    Wire,
}

impl ViewMode {
    pub fn label(self) -> &'static str {
        match self {
            ViewMode::ShadedEdges => "Schattiert + Kanten",
            ViewMode::Shaded => "Schattiert",
            ViewMode::Wire => "Drahtgitter",
        }
    }

    pub const ALL: [ViewMode; 3] = [ViewMode::Shaded, ViewMode::ShadedEdges, ViewMode::Wire];
}

// ---------------------------------------------------------
// Anzeigemodell
// ---------------------------------------------------------

/// Ein Körper in der Bauteilliste (Name, Sichtbarkeit).
#[derive(Clone, Debug)]
pub struct ScBodyInfo {
    pub name: String,
    pub visible: bool,
}

/// Fertiges Anzeigemodell (Szene, Kennzahlen; ausgerichtete Koordinaten, mm).
pub struct ScModel {
    pub scene: Arc<solidcraft_render::Scene>,
    pub center: Vec3,
    pub radius: f32,
    /// Bauteilmaße in ausgerichteten Koordinaten.
    pub part_size: Vec3,
    pub stock_size: Vec3,
    /// Schraubstock-Spanntiefe in Z (aus dem Kalkulationspfad).
    pub clamp_mm: f32,
    /// Planfräs-Aufmaß oben in Z (Plattensprung nach der Rundung).
    pub top_allow_mm: f32,
    /// Bauteilvolumen (mm³) — für das Materialverhältnis der UI.
    /// Signed-Tetra-Summe der Bauteildreiecke; 0.0, wenn unbestimmbar.
    pub volume_mm3: f64,
    pub triangle_count: usize,
    pub edge_count: usize,
    pub bodies: Vec<ScBodyInfo>,
    /// Die reinen Bauteil-Netze (ausgerichtet, mm) ohne Rohteil-Box und
    /// Bodenraster — Datenbasis für die Schattenkontur. Dreiecke sind
    /// über alle Netze durchnummeriert.
    pub part_positions: Vec<[f32; 3]>,
    pub part_triangles: Vec<[u32; 3]>,
}

// ---------------------------------------------------------
// STEP-Import via SolidCraft (Session)
// ---------------------------------------------------------

/// Eine geladene STEP-Datei als SolidCraft-Session.
pub struct ScSession {
    session: Session,
}

/// DisplayQuality (vor `doc.open` gesetzt) nach Rohteil-Qualität.
fn set_quality(q: crate::step::Quality) {
    use solidcraft_engine::doc::display_quality::{DisplayQuality, set_current};
    let id = match q {
        crate::step::Quality::Low => "coarse",
        crate::step::Quality::Medium => "normal",
        crate::step::Quality::High => "fine",
    };
    if let Some(level) = DisplayQuality::parse(id) {
        set_current(level);
    }
}

impl ScSession {
    /// Öffnet eine STEP-Datei über SolidCrafts eigenen STEP-Reader — genau
    /// wie in der SolidCraft-App (Befehl `doc.open`).
    pub fn open(path: &Path, quality: crate::step::Quality) -> Result<Self, String> {
        set_quality(quality);
        let mut session = Session::default();
        let params = serde_json::json!({ "path": path.display().to_string() });
        session
            .execute("doc.open", &params)
            .map_err(|e| format!("SolidCraft-Import: {e}"))?;
        Ok(Self { session })
    }

    /// Sichtbarkeit eines Körpers umschalten (dasselbe Anzeigeverhalten wie
    /// die SolidCraft-App über den Browser).
    pub fn set_visible(&mut self, name: &str, visible: bool) -> Result<(), String> {
        let params = serde_json::json!({ "items": [name], "visible": visible });
        self.session
            .execute("browser.visibility", &params)
            .map_err(|e| format!("Sichtbarkeit: {e}"))?;
        Ok(())
    }

    /// Anzeigemodell bauen: dieselbe Szenenzusammensetzung wie in
    /// `solidcraft-engine::view` (Körper in Farben, Kanten, Hintergrund),
    /// danach Rohteil-Ausrichtung und Rohteil-Box (mit deren Aufmaßen).
    pub fn model(
        &self,
        mode: ViewMode,
        show_stock: bool,
        stock_params: StockParams,
    ) -> Result<ScModel, String> {
        let s = &self.session;
        let st = s.world_state().map_err(|e| e.to_string())?;
        let p = Palette::DEFAULT;
        let mut scene = solidcraft_render::Scene {
            background: Some(p.background),
            ..Default::default()
        };
        let mut bodies = Vec::new();
        for body in st.bodies.iter() {
            let visible = s.body_visible(&body.name);
            if !visible {
                continue;
            }
            let mesh = body.mesh();
            let color = to8_color(s.doc.body_color(body)).unwrap_or(p.body);
            let face_colors: Vec<(u32, solidcraft_render::Rgb)> = s
                .doc
                .face_colors(body)
                .into_iter()
                .map(|(f, l)| (f as u32, solidcraft_render::Rgb(l.color[0], l.color[1], l.color[2])))
                .collect();
            let opacity = s.doc.model_body_opacity(body) as f32;
            let closed = body.is_closed();

            // Kantenlinien je nach Modus wie im SolidCraft-Viewport.
            let mut lines: Vec<solidcraft_render::SceneLine> = Vec::new();
            match mode {
                ViewMode::ShadedEdges => {
                    for (i, e) in mesh.edges.iter().enumerate() {
                        if !mesh.shaded_edge_visible(i) {
                            continue;
                        }
                        lines.push(solidcraft_render::SceneLine {
                            points: e.clone(),
                            color: p.edge,
                            alpha: 1.0,
                            width: 1.2,
                            on_top: false,
                        });
                    }
                }
                ViewMode::Wire => {
                    for (i, e) in mesh.edges.iter().enumerate() {
                        if !mesh.edge_visible(i) {
                            continue;
                        }
                        lines.push(solidcraft_render::SceneLine {
                            points: e.clone(),
                            color: p.edge,
                            alpha: 1.0,
                            width: 1.0,
                            on_top: false,
                        });
                    }
                }
                ViewMode::Shaded => {}
            }

            scene.meshes.push(SceneMesh {
                mesh: Arc::clone(&mesh),
                color,
                face_colors,
                opacity: match mode {
                    ViewMode::Wire => 0.12_f32,
                    _ => opacity,
                },
                closed,
                hatch: solidcraft_render::hatch::Hatch(0),
            });
            scene.lines.extend(lines);
            bodies.push(ScBodyInfo { name: body.name.clone(), visible });
        }
        build_view(scene, mode, show_stock, bodies, &stock_params)
    }
}

/// Farbe eines Körpers nach 8-bit-RGB (Look-Farben sind 0..255).
fn to8_color(c: Option<[f32; 3]>) -> Option<solidcraft_render::Rgb> {
    c.map(|[r, g, b]| {
        let to8 = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
        solidcraft_render::Rgb(to8(r), to8(g), to8(b))
    })
}

// ---------------------------------------------------------
// Szene fertig bauen: Ausrichtung + Rohteil-Box + Bodenraster
// ---------------------------------------------------------

/// Ausgerichtete Szene fertig bauen: Netzkoordinaten rigide auf die
/// Rohteil-Ausrichtung drehen (längste Achse X, unten Z = 0), Rohteilmaße
/// mit den vorgegebenen Aufmaßen, Rohteil-Box und Bodenraster, Kennzahlen.
pub fn build_view(
    mut scene: solidcraft_render::Scene,
    _mode: ViewMode,
    show_stock: bool,
    bodies: Vec<ScBodyInfo>,
    stock_params: &StockParams,
) -> Result<ScModel, String> {
    // Alle Positions-Vertices der Szene (Datei-Koordinaten) sammeln.
    let all: Vec<Vec3> = scene
        .meshes
        .iter()
        .flat_map(|m| m.mesh.positions.iter())
        .map(|p| Vec3::new(p.x as f32, p.y as f32, p.z as f32))
        .collect();
    let Some((align, part_size)) = crate::stock::alignment_for_vertices(&all) else {
        return Err("keine ausrichtbare Geometrie".to_string());
    };

    apply_align(&mut scene, &align);

    // Die Bauteil-Netze für die Schattenkontur festhalten, bevor die
    // Hilfsgeometrie (Rohteil-Box, Bodenraster) dazukommt: Dreiecke über
    // alle Netze durchnummeriert.
    let mut part_positions: Vec<[f32; 3]> = Vec::new();
    let mut part_triangles: Vec<[u32; 3]> = Vec::new();
    let mut base: u32 = 0;
    for m in &scene.meshes {
        let start = part_positions.len() as u32;
        for p in &m.mesh.positions {
            part_positions.push([p.x as f32, p.y as f32, p.z as f32]);
        }
        for t in &m.mesh.triangles {
            part_triangles.push([t[0] + start, t[1] + start, t[2] + start]);
        }
        base = part_positions.len() as u32;
    }
    let _ = base;

    // Bauteilvolumen (Signed-Tetra-Summe) — für das Materialverhältnis.
    let volume_mm3: f64 = part_triangles
        .iter()
        .map(|t| {
            let [ax, ay, az] = part_positions[t[0] as usize];
            let [bx, by, bz] = part_positions[t[1] as usize];
            let [cx, cy, cz] = part_positions[t[2] as usize];
            let (a, b, c) = (
                [f64::from(ax), f64::from(ay), f64::from(az)],
                [f64::from(bx), f64::from(by), f64::from(bz)],
                [f64::from(cx), f64::from(cy), f64::from(cz)],
            );
            // det[a b c] / 6 = a · (b × c) / 6
            (a[0] * (b[1] * c[2] - b[2] * c[1])
                + a[1] * (b[2] * c[0] - b[0] * c[2])
                + a[2] * (b[0] * c[1] - b[1] * c[0]))
                / 6.0
        })
        .sum::<f64>()
        .abs();

    // Ausgerichtete Boundingbox der Netze.
    let mut bb_min = Vec3::splat(f32::MAX);
    let mut bb_max = Vec3::splat(f32::MIN);
    for m in &scene.meshes {
        for p in &m.mesh.positions {
            let v = Vec3::new(p.x as f32, p.y as f32, p.z as f32);
            bb_min = bb_min.min(v);
            bb_max = bb_max.max(v);
        }
    }
    if bb_min.x > bb_max.x {
        bb_min = Vec3::ZERO;
        bb_max = Vec3::ZERO;
    }
    let ext = bb_max - bb_min;
    let center = (bb_min + bb_max) * 0.5;
    let radius = (ext.length() * 0.5).max(1.0);
    let triangle_count: usize = scene.meshes.iter().map(|m| m.mesh.triangles.len()).sum();

    let stock = StockResult::from_part_size_with_params(part_size, *stock_params);
    // Rohteil-Box: X/Y zentriert ums Bauteil, unten ins Spannmaß.
    let size = stock.stock_size;
    let bmin = Vec3::new(
        (part_size.x - size.x) * 0.5,
        (part_size.y - size.y) * 0.5,
        -stock.clamp_depth_z,
    );
    let bmax = bmin + size;
    if show_stock {
        add_stock_box(&mut scene, bmin, bmax, stock.clamp_depth_z);
    }
    add_ground(&mut scene, bmin, bmax);

    let edge_count: usize = scene.lines.iter().map(|l| l.points.len().saturating_sub(1)).sum();
    // Clip-Radius der Szene: wie `view::scene_with_guides` (diagonal/2, mind.
    // 10 mm) mit Puffer für den Zielabstand (hier: Ziel ≈ Zentrum).
    scene.radius = (ext.length() as f64 * 0.5).max(10.0) + radius as f64;
    Ok(ScModel {
        scene: Arc::new(scene),
        center,
        radius,
        part_size,
        stock_size: stock.stock_size,
        clamp_mm: stock.clamp_depth_z,
        top_allow_mm: stock.top_allowance_z,
        volume_mm3,
        triangle_count,
        edge_count,
        bodies,
        part_positions,
        part_triangles,
    })
}

/// Rigide Ausrichtung auf Szene anwenden (Positionen, Normalen, Kanten/Linien).
fn apply_align(scene: &mut solidcraft_render::Scene, align: &crate::stock::Alignment) {
    let to = |p: &RcVec3| align.apply(Vec3::new(p.x as f32, p.y as f32, p.z as f32));
    let to_dir = |n: &RcVec3| align.rotate_dir(Vec3::new(n.x as f32, n.y as f32, n.z as f32));
    let rc = |v: Vec3| RcVec3::new(v.x as f64, v.y as f64, v.z as f64);

    for m in &mut scene.meshes {
        let mesh = Arc::make_mut(&mut m.mesh);
        for p in &mut mesh.positions {
            *p = rc(to(p));
        }
        for n in &mut mesh.normals {
            *n = rc(to_dir(n));
        }
        for e in &mut mesh.edges {
            for p in e.iter_mut() {
                *p = rc(to(p));
            }
        }
    }
    for l in &mut scene.lines {
        for p in &mut l.points {
            *p = rc(to(p));
        }
    }
}

const STOCK_FILL: solidcraft_render::Rgb = solidcraft_render::Rgb(255, 199, 69);
const STOCK_CLAMP: solidcraft_render::Rgb = solidcraft_render::Rgb(229, 89, 89);

/// Die Schattenkontur als cyanfarbene Linie über der Szene (Vorschau).
/// Die Basis-Szene wird unverändert übernommen (Netze/Arc-Freiheiten bleiben
/// geteilt), nur die Linienliste zieht die Konterlinie als geschlossene,
/// zeichnungsober liegende Polylinie auf `z` ein.
pub fn scene_with_shadow_contour(
    base: &solidcraft_render::Scene,
    shadow: &crate::shadow::ShadowResult,
    z: f32,
) -> solidcraft_render::Scene {
    let rc = |p: [f32; 2]| {
        RcVec3::new(
            f64::from(shadow.origin[0] + p[0]),
            f64::from(shadow.origin[1] + p[1]),
            f64::from(z),
        )
    };
    let mut contour: Vec<RcVec3> = shadow.polygon.iter().map(|p| rc(*p)).collect();
    if let Some(first) = contour.first().copied() {
        contour.push(first); // geschlossen zeichnen
    }
    let preview_line = solidcraft_render::SceneLine {
        points: contour,
        color: solidcraft_render::Rgb(
            crate::ui::theme::PREVIEW.r(),
            crate::ui::theme::PREVIEW.g(),
            crate::ui::theme::PREVIEW.b(),
        ),
        alpha: 1.0,
        width: 2.5,
        on_top: true,
    };
    let mut lines: Vec<solidcraft_render::SceneLine> = base
        .lines
        .iter()
        .map(|l| solidcraft_render::SceneLine {
            points: l.points.clone(),
            color: l.color,
            alpha: l.alpha,
            width: l.width,
            on_top: l.on_top,
        })
        .collect();
    lines.push(preview_line);
    solidcraft_render::Scene {
        meshes: base
            .meshes
            .iter()
            .map(|m| SceneMesh {
                mesh: Arc::clone(&m.mesh),
                color: m.color,
                face_colors: m.face_colors.clone(),
                opacity: m.opacity,
                closed: m.closed,
                hatch: m.hatch,
            })
            .collect(),
        lines,
        grid: base
            .grid
            .iter()
            .map(|l| solidcraft_render::SceneLine {
                points: l.points.clone(),
                color: l.color,
                alpha: l.alpha,
                width: l.width,
                on_top: l.on_top,
            })
            .collect(),
        background: base.background,
        radius: base.radius,
        clip: base.clip,
        cap: base.cap,
        plain_cap: base.plain_cap,
        part_hatch: base.part_hatch,
        analysis: base.analysis,
    }
}

/// Rohteil-Box als glasiger Körper plus obere Kontur und Spannlinie.
fn add_stock_box(scene: &mut solidcraft_render::Scene, bmin: Vec3, bmax: Vec3, clamp_z: f32) {
    let rc = |v: Vec3| RcVec3::new(v.x as f64, v.y as f64, v.z as f64);
    // Rohteil als transluzenter Körper (statt Drahtgitter, das sonst sichtbar
    // durch das Werkstück scheint) — dieselbe Farbenwahl wie bisher.
    let quads: [[Vec3; 4]; 6] = [
        // unten, oben
        [Vec3::new(bmin.x, bmin.y, bmin.z), Vec3::new(bmax.x, bmin.y, bmin.z), Vec3::new(bmax.x, bmax.y, bmin.z), Vec3::new(bmin.x, bmax.y, bmin.z)],
        [Vec3::new(bmin.x, bmin.y, bmax.z), Vec3::new(bmax.x, bmin.y, bmax.z), Vec3::new(bmax.x, bmax.y, bmax.z), Vec3::new(bmin.x, bmax.y, bmax.z)],
        // vorne, hinten
        [Vec3::new(bmin.x, bmin.y, bmin.z), Vec3::new(bmax.x, bmin.y, bmin.z), Vec3::new(bmax.x, bmin.y, bmax.z), Vec3::new(bmin.x, bmin.y, bmax.z)],
        [Vec3::new(bmin.x, bmax.y, bmin.z), Vec3::new(bmax.x, bmax.y, bmin.z), Vec3::new(bmax.x, bmax.y, bmax.z), Vec3::new(bmin.x, bmax.y, bmax.z)],
        // links, rechts
        [Vec3::new(bmin.x, bmin.y, bmin.z), Vec3::new(bmin.x, bmax.y, bmin.z), Vec3::new(bmin.x, bmax.y, bmax.z), Vec3::new(bmin.x, bmin.y, bmax.z)],
        [Vec3::new(bmax.x, bmin.y, bmin.z), Vec3::new(bmax.x, bmax.y, bmin.z), Vec3::new(bmax.x, bmax.y, bmax.z), Vec3::new(bmax.x, bmin.y, bmax.z)],
    ];
    let mut mesh = solidcraft_geom::Mesh::default();
    for (tri, q) in (0_u32..).zip(quads.iter()) {
        let a = rc(q[0]);
        let b = rc(q[1]);
        let c = rc(q[2]);
        let d = rc(q[3]);
        let n = (b - a).cross(c - a);
        let base = mesh.positions.len() as u32;
        for v in [a, b, c, d] {
            mesh.positions.push(v);
            mesh.normals.push(n.normalized().unwrap_or(RcVec3::Z));
        }
        mesh.triangles.push([base, base + 1, base + 2]);
        mesh.triangles.push([base, base + 2, base + 3]);
        mesh.tri_face.extend([tri; 2]);
    }
    scene.meshes.push(SceneMesh {
        mesh: Arc::new(mesh),
        color: STOCK_FILL,
        face_colors: Vec::new(),
        // unter SEE_THROUGH → glasig: das Rohteil bleibt lesbar, das Teil drin.
        opacity: 0.28,
        closed: false,
        hatch: solidcraft_render::hatch::Hatch(0),
    });

    // Nur die obere Kontur und die Spannlinie (Z 0) sind als Linien sinnvoll.
    let top = [
        Vec3::new(bmin.x, bmin.y, bmax.z),
        Vec3::new(bmax.x, bmin.y, bmax.z),
        Vec3::new(bmax.x, bmax.y, bmax.z),
        Vec3::new(bmin.x, bmax.y, bmax.z),
    ];
    for i in 0..4 {
        scene.lines.push(solidcraft_render::SceneLine {
            points: vec![rc(top[i]), rc(top[(i + 1) % 4])],
            color: STOCK_FILL,
            alpha: 1.0,
            width: 1.5,
            on_top: false,
        });
    }
    let clamp = [
        Vec3::new(bmin.x, bmin.y, 0.0),
        Vec3::new(bmax.x, bmin.y, 0.0),
        Vec3::new(bmax.x, bmax.y, 0.0),
        Vec3::new(bmin.x, bmax.y, 0.0),
    ];
    for i in 0..4 {
        scene.lines.push(solidcraft_render::SceneLine {
            points: vec![rc(clamp[i]), rc(clamp[(i + 1) % 4])],
            color: STOCK_CLAMP,
            alpha: 1.0,
            width: 1.5,
            on_top: false,
        });
    }
    let _ = clamp_z;
}

/// Feines Bodenraster auf Z 0 unter dem Bauteil, im SolidCraft-Stil
/// (feine Nebelinien, kräftigere Hauptlinien alle 100 mm).
fn add_ground(scene: &mut solidcraft_render::Scene, bmin: Vec3, bmax: Vec3) {
    let rc = |v: Vec3| RcVec3::new(v.x as f64, v.y as f64, v.z as f64);
    let margin = 30.0_f32;
    let step = 10.0_f32;
    let major = 100.0_f32;
    let snap = |v: f32| (v / step).floor() * step;
    let (x0, x1) = (snap(bmin.x - margin), snap(bmax.x + margin));
    let (y0, y1) = (snap(bmin.y - margin), snap(bmax.y + margin));
    let color_minor = solidcraft_render::Rgb(170, 180, 192);
    let color_major = solidcraft_render::Rgb(146, 158, 174);
    let mut x = x0;
    while x <= x1 {
        let is_major = (x / major).round() * major == x;
        scene.grid.push(solidcraft_render::SceneLine {
            points: vec![rc(Vec3::new(x, y0, 0.0)), rc(Vec3::new(x, y1, 0.0))],
            color: if is_major { color_major } else { color_minor },
            alpha: if is_major { 0.55 } else { 0.3 },
            width: if is_major { 1.2 } else { 0.85 },
            on_top: false,
        });
        x += step;
    }
    let mut y = y0;
    while y <= y1 {
        let is_major = (y / major).round() * major == y;
        scene.grid.push(solidcraft_render::SceneLine {
            points: vec![rc(Vec3::new(x0, y, 0.0)), rc(Vec3::new(x1, y, 0.0))],
            color: if is_major { color_major } else { color_minor },
            alpha: if is_major { 0.55 } else { 0.3 },
            width: if is_major { 1.2 } else { 0.85 },
            on_top: false,
        });
        y += step;
    }
}

// ---------------------------------------------------------
// Demo-Modell
// ---------------------------------------------------------

/// Demo-/Rohteilnetz als Szenenmodell (kein Session-Import nötig): Körper in
/// Stahl-Farbe, Feature-Kanten als Linien, gleiche Ausrichtungskette wie STEP.
pub fn demo_scene(
    mesh: MeshData,
    mode: ViewMode,
    show_stock: bool,
    stock_params: &StockParams,
) -> Result<ScModel, String> {
    let p = Palette::DEFAULT;
    let mut scene = solidcraft_render::Scene {
        background: Some(p.background),
        ..Default::default()
    };
    let sm = mesh_data_to_mesh(&mesh);
    if mode != ViewMode::Shaded {
        for (i, e) in sm.edges.iter().enumerate() {
            let smooth = sm.smooth_edges.get(i).copied().unwrap_or(false);
            let drawn = match mode {
                ViewMode::ShadedEdges => !smooth,
                ViewMode::Wire => true,
                ViewMode::Shaded => false,
            };
            if !drawn {
                continue;
            }
            scene.lines.push(solidcraft_render::SceneLine {
                points: e.clone(),
                color: p.edge,
                alpha: 1.0,
                width: match mode {
                    ViewMode::Wire => 1.0,
                    _ => 1.2,
                },
                on_top: false,
            });
        }
    }
    let opacity = match mode {
        ViewMode::Wire => 0.12_f32,
        _ => 1.0_f32,
    };
    scene.meshes.push(SceneMesh {
        mesh: Arc::new(sm),
        color: solidcraft_render::Rgb(90, 96, 105),
        face_colors: Vec::new(),
        opacity,
        closed: true,
        hatch: solidcraft_render::hatch::Hatch(0),
    });
    build_view(scene, mode, show_stock, Vec::new(), stock_params)
}

/// Rohteilnetz (MeshData, pro-Eck-Normalen) in SolidCrafts Mesh-Typ mit
/// vertexindizierten, weich gemittelten Normalen überführen.
fn mesh_data_to_mesh(m: &MeshData) -> solidcraft_geom::Mesh {
    let mut out = solidcraft_geom::Mesh {
        positions: m
            .vertices
            .iter()
            .map(|v| RcVec3::new(v.x as f64, v.y as f64, v.z as f64))
            .collect(),
        ..solidcraft_geom::Mesh::default()
    };
    out.triangles = m
        .triangles
        .iter()
        .map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
        .collect();
    // Weiche Normalen: Flächennormalen über die Ecken summieren, normalisieren.
    let mut sums = vec![RcVec3::ZERO; out.positions.len()];
    for t in &out.triangles {
        let Some(a) = out.positions.get(t[0] as usize).copied() else { continue };
        let Some(b) = out.positions.get(t[1] as usize).copied() else { continue };
        let Some(c) = out.positions.get(t[2] as usize).copied() else { continue };
        let n = (b - a).cross(c - a);
        for idx in t {
            if let Some(s) = sums.get_mut(*idx as usize) {
                *s += n;
            }
        }
    }
    out.normals = sums
        .iter()
        .map(|n| n.normalized().unwrap_or(RcVec3::Z))
        .collect();
    out.tri_face = (0..out.triangles.len()).map(|i| i as u32).collect();
    let mut edges: Vec<Vec<RcVec3>> = Vec::new();
    for [a, b] in &m.feature_edges {
        if let (Some(pa), Some(pb)) = (m.vertices.get(*a).copied(), m.vertices.get(*b).copied()) {
            edges.push(vec![
                RcVec3::new(pa.x as f64, pa.y as f64, pa.z as f64),
                RcVec3::new(pb.x as f64, pb.y as f64, pb.z as f64),
            ]);
        }
    }
    out.edges = edges;
    out.display_only_edges = false;
    out
}

// ---------------------------------------------------------
// Kamera-Mapping
// ---------------------------------------------------------

/// Rohteil-Orbitkamera → SolidCraft-Renderkamera. SolidCraft misst yaw als
/// Blickrichtung im Z-up-Rahmen (back = (−sin yaw, −cos yaw, …)); Rohteil
/// legt die Auge-Richtung als (sin yaw, −cos yaw, …) an → yaw-Vorzeichen
/// kippen. Genau dieselbe Kamera wie in `solidcraft_engine::view`.
pub fn render_camera(c: &OrbitCamera) -> RenderCamera {
    RenderCamera {
        target: RcVec3::new(c.target.x as f64, c.target.y as f64, c.target.z as f64),
        yaw: -(c.yaw as f64),
        pitch: c.pitch as f64,
        // Roll ist in beiden Rahmen gleich definiert (die Grundbasis stimmte
        // auch nach dem yaw-Kipp schon).
        roll: c.roll as f64,
        distance: c.distance as f64,
        fov: c.fov_y as f64,
        ..Default::default()
    }
}

// ---------------------------------------------------------
// GPU-Viewport (egui-Callback, dieselben Pipelines wie SolidCraft)
// ---------------------------------------------------------

/// Geteilte Szene für den Frame: gleiches Schlüssel → dasselbe GPU-Upload
/// wiederverwendet; nur Kamera/Größe aktualisieren sich pro Bild.
pub struct SceneState {
    pub slot: Arc<Mutex<Option<GpuScene>>>,
    pub key: u64,
}

impl SceneState {
    /// Szene für eine neue Revision anlegen (Neu-Upload beim nächsten Bild).
    pub fn new(scene: &solidcraft_render::Scene) -> Self {
        Self {
            slot: Arc::new(Mutex::new(Some(solidcraft_gpu::capture::scene_geometry(scene)))),
            key: 0,
        }
    }

    /// Nächster eindeutiger Schlüssel (Revision).
    pub fn set_key(&mut self, key: u64) {
        self.key = key;
    }
}

/// Frame-Aufbau wie `solidcraft_gpu::capture::frame` — dieselben Pipelines,
/// kein Produkt-/Schnitt-/Analyse-Kontext wie hier nötig.
pub fn make_frame(
    state: &SceneState,
    model: &ScModel,
    cam: &RenderCamera,
    size_px: [f32; 2],
    moving: bool,
) -> Frame {
    let aspect = size_px[0] / size_px[1].max(1.0);
    let eye = cam.eye().to_f32();
    let empty = || Arc::new(Mutex::new(None));
    Frame {
        product_lights: None,
        product: false,
        upload_error: None,
        key: state.key,
        slot: Arc::clone(&state.slot),
        sketch_key: 0,
        sketch_slot: empty(),
        hl_key: 0,
        hl_slot: empty(),
        pv_key: 0,
        pv_slot: empty(),
        pv_hide: false,
        view_proj: cam
            .view_proj(
                aspect as f64,
                (model.scene.radius.max((model.radius as f64) * 1.05)).max(1.0),
            )
            .to_f32(),
        back: cam.back().to_f32(),
        size_px,
        clip: [0.0; 4],
        cap: [214.0 / 255.0, 128.0 / 255.0, 72.0 / 255.0, 1.0],
        images: Vec::new(),
        clouds: Vec::new(),
        analysis: analysis_uniform(None, None),
        analysis_colors: [0.0; 36],
        access: None,
        outline: true,
        eye: [eye[0], eye[1], eye[2], if cam.fov > 0.0 { 1.0 } else { 0.0 }],
        ao_radius: 0.0,
        ao_samples: if moving {
            solidcraft_gpu::view::AO_SAMPLES_MOVING
        } else {
            solidcraft_gpu::view::AO_SAMPLES
        },
        sky: [0.0; 4],
        ssaa: 1.0,
        hover_tint: [0.0; 3],
        selection_tint: [0.0; 3],
        hatch: Default::default(),
    }
}

/// egui-Paint-Callback wie in SolidCrafts `ui-egui::gpu::ViewportCallback`.
pub struct ViewCallback(pub Frame);

impl egui_wgpu::CallbackTrait for ViewCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(res) = resources.get_mut::<Renderer>() {
            res.prepare(device, queue, encoder, &self.0);
        }
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(res) = resources.get::<Renderer>() else { return };
        let vp = info.viewport_in_pixels();
        res.paint(
            &self.0,
            pass,
            [
                vp.left_px as f32,
                vp.top_px as f32,
                vp.width_px as f32,
                vp.height_px as f32,
            ],
        );
    }
}

/// Ein nicht abgefangener GPU-Fehler wird geloggt, nie ein Panic (wie bei
/// SolidCraft #5486): ein invalider Aufruf darf die App nicht schließen.
fn log_uncaptured_errors(device: &wgpu::Device) {
    use std::sync::atomic::{AtomicU32, Ordering};
    let reported = Arc::new(AtomicU32::new(0));
    device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
        if reported.fetch_add(1, Ordering::Relaxed) < 8 {
            eprintln!("GPU: {e}");
        }
    }));
}

/// Pipelines einmal im Creation-Context installieren (Szene offscreen mit
/// Tiefenpuffer und MSAA, Blit ins egui-Fenster — wie SolidCraft).
pub fn install(cc: &eframe::CreationContext<'_>) -> Result<(), String> {
    let Some(rs) = cc.wgpu_render_state.as_ref() else {
        return Err("kein wgpu-RenderState (eframe ohne Wgpu?)".to_string());
    };
    log_uncaptured_errors(&rs.device);
    let depth = solidcraft_gpu::depth_format(&rs.device, true);
    let samples = solidcraft_gpu::capture::SAMPLES;
    let scene = GpuTarget {
        format: rs.target_format,
        depth: Some(depth),
        samples,
        tess: false,
        oit: false,
    };
    let window = GpuTarget {
        format: rs.target_format,
        depth: None,
        samples: 1,
        tess: false,
        oit: false,
    };
    let res = Renderer::with_window_target(&rs.device, scene, window, None);
    rs.renderer.write().callback_resources.insert(res);
    Ok(())
}

// ---------------------------------------------------------
// Offscreen-Render für --render (desselbe Bild wie der Viewport)
// ---------------------------------------------------------

/// Szene mit Kamera in `width × height` px (RGBA) rendern — die
/// SolidCraft-Headless-Pipelines (`SOLIDCRAFT_GPU` regelt das Adapter-Wahl).
pub fn render_offscreen(
    scene: &solidcraft_render::Scene,
    cam: &RenderCamera,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let (rgba, renderer) = solidcraft_gpu::capture::headless_rgba(scene, cam, width, height)?;
    let _ = renderer;
    Ok(rgba)
}

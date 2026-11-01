use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use glam::{Mat3, Vec3};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use truck_meshalgo::tessellation::RobustMeshableShape;
use truck_stepio::r#in::Table;

// ---------------------------------------------------------
// 1. Mathematische Ausrichtung & Rohteillogik
// ---------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct MeshData {
    pub vertices: Vec<Vec3>,
    pub triangles: Vec<[usize; 3]>,
    /// Geglättete Flächennormale je Dreiecksecke (parallel zu `triangles`).
    /// Damit werden gekrümmte Flächen (z. B. Bohrungswände) weich schattiert,
    /// während echte Kanten scharf bleiben.
    pub normals: Vec<[Vec3; 3]>,
}

/// Berechnet geglättete Eckennormalen. Flächen, die unter einem Winkel von
/// weniger als ca. 40° aufeinandertreffen, gelten als eine glatte Oberfläche
/// (Zylinder-/Kegelwand). Echte Kanten (>= 40°) bleiben scharf, damit z. B.
/// die Deckfläche nicht in die Bohrungswand "verläuft".
fn compute_corner_normals(vertices: &[Vec3], triangles: &[[usize; 3]]) -> Vec<[Vec3; 3]> {
    const SMOOTH_COS: f32 = 0.766; // cos(40°)

    let face_normals: Vec<Vec3> = triangles
        .iter()
        .map(|t| {
            let (a, b, c) = (vertices[t[0]], vertices[t[1]], vertices[t[2]]);
            (b - a).cross(c - a).normalize_or_zero()
        })
        .collect();

    let mut incident: Vec<Vec<usize>> = vec![Vec::new(); vertices.len()];
    for (ti, t) in triangles.iter().enumerate() {
        for &vi in t {
            incident[vi].push(ti);
        }
    }

    let mut out = Vec::with_capacity(triangles.len());
    for (ti, t) in triangles.iter().enumerate() {
        let n_ref = face_normals[ti];
        let mut corner = [n_ref; 3];
        for (k, &vi) in t.iter().enumerate() {
            let mut sum = Vec3::ZERO;
            for &tj in &incident[vi] {
                // truck liefert teils uneinheitliche Windungen: vor dem
                // Mitteln an der Referenznormale ausrichten.
                let mut n = face_normals[tj];
                if n.dot(n_ref) < 0.0 {
                    n = -n;
                }
                if n.dot(n_ref) >= SMOOTH_COS {
                    sum += n;
                }
            }
            corner[k] = if sum.length_squared() > 1e-12 {
                sum.normalize()
            } else {
                n_ref
            };
        }
        out.push(corner);
    }
    out
}

#[derive(Clone, Debug)]
pub struct StockResult {
    pub part_size: Vec3,      // [Länge (X), Breite (Y), Höhe (Z)]
    pub stock_size: Vec3,     // [Roh-X, Roh-Y, Roh-Z]
    pub clamp_depth_z: f32,   // 4.0 mm Spannrand unten
    pub top_allowance_z: f32, // min. 1.0 mm Planaufmaß oben
    pub transform: Mat3,      // Ausrichtungsmatrix
}

/// Ermittelt die Hauptachsen (Trägheitsachsen/Kovarianz) und richtet das Bauteil aus:
/// Längste Seite -> X | Mittlere Seite -> Y | Kleinste Seite -> Z
pub fn orient_and_align_mesh(mesh: &mut MeshData) -> StockResult {
    if mesh.vertices.is_empty() {
        return StockResult {
            part_size: Vec3::ZERO,
            stock_size: Vec3::ZERO,
            clamp_depth_z: 4.0,
            top_allowance_z: 1.0,
            transform: Mat3::IDENTITY,
        };
    }

    // 1. Schwerpunkt (Centroid) berechnen
    let n = mesh.vertices.len() as f32;
    let centroid = mesh.vertices.iter().fold(Vec3::ZERO, |acc, &v| acc + v) / n;

    // 2. Kovarianzmatrix für Hauptträgheitsachsen berechnen
    let mut cov = Mat3::ZERO;
    for v in &mesh.vertices {
        let p = *v - centroid;
        cov.x_axis += Vec3::new(p.x * p.x, p.x * p.y, p.x * p.z);
        cov.y_axis += Vec3::new(p.y * p.x, p.y * p.y, p.y * p.z);
        cov.z_axis += Vec3::new(p.z * p.x, p.z * p.y, p.z * p.z);
    }
    cov.x_axis /= n;
    cov.y_axis /= n;
    cov.z_axis /= n;

    // Einfache Eigenvektor-Approximation für CAD-Bounding-Boxes:
    let candidates = [
        Vec3::X,
        Vec3::Y,
        Vec3::Z,
        (cov.x_axis).normalize_or_zero(),
        (cov.y_axis).normalize_or_zero(),
        (cov.z_axis).normalize_or_zero(),
    ];

    // Startwert: achsenparallele Bounding Box
    let mut bb_min = Vec3::splat(f32::MAX);
    let mut bb_max = Vec3::splat(f32::MIN);
    for v in &mesh.vertices {
        bb_min = bb_min.min(*v);
        bb_max = bb_max.max(*v);
    }
    let mut best_extents = bb_max - bb_min;
    let mut best_axes = (Vec3::X, Vec3::Y, Vec3::Z);

    for &ax in &candidates {
        if ax.length_squared() < 0.1 {
            continue;
        }
        for &ay in &candidates {
            if ay.length_squared() < 0.1 || ax.dot(ay).abs() > 0.9 {
                continue;
            }
            let az = ax.cross(ay).normalize_or_zero();
            if az.length_squared() < 0.1 {
                continue;
            }
            let ay_ortho = az.cross(ax).normalize();

            let (mut min_b, mut max_b) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for v in &mesh.vertices {
                let proj = Vec3::new(v.dot(ax), v.dot(ay_ortho), v.dot(az));
                min_b = min_b.min(proj);
                max_b = max_b.max(proj);
            }
            let ext = max_b - min_b;
            if (ext.x * ext.y * ext.z) < (best_extents.x * best_extents.y * best_extents.z) {
                best_extents = ext;
                best_axes = (ax, ay_ortho, az);
            }
        }
    }

    // Sortiere Achsen nach Ausdehnungsgröße:
    // Längste -> X, mittlere -> Y, kürzeste -> Z
    let mut dims = [
        (best_extents.x, best_axes.0),
        (best_extents.y, best_axes.1),
        (best_extents.z, best_axes.2),
    ];
    dims.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let new_x = dims[0].1;
    let new_y_tmp = dims[1].1;
    let new_z = new_x.cross(new_y_tmp).normalize_or_zero();
    let new_y = new_z.cross(new_x).normalize_or_zero(); // Rechtssystem sicherstellen

    let rot = Mat3::from_cols(new_x, new_y, new_z).transpose();

    // Vertices rotieren und Nullpunkt auf (min_x, min_y, 0) setzen
    let mut min_bound = Vec3::splat(f32::MAX);
    let mut max_bound = Vec3::splat(f32::MIN);

    for v in &mut mesh.vertices {
        *v = rot * (*v - centroid);
        min_bound = min_bound.min(*v);
        max_bound = max_bound.max(*v);
    }

    // Nullpunkt ausrichten: Unterkante Z = 0
    let offset = Vec3::new(min_bound.x, min_bound.y, min_bound.z);
    for v in &mut mesh.vertices {
        *v -= offset;
    }

    let part_size = max_bound - min_bound;

    // -------------------------------------------------------------
    // Fertigungsregeln für das Rohteil:
    // -------------------------------------------------------------
    // 1. In X: Bauteillänge + 5 mm
    let stock_x = part_size.x + 5.0;

    // 2. In Y: Flachmaterial-Raster
    // Bis Bauteilbreite 25 in 5mm Schritten, ab 30 in 10mm Schritten
    let stock_y = calculate_standard_y_stock(part_size.y);

    // 3. In Z: Standard-Plattenstärke in 10-mm-Stufen (10, 20, 30, ...),
    //    muss mind. 4 mm Spannrand + 1 mm Planaufmaß enthalten.
    let stock_z = calculate_standard_z_stock(part_size.z);
    let top_allowance_z = stock_z - part_size.z - 4.0;

    // Nach Rotation/Translation Normalen neu berechnen.
    mesh.normals = compute_corner_normals(&mesh.vertices, &mesh.triangles);

    StockResult {
        part_size,
        stock_size: Vec3::new(stock_x, stock_y, stock_z),
        clamp_depth_z: 4.0,
        top_allowance_z,
        transform: rot,
    }
}

/// Berechnet die nächste Standard-Plattenstärke in Z:
/// 10, 20, 30, 40, 50, ... (10-mm-Stufen)
fn calculate_standard_z_stock(z_part: f32) -> f32 {
    let min_allowance = 5.0; // 4 mm Spannrand unten + 1 mm Planaufmaß oben
    ((z_part + min_allowance) / 10.0).ceil() * 10.0
}

/// Berechnet die nächste Standard-Halbzeugbreite in Y:
/// bis 25mm in 5er Schritten, ab 30mm in 10er Schritten
fn calculate_standard_y_stock(y_part: f32) -> f32 {
    let min_allowance = 1.0; // Mindestaufmaß zum Überfräsen der Flanken
    let required = y_part + min_allowance;

    if required <= 25.0 {
        // 5mm Stufen: 10, 15, 20, 25
        (required / 5.0).ceil() * 5.0
    } else if required <= 30.0 {
        30.0
    } else {
        // Ab 30mm: 10mm Stufen: 40, 50, 60, ...
        (required / 10.0).ceil() * 10.0
    }
}

// ---------------------------------------------------------
// 2. STEP / 3D-Geometrie Loader
// ---------------------------------------------------------

/// Lädt eine STEP-Datei, wandelt die B-rep-Flächen in Dreiecke um
/// und gibt sie als Mesh zurück.
pub fn load_step_mesh(path: &Path) -> Result<MeshData, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Datei konnte nicht gelesen werden: {e}"))?;

    let table = Table::from_step(&content)
        .ok_or_else(|| "STEP-Datei konnte nicht geparst werden (ungültiges Format?)".to_string())?;

    if table.shell.is_empty() {
        return Err("Keine B-rep-Shell in der STEP-Datei gefunden.".to_string());
    }

    let mut vertices: Vec<Vec3> = Vec::new();
    let mut triangles: Vec<[usize; 3]> = Vec::new();
    let mut last_error: Option<String> = None;

    for shell in table.shell.values() {
        let cshell = match table.to_compressed_shell(shell) {
            Ok(c) => c,
            Err(e) => {
                last_error = Some(format!("{e:?}"));
                continue;
            }
        };

        // Referenz-Bounding-Box der Shell: Dreieckspunkte, die deutlich
        // außerhalb liegen, stammen aus fehlerhaften STEP-Flächen (Spikes)
        // und werden verworfen.
        let mut shell_min = Vec3::splat(f32::MAX);
        let mut shell_max = Vec3::splat(f32::MIN);
        for p in &cshell.vertices {
            let v = Vec3::new(p.x as f32, p.y as f32, p.z as f32);
            shell_min = shell_min.min(v);
            shell_max = shell_max.max(v);
        }
        let tolerance = ((shell_max - shell_min).length() * 0.01).max(0.5);
        let inside = |p: Vec3| {
            p.x >= shell_min.x - tolerance
                && p.x <= shell_max.x + tolerance
                && p.y >= shell_min.y - tolerance
                && p.y <= shell_max.y + tolerance
                && p.z >= shell_min.z - tolerance
                && p.z <= shell_max.z + tolerance
        };

        // 0.05 mm Toleranz: fein genug für glatte Rundungen, ohne die
        // Dreieckszahl unnötig aufzublähen.
        let tessellated = cshell.robust_triangulation(0.05);

        for face in tessellated.faces {
            let Some(poly) = face.surface else {
                continue;
            };
            let base = vertices.len();
            let positions = poly.positions();
            vertices.extend(
                positions
                    .iter()
                    .map(|p| Vec3::new(p.x as f32, p.y as f32, p.z as f32)),
            );

            let world = |idx: usize| {
                let p = positions[idx];
                Vec3::new(p.x as f32, p.y as f32, p.z as f32)
            };
            let mut push_tri = |a: usize, b: usize, c: usize| {
                let (pa, pb, pc) = (world(a), world(b), world(c));
                if inside(pa) && inside(pb) && inside(pc) {
                    triangles.push([base + a, base + b, base + c]);
                }
            };

            let faces = poly.faces();
            for tri in faces.tri_faces() {
                push_tri(tri[0].pos, tri[1].pos, tri[2].pos);
            }
            for quad in faces.quad_faces() {
                push_tri(quad[0].pos, quad[1].pos, quad[2].pos);
                push_tri(quad[0].pos, quad[2].pos, quad[3].pos);
            }
            for other in faces.other_faces() {
                for i in 1..other.len().saturating_sub(1) {
                    push_tri(other[0].pos, other[i].pos, other[i + 1].pos);
                }
            }
        }
    }

    if triangles.is_empty() {
        let hint = last_error
            .map(|e| format!(" Letzter Konvertierungsfehler: {e}"))
            .unwrap_or_default();
        Err(format!(
            "STEP enthielt keine triangulierbaren Flächen.{hint}"
        ))
    } else {
        Ok(weld_and_clean(vertices, triangles))
    }
}

/// Verschmilzt räumlich identische Vertices (z. B. an Nahtstellen zwischen
/// Flächen) und entfernt danach entartete Dreiecke. Das schließt Haarrisse,
/// die sonst durch getrennte Vertex-Kopien entstehen.
fn weld_and_clean(vertices: Vec<Vec3>, triangles: Vec<[usize; 3]>) -> MeshData {
    const GRID: f32 = 1000.0; // 0.001 mm Rasterweite

    let mut map: HashMap<(i64, i64, i64), usize> = HashMap::with_capacity(vertices.len());
    let mut remap = vec![usize::MAX; vertices.len()];
    let mut welded: Vec<Vec3> = Vec::with_capacity(vertices.len());

    for (i, v) in vertices.iter().enumerate() {
        let key = (
            (v.x * GRID).round() as i64,
            (v.y * GRID).round() as i64,
            (v.z * GRID).round() as i64,
        );
        let idx = *map.entry(key).or_insert_with(|| {
            welded.push(*v);
            welded.len() - 1
        });
        remap[i] = idx;
    }

    let mut cleaned = Vec::with_capacity(triangles.len());
    for tri in triangles {
        let a = remap[tri[0]];
        let b = remap[tri[1]];
        let c = remap[tri[2]];
        if a == b || b == c || a == c {
            continue;
        }
        let (pa, pb, pc) = (welded[a], welded[b], welded[c]);
        let area2 = (pb - pa).cross(pc - pa).length();
        if !area2.is_finite() || area2 < 1e-9 {
            continue;
        }
        cleaned.push([a, b, c]);
    }

    compact_mesh(welded, cleaned)
}

/// Entfernt nicht mehr verwendete Vertices (z. B. von verworfenen Spikes)
/// und kompaktiert die Indizes.
fn compact_mesh(vertices: Vec<Vec3>, triangles: Vec<[usize; 3]>) -> MeshData {
    let mut remap = vec![usize::MAX; vertices.len()];
    let mut new_vertices = Vec::with_capacity(vertices.len());
    let mut new_triangles = Vec::with_capacity(triangles.len());

    for tri in triangles {
        let mut mapped = [0usize; 3];
        for (k, &old) in tri.iter().enumerate() {
            if remap[old] == usize::MAX {
                remap[old] = new_vertices.len();
                new_vertices.push(vertices[old]);
            }
            mapped[k] = remap[old];
        }
        new_triangles.push(mapped);
    }

    let mut mesh = MeshData {
        vertices: new_vertices,
        triangles: new_triangles,
        normals: Vec::new(),
    };
    mesh.normals = compute_corner_normals(&mesh.vertices, &mesh.triangles);
    mesh
}

fn generate_sample_milled_part(lx: f32, ly: f32, lz: f32) -> MeshData {
    // Generiert ein Prisma mit 8 Eckpunkten
    let v = vec![
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(lx, 0.0, 0.0),
        Vec3::new(lx, ly, 0.0),
        Vec3::new(0.0, ly, 0.0),
        Vec3::new(0.0, 0.0, lz),
        Vec3::new(lx, 0.0, lz),
        Vec3::new(lx, ly, lz),
        Vec3::new(0.0, ly, lz),
    ];
    let triangles = vec![
        [0, 1, 2],
        [0, 2, 3], // Boden
        [4, 6, 5],
        [4, 7, 6], // Deckel
        [0, 4, 5],
        [0, 5, 1], // Vorne
        [2, 6, 7],
        [2, 7, 3], // Hinten
        [0, 3, 7],
        [0, 7, 4], // Links
        [1, 5, 6],
        [1, 6, 2], // Rechts
    ];
    let mut mesh = MeshData {
        vertices: v,
        triangles,
        normals: Vec::new(),
    };
    mesh.normals = compute_corner_normals(&mesh.vertices, &mesh.triangles);
    mesh
}

// ---------------------------------------------------------
// 3. UI & 3D-Viewport (egui)
// ---------------------------------------------------------

pub struct AppState {
    mesh: MeshData,
    stock: StockResult,
    file_path: Option<PathBuf>,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    status: String,
    status_is_error: bool,
    show_stock: bool,
    /// Zwischengespeichertes, software-gerastertes Bild des Bauteils.
    scene: Option<SceneCache>,
    revision: u64,
}

/// Schlüssel, um zu erkennen, wann das gerasterte Bild neu berechnet werden muss.
#[derive(Clone, Copy, PartialEq)]
struct SceneCacheKey {
    width: u32,
    height: u32,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    revision: u64,
}

struct SceneCache {
    key: SceneCacheKey,
    texture: egui::TextureHandle,
}

impl AppState {
    pub fn new(initial: Option<PathBuf>) -> Self {
        let mut mesh = generate_sample_milled_part(110.0, 42.0, 18.0);
        let stock = orient_and_align_mesh(&mut mesh);
        let mut app = Self {
            mesh,
            stock,
            file_path: None,
            yaw: -0.72,
            pitch: 0.42,
            zoom: 1.0,
            status: "Demo-Bauteil aktiv – STEP-Datei über den Button laden.".to_string(),
            status_is_error: false,
            show_stock: true,
            scene: None,
            revision: 0,
        };
        if let Some(path) = initial {
            app.load_from_path(path);
        }
        app
    }

    fn reset_camera(&mut self) {
        self.yaw = -0.72;
        self.pitch = 0.42;
        self.zoom = 1.0;
    }

    fn load_demo(&mut self) {
        let mut mesh = generate_sample_milled_part(110.0, 42.0, 18.0);
        self.stock = orient_and_align_mesh(&mut mesh);
        self.mesh = mesh;
        self.file_path = None;
        self.revision += 1;
        self.status = "Demo-Bauteil geladen.".to_string();
        self.status_is_error = false;
        self.reset_camera();
    }

    fn load_from_path(&mut self, path: PathBuf) {
        // Ein fehlerhaftes STEP-File darf die App nicht zum Absturz bringen.
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| load_step_mesh(&path)))
                .unwrap_or_else(|_| {
                    Err(
                        "Beim Verarbeiten der STEP-Geometrie ist ein interner Fehler aufgetreten."
                            .to_string(),
                    )
                });

        match result {
            Ok(mut mesh) => {
                if mesh.vertices.is_empty() || mesh.triangles.is_empty() {
                    self.status = "STEP-Datei enthält keine anzeigbare Geometrie.".to_string();
                    self.status_is_error = true;
                    return;
                }
                let tri_count = mesh.triangles.len();
                self.stock = orient_and_align_mesh(&mut mesh);
                self.mesh = mesh;
                self.file_path = Some(path);
                self.revision += 1;
                self.status = format!(
                    "STEP geladen: {tri_count} Dreiecke, Bauteil {:.1} × {:.1} × {:.1} mm",
                    self.stock.part_size.x, self.stock.part_size.y, self.stock.part_size.z
                );
                self.status_is_error = false;
                self.reset_camera();
            }
            Err(err) => {
                self.status = format!("STEP konnte nicht geladen werden: {err}");
                self.status_is_error = true;
            }
        }
    }
}

impl eframe::App for AppState {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drag & Drop: die zuletzt abgelegte Datei laden.
        let dropped = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .find_map(|file| file.path.clone())
        });
        if let Some(path) = dropped {
            self.load_from_path(path);
            ctx.request_repaint();
        }

        egui::SidePanel::left("control_panel")
            .min_width(330.0)
            .resizable(true)
            .show(ctx, |ui| {
                // Nur die Schrift der linken Leiste doppelt so groß.
                for font_id in ui.style_mut().text_styles.values_mut() {
                    font_id.size *= 2.0;
                }

                ui.heading("📦 Rohteil-Kalkulation (CNC)");
                ui.separator();

                ui.horizontal(|ui| {
                    if ui.button("📂 STEP-Datei öffnen...").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("STEP CAD", &["step", "stp", "Step", "STP", "SteP"])
                            .pick_file()
                        {
                            self.load_from_path(path);
                            ctx.request_repaint();
                        }
                    }
                    if ui.button("🧱 Demo").clicked() {
                        self.load_demo();
                    }
                });

                if let Some(p) = &self.file_path {
                    ui.label(format!(
                        "Datei: {}",
                        p.file_name().unwrap_or_default().to_string_lossy()
                    ));
                } else {
                    ui.label("Datei: –");
                }

                if !self.status.is_empty() {
                    let color = if self.status_is_error {
                        Color32::from_rgb(255, 120, 110)
                    } else {
                        Color32::from_rgb(120, 210, 150)
                    };
                    ui.label(egui::RichText::new(&self.status).color(color));
                }

                ui.separator();
                ui.label(egui::RichText::new("1. Bauteilmaße (ausgerichtet)").strong());
                ui.label(format!("X (Länge):  {:.2} mm", self.stock.part_size.x));
                ui.label(format!("Y (Breite): {:.2} mm", self.stock.part_size.y));
                ui.label(format!("Z (Höhe):   {:.2} mm", self.stock.part_size.z));

                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("2. Berechnetes Rohteil")
                        .strong()
                        .color(Color32::from_rgb(255, 200, 70)),
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
                    "Z-Rohmaß: {:.2} mm (10-mm-Stufen: 10, 20, 30, …)",
                    self.stock.stock_size.z
                ));

                ui.add_space(8.0);
                ui.label(egui::RichText::new("3. Z-Spannsituation").strong());
                ui.label(format!(
                    "• Schraubstock-Spanntiefe: {:.1} mm (unten)",
                    self.stock.clamp_depth_z
                ));
                ui.label(format!(
                    "• Planfräs-Aufmaß:         {:.1} mm (oben)",
                    self.stock.top_allowance_z
                ));

                ui.separator();
                ui.checkbox(&mut self.show_stock, "Rohteil-Box anzeigen");
                ui.label(format!(
                    "Dreiecke im Viewport: {}",
                    self.mesh.triangles.len()
                ));

                ui.add_space(8.0);
                ui.label(egui::RichText::new("Steuerung Viewport").strong());
                ui.label("• Linke Maustaste ziehen: Drehen");
                ui.label("• Mausrad: Zoomen");
                ui.label("• STEP-Datei ins Fenster ziehen: Laden");

                if ui.button("Kamera zurücksetzen").clicked() {
                    self.reset_camera();
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::drag());

            if response.dragged() {
                self.yaw += response.drag_delta().x * 0.01;
                self.pitch = (self.pitch + response.drag_delta().y * 0.01).clamp(-1.5, 1.5);
            }
            let scroll = ui.input(|i| i.raw_scroll_delta.y);
            if scroll.abs() > 0.0 {
                self.zoom = (self.zoom + scroll * 0.002).clamp(0.3, 6.0);
            }

            self.render_3d_scene(ui, rect, response.dragged());

            // Overlay, solange eine Datei über dem Fenster schwebt.
            let hovering = ui.input(|i| !i.raw.hovered_files.is_empty());
            if hovering {
                let painter = ui.painter_at(rect);
                painter.rect_filled(
                    rect,
                    0.0_f32,
                    Color32::from_rgba_unmultiplied(20, 30, 45, 200),
                );
                painter.rect_stroke(
                    rect.shrink(12.0),
                    10.0_f32,
                    Stroke::new(2.5_f32, Color32::from_rgb(120, 200, 255)),
                );
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "STEP-Datei hier ablegen",
                    egui::FontId::proportional(26.0),
                    Color32::from_rgb(200, 230, 255),
                );
            }
        });
    }
}

// ---------------------------------------------------------
// 4. 3D Projektion & Rendering
// ---------------------------------------------------------

impl AppState {
    fn render_3d_scene(&mut self, ui: &mut egui::Ui, rect: Rect, interactive: bool) {
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0_f32, Color32::from_rgb(23, 25, 29));

        let part = self.stock.part_size;
        if part.x <= 0.0 || part.y <= 0.0 || part.z <= 0.0 {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Keine Geometrie geladen",
                egui::FontId::proportional(18.0),
                Color32::GRAY,
            );
            return;
        }

        let sx = self.stock.stock_size.x;
        let sy = self.stock.stock_size.y;
        let sz = self.stock.stock_size.z;

        // Rohteil-Box: X/Y zentriert, unten 4 mm Spannrand, oben 1 mm Aufmaß
        let z_min = -self.stock.clamp_depth_z;
        let bmin = Vec3::new((part.x - sx) * 0.5, (part.y - sy) * 0.5, z_min);
        let bmax = bmin + Vec3::new(sx, sy, sz);
        let center = (bmin + bmax) * 0.5;
        let radius = ((bmax - bmin).length() * 0.5).max(1.0);

        // Automatische Einpassung: die gesamte Rohteil-Box passt immer ins Bild.
        let scale = rect.width().min(rect.height()) * 0.42 / radius * self.zoom;
        let screen_center = rect.center();

        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();

        let rotate = |p: Vec3| -> Vec3 {
            let x1 = p.x * cos_yaw - p.y * sin_yaw;
            let y1 = p.x * sin_yaw + p.y * cos_yaw;
            let z1 = p.z;
            let y2 = y1 * cos_pitch - z1 * sin_pitch;
            let z2 = y1 * sin_pitch + z1 * cos_pitch;
            Vec3::new(x1, y2, z2)
        };
        let to_view = |v: Vec3| rotate(v - center);
        let project = |v: Vec3| -> Pos2 {
            let p = to_view(v);
            Pos2::new(screen_center.x + p.x * scale, screen_center.y - p.z * scale)
        };

        // ---- Bauteil: software-gerastert mit Z-Buffer ----
        // egui selbst hat keinen Tiefenpuffer. Ohne einen solchen werden
        // Rückseiten (z. B. Innenwände von Bohrungen) über die Vorderseite
        // gemalt und erscheinen als dunkle Dreiecke/Linien. Deshalb wird das
        // Bauteil hier pro Pixel tiefensortiert gerastert und als Textur
        // angezeigt.
        // Während des Drehens in geringerer Auflösung rastern (flüssiger),
        // danach in voller Qualität.
        let max_ppp = if interactive { 1.0 } else { 1.5 };
        let ppp = ui.ctx().pixels_per_point().clamp(1.0, max_ppp);
        let width = (rect.width() * ppp).round().max(1.0) as u32;
        let height = (rect.height() * ppp).round().max(1.0) as u32;
        let key = SceneCacheKey {
            width,
            height,
            yaw: self.yaw,
            pitch: self.pitch,
            zoom: self.zoom,
            revision: self.revision,
        };

        let needs_render = self
            .scene
            .as_ref()
            .map(|cache| cache.key != key)
            .unwrap_or(true);

        if needs_render {
            let image = rasterize_part(
                &self.mesh,
                &self.stock,
                self.yaw,
                self.pitch,
                self.zoom,
                width as usize,
                height as usize,
            );
            let texture = ui
                .ctx()
                .load_texture("part_scene", image, egui::TextureOptions::LINEAR);
            self.scene = Some(SceneCache { key, texture });
        }

        if let Some(cache) = &self.scene {
            painter.image(
                cache.texture.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        // ---- Rohteil-Boundingbox ----
        if self.show_stock {
            let corners = [
                Vec3::new(bmin.x, bmin.y, bmin.z),
                Vec3::new(bmax.x, bmin.y, bmin.z),
                Vec3::new(bmax.x, bmax.y, bmin.z),
                Vec3::new(bmin.x, bmax.y, bmin.z),
                Vec3::new(bmin.x, bmin.y, bmax.z),
                Vec3::new(bmax.x, bmin.y, bmax.z),
                Vec3::new(bmax.x, bmax.y, bmax.z),
                Vec3::new(bmin.x, bmax.y, bmax.z),
            ];
            let edges = [
                (0, 1),
                (1, 2),
                (2, 3),
                (3, 0),
                (4, 5),
                (5, 6),
                (6, 7),
                (7, 4),
                (0, 4),
                (1, 5),
                (2, 6),
                (3, 7),
            ];

            for &(i, j) in &edges {
                painter.line_segment(
                    [project(corners[i]), project(corners[j])],
                    Stroke::new(1.6_f32, Color32::from_rgb(255, 200, 70)),
                );
            }

            // Spanngrenze auf Z = 0 (Oberkante des Spannrands)
            let clamp = [
                Vec3::new(bmin.x, bmin.y, 0.0),
                Vec3::new(bmax.x, bmin.y, 0.0),
                Vec3::new(bmax.x, bmax.y, 0.0),
                Vec3::new(bmin.x, bmax.y, 0.0),
            ];
            for i in 0..4 {
                painter.line_segment(
                    [project(clamp[i]), project(clamp[(i + 1) % 4])],
                    Stroke::new(1.0_f32, Color32::from_rgb(230, 90, 90)),
                );
            }
        }

        // ---- Achsen-Gizmo unten links ----
        let anchor = Pos2::new(rect.left() + 54.0, rect.bottom() - 54.0);
        for (axis, color, label) in [
            (Vec3::X, Color32::from_rgb(235, 80, 80), "X"),
            (Vec3::Y, Color32::from_rgb(90, 210, 120), "Y"),
            (Vec3::Z, Color32::from_rgb(100, 150, 255), "Z"),
        ] {
            let v = rotate(axis);
            let end = anchor + Vec2::new(v.x, -v.z) * 36.0;
            painter.line_segment([anchor, end], Stroke::new(2.0_f32, color));
            painter.text(
                end + Vec2::new(4.0, -4.0),
                egui::Align2::LEFT_BOTTOM,
                label,
                egui::FontId::monospace(12.0),
                color,
            );
        }
        painter.circle_filled(anchor, 2.5, Color32::from_gray(150));
    }
}

// ---------------------------------------------------------
// 5. Einstiegspunkt
// ---------------------------------------------------------

/// Headless-Debugrender mit Z-Buffer (nur zur Entwicklung).
/// Rastert das Bauteil tiefensortiert in ein Bild (Z-Buffer).
/// Dadurch sind verdeckte Flächen (z. B. Rückseiten von Bohrungen) korrekt
/// verdeckt, unabhängig von der Dreiecksreihenfolge.
fn rasterize_part(
    mesh: &MeshData,
    stock: &StockResult,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    w: usize,
    h: usize,
) -> egui::ColorImage {
    let mut pixels = vec![0u8; w * h * 4];
    let mut depth = vec![f32::INFINITY; w * h];
    if w == 0 || h == 0 {
        return egui::ColorImage::from_rgba_unmultiplied([w, h], &pixels);
    }

    let part = stock.part_size;
    let (sx, sy, sz) = (stock.stock_size.x, stock.stock_size.y, stock.stock_size.z);
    let z_min = -stock.clamp_depth_z;
    let bmin = Vec3::new((part.x - sx) * 0.5, (part.y - sy) * 0.5, z_min);
    let bmax = bmin + Vec3::new(sx, sy, sz);
    let center = (bmin + bmax) * 0.5;
    let radius = ((bmax - bmin).length() * 0.5).max(1.0);

    let scale = (w.min(h) as f32) * 0.42 / radius * zoom;
    let screen_center = Pos2::new(w as f32 * 0.5, h as f32 * 0.5);
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    let (sin_pitch, cos_pitch) = pitch.sin_cos();

    let to_view = |p: Vec3| -> Vec3 {
        let x1 = p.x * cos_yaw - p.y * sin_yaw;
        let y1 = p.x * sin_yaw + p.y * cos_yaw;
        let z1 = p.z;
        let y2 = y1 * cos_pitch - z1 * sin_pitch;
        let z2 = y1 * sin_pitch + z1 * cos_pitch;
        Vec3::new(x1, y2, z2)
    };
    let project = |v: Vec3| -> (Pos2, f32) {
        let p = to_view(v - center);
        (
            Pos2::new(screen_center.x + p.x * scale, screen_center.y - p.z * scale),
            p.y,
        )
    };

    let light = Vec3::new(-0.35, -0.5, 0.8).normalize();
    // Richtungsvektoren werden genauso rotiert wie Punkte (nur ohne Translation).
    let view_center = to_view(center);
    let to_dir = |n: Vec3| to_view(center + n) - view_center;

    for (ti, tri) in mesh.triangles.iter().enumerate() {
        let (a, b, c) = (
            mesh.vertices[tri[0]],
            mesh.vertices[tri[1]],
            mesh.vertices[tri[2]],
        );

        let va_v = to_view(a);
        let vb_v = to_view(b);
        let vc_v = to_view(c);
        let (pa, da) = project(a);
        let (pb, db) = project(b);
        let (pc, dc) = project(c);

        // Dreiecke minimal aufweiten: schließt Haarrisse zwischen Nachbarn.
        let mut pts = [pa, pb, pc];
        let centroid = Pos2::new(
            (pts[0].x + pts[1].x + pts[2].x) / 3.0,
            (pts[0].y + pts[1].y + pts[2].y) / 3.0,
        );
        for p in &mut pts {
            let d = *p - centroid;
            let len = d.length();
            if len > 1e-4 {
                *p += d / len * 0.4;
            }
        }
        let (pa, pb, pc) = (pts[0], pts[1], pts[2]);

        let normal = (vb_v - va_v).cross(vc_v - va_v);
        let len = normal.length();
        if len < 1e-9 {
            continue;
        }
        let face_normal = normal / len;

        // Geglättete Eckennormalen (falls vorhanden) für weiche Schattierung
        // gekrümmter Flächen; sonst flache Flächennormale.
        let (na, nb, nc) = match mesh.normals.get(ti) {
            Some(n) => (to_dir(n[0]), to_dir(n[1]), to_dir(n[2])),
            None => (face_normal, face_normal, face_normal),
        };

        let area = (pb.x - pa.x) * (pc.y - pa.y) - (pb.y - pa.y) * (pc.x - pa.x);
        if area.abs() < 1e-9 {
            continue;
        }

        let minx = pa.x.min(pb.x).min(pc.x).floor().max(0.0) as usize;
        let maxx = (pa.x.max(pb.x).max(pc.x).ceil()).min(w as f32 - 1.0) as usize;
        let miny = pa.y.min(pb.y).min(pc.y).floor().max(0.0) as usize;
        let maxy = (pa.y.max(pb.y).max(pc.y).ceil()).min(h as f32 - 1.0) as usize;

        for y in miny..=maxy {
            for x in minx..=maxx {
                let p = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
                let e0 = (pb.x - pa.x) * (p.y - pa.y) - (pb.y - pa.y) * (p.x - pa.x);
                let e1 = (pc.x - pb.x) * (p.y - pb.y) - (pc.y - pb.y) * (p.x - pb.x);
                let e2 = (pa.x - pc.x) * (p.y - pc.y) - (pa.y - pc.y) * (p.x - pc.x);
                if (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0) || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0) {
                    let w0 = e1 / area;
                    let w1 = e2 / area;
                    let w2 = e0 / area;
                    let z = da * w0 + db * w1 + dc * w2;
                    let idx = y * w + x;
                    if z < depth[idx] {
                        depth[idx] = z;
                        let mut n = na * w0 + nb * w1 + nc * w2;
                        let nl = n.length();
                        if nl < 1e-9 {
                            n = face_normal;
                        } else {
                            n /= nl;
                        }
                        let shade = 0.35 + 0.65 * n.dot(light).abs();
                        let color = [
                            (185.0 * shade).clamp(0.0, 255.0) as u8,
                            (195.0 * shade).clamp(0.0, 255.0) as u8,
                            (210.0 * shade).clamp(0.0, 255.0) as u8,
                            255u8,
                        ];
                        pixels[idx * 4..idx * 4 + 4].copy_from_slice(&color);
                    }
                }
            }
        }
    }

    egui::ColorImage::from_rgba_unmultiplied([w, h], &pixels)
}

/// Headless-Debugrender: schreibt das gerasterte Bauteil als BMP.
fn render_offscreen(path: &Path, out: &Path) -> Result<(), String> {
    let mut mesh = load_step_mesh(path)?;
    let stock = orient_and_align_mesh(&mut mesh);
    let (w, h) = (1200usize, 900usize);
    let image = rasterize_part(&mesh, &stock, -0.72, 0.42, 1.0, w, h);

    let row = (w * 3 + 3) & !3;
    let size = 54 + row * h;
    let mut bytes = Vec::with_capacity(size);
    bytes.extend_from_slice(b"BM");
    bytes.extend_from_slice(&(size as u32).to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(&54u32.to_le_bytes());
    bytes.extend_from_slice(&40u32.to_le_bytes());
    bytes.extend_from_slice(&(w as i32).to_le_bytes());
    bytes.extend_from_slice(&(h as i32).to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&24u16.to_le_bytes());
    bytes.extend_from_slice(&[0u8; 24]);
    for y in (0..h).rev() {
        for x in 0..w {
            let i = y * w + x;
            let c = image.pixels[i];
            let rgb = if c.a() == 0 {
                [23u8, 25u8, 29u8]
            } else {
                [c.r(), c.g(), c.b()]
            };
            bytes.extend_from_slice(&[rgb[2], rgb[1], rgb[0]]);
        }
        for _ in w * 3..row {
            bytes.push(0);
        }
    }
    std::fs::write(out, bytes).map_err(|e| e.to_string())?;
    println!(
        "Render: {} Dreiecke -> {} ({w}x{h})",
        mesh.triangles.len(),
        out.display()
    );
    Ok(())
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Headless-Test: `cargo run -- --inspect datei.step`
    if args.first().map(String::as_str) == Some("--inspect") {
        let Some(path) = args.get(1) else {
            eprintln!("Nutzung: rohteil_calc --inspect <datei.step>");
            std::process::exit(2);
        };
        match load_step_mesh(Path::new(path)) {
            Ok(mut mesh) => {
                let stock = orient_and_align_mesh(&mut mesh);
                println!("Dreiecke: {}", mesh.triangles.len());
                println!(
                    "Bauteil:  {:.2} × {:.2} × {:.2} mm",
                    stock.part_size.x, stock.part_size.y, stock.part_size.z
                );
                println!(
                    "Rohteil:  {:.2} × {:.2} × {:.2} mm",
                    stock.stock_size.x, stock.stock_size.y, stock.stock_size.z
                );
            }
            Err(err) => {
                eprintln!("Fehler: {err}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // Headless-Debug: `cargo run -- --render datei.step out.bmp`
    if args.first().map(String::as_str) == Some("--render") {
        let (Some(path), Some(out)) = (args.get(1), args.get(2)) else {
            eprintln!("Nutzung: rohteil_calc --render <datei.step> <out.bmp>");
            std::process::exit(2);
        };
        if let Err(err) = render_offscreen(Path::new(path), Path::new(out)) {
            eprintln!("Fehler: {err}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let initial = args.first().map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_title("Automatisierte CNC Rohteilberechnung"),
        multisampling: 4,
        ..Default::default()
    };
    eframe::run_native(
        "Rohteilberechnung",
        options,
        Box::new(move |_cc| Box::new(AppState::new(initial))),
    )
}

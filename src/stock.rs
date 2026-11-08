//! Rohteilberechnung: Ausrichtung des Bauteils und Ableitung der
//! Halbzeugmaße aus den Bauteilmaßen.

use crate::mesh::{build_feature_edges, compute_corner_normals, MeshData};
use glam::{Mat3, Vec3};

#[derive(Clone, Debug)]
pub struct StockResult {
    /// [Länge (X), Breite (Y), Höhe (Z)] des ausgerichteten Bauteils.
    #[allow(dead_code)] // bleibt Teil der Rohteil-Rechnung (Legacy-Pfad)
    pub part_size: Vec3,
    /// [Roh-X, Roh-Y, Roh-Z]. Bei Rundmaterial: X = Länge, Y = Z = Ø.
    pub stock_size: Vec3,
    /// 4.0 mm Spannrand unten (0.0 bei Rundmaterial).
    pub clamp_depth_z: f32,
    /// Mindestens 1.0 mm Planaufmaß oben (0.0 bei Rundmaterial).
    #[allow(dead_code)] // bleibt Teil der Rohteil-Rechnung (Legacy-Pfad)
    pub top_allowance_z: f32,
    /// Ausrichtungsmatrix (Bauteilkoordinaten -> ausgerichtete Koordinaten).
    /// Bleibt für spätere CAD-Operationen erhalten.
    #[allow(dead_code)]
    pub transform: Mat3,
    /// Rundmaterial (Rundstahl) statt Rechteckplatte.
    pub round: bool,
}

impl Default for StockResult {
    fn default() -> Self {
        Self {
            part_size: Vec3::ZERO,
            stock_size: Vec3::ZERO,
            clamp_depth_z: 4.0,
            top_allowance_z: 1.0,
            transform: Mat3::IDENTITY,
            round: false,
        }
    }
}

pub(crate) struct Alignment {
    pub(crate) rot: Mat3,
    pub(crate) centroid: Vec3,
    pub(crate) offset: Vec3,
}

impl Alignment {
    /// Wendet die Ausrichtung auf einen Punkt an (Bauteil -> ausgerichtet).
    pub(crate) fn apply(&self, p: Vec3) -> Vec3 {
        self.rot * (p - self.centroid) - self.offset
    }

    /// Wendet die Ausrichtung auf eine Richtung an (nur Rotation).
    pub(crate) fn rotate_dir(&self, d: Vec3) -> Vec3 {
        self.rot * d
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StockParams {
    /// Sägeaufmaß auf X — Standard 5.0 mm.
    pub saw_x_mm: f32,
    /// Schraubstock-Spanntiefe unten in Z — Standard 4.0 mm.
    pub clamp_z_mm: f32,
    /// Planfräs-Aufmaß oben in Z — Standard 1.0 mm.
    pub plan_top_mm: f32,
    /// Rundmaterial: `None` = automatisch nach Erkennung (kreisförmiger
    /// Querschnitt), `Some(true)`/`Some(false)` = übergesteuert.
    pub round_stock: Option<bool>,
    /// Ø-Aufmaß für Rundmaterial (gesamt auf den Durchmesser) — Standard 2.0 mm.
    pub round_add_mm: f32,
}

impl Default for StockParams {
    fn default() -> Self {
        Self {
            saw_x_mm: 5.0,
            clamp_z_mm: 4.0,
            plan_top_mm: 1.0,
            round_stock: None,
            round_add_mm: 2.0,
        }
    }
}

impl StockResult {
    /// Fertigungsregeln direkt aus den Bauteilmaßen (Standardeinstellungen;
    /// bleibt für Diagnose/Werkzeuge mit Legacy-Regeln erhalten).
    #[allow(dead_code)]
    pub fn from_part_size(part: Vec3) -> Self {
        Self::from_part_size_with_params(part, StockParams::default())
    }

    /// Dasselbe mit einstellbaren Aufmaßen (UI): Sägeaufmaß, Spanntiefe,
    /// Planfräs-Aufmaß; Y-Staffel und 10-mm-Plattenstufen bleiben Regeln.
    pub fn from_part_size_with_params(part: Vec3, p: StockParams) -> Self {
        if part == Vec3::ZERO {
            return Self::default();
        }
        // X: Bauteillänge + Sägeaufmaß
        let stock_x = part.x + p.saw_x_mm;
        // Y: Halbzeug-Staffel (Regel)
        let stock_y = calculate_standard_y_stock(part.y);
        // Z: Rohteilplatte — bis 30 mm in 5-mm-, darüber in 10-mm-Stufen.
        // (Platten 5/10/…/30 sind bestellbar, darüber gibt es nur 10er.)
        let raw_z = part.z + p.clamp_z_mm + p.plan_top_mm;
        let stock_z = if raw_z <= 30.0 + 1e-4 {
            (raw_z / 5.0).ceil() * 5.0
        } else {
            (raw_z / 10.0).ceil() * 10.0
        };
        let top_allowance_z = stock_z - part.z - p.clamp_z_mm;
        StockResult {
            part_size: part,
            stock_size: Vec3::new(stock_x, stock_y, stock_z),
            clamp_depth_z: p.clamp_z_mm,
            top_allowance_z,
            transform: Mat3::IDENTITY,
            round: false,
        }
    }
}

/// Rundstahl als Rohteil (liegend, Achse X): Ø = Bauteil-Ø + Ø-Aufmaß,
/// gerundet auf die Katalogstufen, Länge = Bauteillänge + Sägeaufmaß
/// auf X. Kein Platten-Spannmaß (der Stab läuft axial ins Spannfutter).
#[allow(dead_code)] // Wrapper für Diagnose/Werkzeuge (nutzt round_stock_for_axis)
pub fn round_stock_from_part(part: Vec3, p: StockParams) -> StockResult {
    round_stock_for_axis(part, 0, p)
}

/// Rundstahl für eine gegebene Rotationsachse: Axial-Richtung bekommt
/// Bauteil + Sägeaufmaß, die Querschnittsrichtung den Katalog-Ø
/// (unter 30 mm in 2er-, darüber in 5er-Schritten). `axis`: 0 = X
/// (liegender Stab), 2 = Z (stehende Scheibe); 1 wird wie 0 behandelt,
/// da die Anzeige-Achse angesichts der Ausrichtung X bevorzugt ist.
pub fn round_stock_for_axis(mut part: Vec3, mut axis: usize, p: StockParams) -> StockResult {
    if part == Vec3::ZERO {
        return StockResult::default();
    }
    if axis == 1 {
        // Y-Achse ist für die Anzeige-Ausrichtung unüblich — als X rechnen.
        part = Vec3::new(part.y, part.x, part.z);
        axis = 0;
    }
    let (r0, r1) = match axis {
        0 => (1usize, 2usize),
        _ => (0usize, 1usize),
    };
    // Ø aus dem Querschnitt: das größere der beiden Maße — bei knapp
    // nicht rundem Bauteil die sichere Seite.
    let d = part[r0].max(part[r1]);
    let stock_d = catalog_diameter(d + p.round_add_mm);
    let axial = part[axis] + p.saw_x_mm;
    let mut size = [0.0_f32; 3];
    size[axis] = axial;
    size[r0] = stock_d;
    size[r1] = stock_d;
    StockResult {
        part_size: part,
        stock_size: Vec3::new(size[0], size[1], size[2]),
        clamp_depth_z: 0.0,
        top_allowance_z: 0.0,
        transform: Mat3::IDENTITY,
        round: true,
    }
}

/// Nächstgrößere Katalogstufe für Rundstahl: unter 30 mm in 2er-, ab
/// 30 mm in 5er-Schritten (… 26, 28, 30, 35, 40, 45 …).
fn catalog_diameter(raw: f32) -> f32 {
    let raw = raw.max(2.0);
    if raw <= 30.0 {
        (raw * 0.5).ceil() * 2.0
    } else {
        (raw / 5.0).ceil() * 5.0
    }
}

/// Rundmaterial-Erkennung: Ist die Geometrie ein Dreh-Teil mit Achse
/// entlang einer Koordinatenachse? Prüft je Achse, ob der Querschnitt
/// senkrecht dazu kreisförmig ist: Erst sind die beiden Querschnitt-Maße
/// gleich (innerhalb 2 % bzw. 0,25 mm), dann deckt die Silhouettenfläche
/// (halb die Summe der projizierten Dreiecksflächen — für geschlossene
/// Netze exakt) fast den Kreis π/4 · Ø² ab (innerhalb 2 %). Liefert die
/// Achse (0 = X, 1 = Y, 2 = Z) der Rotationsachse, oder `None`.
pub fn detect_round_axis(positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> Option<usize> {
    if positions.is_empty() || triangles.is_empty() {
        return None;
    }
    (0..3).find_map(|axis| cross_section_round(positions, triangles, axis))
}

fn cross_section_round(positions: &[[f32; 3]], triangles: &[[u32; 3]], axis: usize) -> Option<usize> {
    // Achsen-Wahl: (a, b) = die beiden übrigen Koordinaten.
    let others = match axis {
        0 => (1usize, 2usize),
        1 => (0usize, 2usize),
        _ => (0usize, 1usize),
    };
    let mut mn_b = [f32::MAX; 2];
    let mut mx_b = [f32::MIN; 2];
    for p in positions {
        mn_b[0] = mn_b[0].min(p[others.0]);
        mn_b[1] = mn_b[1].min(p[others.1]);
        mx_b[0] = mx_b[0].max(p[others.0]);
        mx_b[1] = mx_b[1].max(p[others.1]);
    }
    let d1 = mx_b[0] - mn_b[0];
    let d2 = mx_b[1] - mn_b[1];
    if d1 <= 0.0 || d2 <= 0.0 {
        return None;
    }
    // Gleichmäßige Ausdehnung beider Querschnitt-Richtungen (Kreis).
    if (d1 - d2).abs() > (0.02 * d1.max(d2)).max(0.25) {
        return None;
    }
    let d = (d1 + d2) * 0.5;
    // Silhouettenfläche: für ein geschlossenes Netz deckt jede Blick-
    // strahlt die Fläche zweimal (Vorder- und Rückseite) — halbe Summe
    // der projizierten Dreiecksflächen ist die Konturfläche.
    let mut area = 0.0_f64;
    for t in triangles {
        let Some(a) = positions.get(t[0] as usize) else { continue };
        let Some(b) = positions.get(t[1] as usize) else { continue };
        let Some(c) = positions.get(t[2] as usize) else { continue };
        let u = [f64::from(b[others.0] - a[others.0]), f64::from(b[others.1] - a[others.1])];
        let v = [f64::from(c[others.0] - a[others.0]), f64::from(c[others.1] - a[others.1])];
        area += (u[0] * v[1] - u[1] * v[0]).abs() * 0.5;
    }
    let silhouette = area * 0.5;
    let circle = std::f64::consts::PI * 0.25 * f64::from(d) * f64::from(d);
    // Toleranztunnel: Kreis innerhalb 2 % (Sechsecke &c. fallen raus).
    if (silhouette - circle).abs() / circle <= 0.02 {
        Some(axis)
    } else {
        None
    }
}


/// Ermittelt die Hauptachsen (Trägheitsachsen/Kovarianz) und die
/// Ausrichtungsmatrix: Längste Seite -> X, mittlere -> Y, kleinste -> Z.
/// Ausrichtungsmatrix aus bereits gesammelten Vertices (alle Körper zusammen):
/// für die SolidCraft-Szenen-Transformation (rigide) vor der Anzeige.
///
/// Legacy-Form: die Kandidaten sind Weltachsen plus die normalisierten
/// Kovarianz-Spalten — keine echten Eigenvektoren. Bleibt für den
/// Legacy-Rohteil-Pfad (tests/diagnose, frozen Regression) unverändert.
pub fn alignment_for_vertices(vertices: &[Vec3]) -> Option<(Alignment, Vec3)> {
    let (align, size) = compute_alignment(vertices);
    (size != Vec3::ZERO).then_some((align, size))
}

/// Dasselbe wie [`alignment_for_vertices`], aber mit echten Eigenvektoren
/// der Kovarianzmatrix (Jacobi-Rotationen) als Hauptachsen-Kandidaten —
/// der Pfad für die SolidCraft-Szene: dort stimmte die Ausrichtung bei
/// schräg liegenden Bauteilen (nach der Kovarianz-Spalten-Heuristik)
/// nicht immer.
pub fn alignment_for_vertices_refined(vertices: &[Vec3]) -> Option<(Alignment, Vec3)> {
    let (align, size) = compute_alignment_refined(vertices);
    (size != Vec3::ZERO).then_some((align, size))
}

fn compute_alignment(vertices: &[Vec3]) -> (Alignment, Vec3) {
    if vertices.is_empty() {
        return (
            Alignment {
                rot: Mat3::IDENTITY,
                centroid: Vec3::ZERO,
                offset: Vec3::ZERO,
            },
            Vec3::ZERO,
        );
    }

    // 1. Schwerpunkt
    let n = vertices.len() as f32;
    let centroid = vertices.iter().fold(Vec3::ZERO, |acc, &v| acc + v) / n;

    // 2. Kovarianzmatrix für die Hauptträgheitsachsen
    let mut cov = Mat3::ZERO;
    for v in vertices {
        let p = *v - centroid;
        cov.x_axis += Vec3::new(p.x * p.x, p.x * p.y, p.x * p.z);
        cov.y_axis += Vec3::new(p.y * p.x, p.y * p.y, p.y * p.z);
        cov.z_axis += Vec3::new(p.z * p.x, p.z * p.y, p.z * p.z);
    }
    cov.x_axis /= n;
    cov.y_axis /= n;
    cov.z_axis /= n;

    // Kandidaten: Weltachsen + Kovarianzrichtungen (Legacy-Heuristik).
    let candidates = [
        Vec3::X,
        Vec3::Y,
        Vec3::Z,
        cov.x_axis.normalize_or_zero(),
        cov.y_axis.normalize_or_zero(),
        cov.z_axis.normalize_or_zero(),
    ];

    // Legacy-Rohteil-Pfad (frozen Regression) behält die Ecke als Ursprung.
    alignment_from_candidates(vertices, &candidates, false)
}

fn compute_alignment_refined(vertices: &[Vec3]) -> (Alignment, Vec3) {
    if vertices.is_empty() {
        return (
            Alignment {
                rot: Mat3::IDENTITY,
                centroid: Vec3::ZERO,
                offset: Vec3::ZERO,
            },
            Vec3::ZERO,
        );
    }

    let n = vertices.len() as f32;
    let centroid = vertices.iter().fold(Vec3::ZERO, |acc, &v| acc + v) / n;

    let mut cov = Mat3::ZERO;
    for v in vertices {
        let p = *v - centroid;
        cov.x_axis += Vec3::new(p.x * p.x, p.x * p.y, p.x * p.z);
        cov.y_axis += Vec3::new(p.y * p.x, p.y * p.y, p.y * p.z);
        cov.z_axis += Vec3::new(p.z * p.x, p.z * p.y, p.z * p.z);
    }
    cov.x_axis /= n;
    cov.y_axis /= n;
    cov.z_axis /= n;

    // Kandidaten: Weltachsen + echte Hauptträgheitsachsen (Eigenvektoren).
    let [e1, e2, e3] = eigen_symmetric(cov);
    let candidates = [Vec3::X, Vec3::Y, Vec3::Z, e1, e2, e3];

    // Anzeige-/Rohteil-Pfad: Bauteil in X/Y um die Mitte ausgemittelt.
    alignment_from_candidates(vertices, &candidates, true)
}

/// Bestes Achsen-Tripel nach Kanten-Volumen suchen und daraus die
/// Ausrichtung bauen: Längste -> X, mittlere -> Y, kürzeste -> Z
/// (rechtshändiger Rahmen; Nullpunkt an der Unterkante, bei
/// `center_xy` zudem in X/Y um die Bauteilmitte ausgemittelt). Kein
/// Kandidat gewinnt, ist die achsenparallele Box (X, Y, Z) das Ergebnis.
fn alignment_from_candidates(
    vertices: &[Vec3],
    candidates: &[Vec3; 6],
    center_xy: bool,
) -> (Alignment, Vec3) {
    // 1. Schwerpunkt (Aufrufer leeren Input vorher ab)
    let n = vertices.len() as f32;
    let centroid = vertices.iter().fold(Vec3::ZERO, |acc, &v| acc + v) / n;

    // Startwert: achsenparallele Bounding Box
    let mut bb_min = Vec3::splat(f32::MAX);
    let mut bb_max = Vec3::splat(f32::MIN);
    for v in vertices {
        bb_min = bb_min.min(*v);
        bb_max = bb_max.max(*v);
    }
    let mut best_extents = bb_max - bb_min;
    let mut best_axes = (Vec3::X, Vec3::Y, Vec3::Z);

    for &ax in candidates {
        if ax.length_squared() < 0.1 {
            continue;
        }
        for &ay in candidates {
            if ay.length_squared() < 0.1 || ax.dot(ay).abs() > 0.9 {
                continue;
            }
            let az = ax.cross(ay).normalize_or_zero();
            if az.length_squared() < 0.1 {
                continue;
            }
            let ay_ortho = az.cross(ax).normalize();

            let (mut min_b, mut max_b) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for v in vertices {
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
    let new_y = new_z.cross(new_x).normalize_or_zero();
    let rot = Mat3::from_cols(new_x, new_y, new_z).transpose();

    // Extents und Offset im ausgerichteten Raum bestimmen.
    let mut min_bound = Vec3::splat(f32::MAX);
    let mut max_bound = Vec3::splat(f32::MIN);
    for v in vertices {
        let p = rot * (*v - centroid);
        min_bound = min_bound.min(p);
        max_bound = max_bound.max(p);
    }
    let ext = max_bound - min_bound;

    // Legacy-Pfad: Ecke (0,0) unten; verfeinerter Pfad (Anzeige/Rohteil):
    // X/Y um die Bauteilmitte ausgemittelt — Z bleibt unten auf 0.
    let offset = if center_xy {
        Vec3::new(min_bound.x + ext.x * 0.5, min_bound.y + ext.y * 0.5, min_bound.z)
    } else {
        min_bound
    };

    (
        Alignment {
            rot,
            centroid,
            offset,
        },
        ext,
    )
}

/// Ausrichtung mit vorgegebener „unten“-Richtung: `down` ist die Normale
/// einer angeklickten Fläche (Datei-Koordinaten); diese Fläche liegt
/// danach unten (−Z). Weiteres gilt nur für diesen manuellen Pfad:
/// die längere Grundflächenseite liegt auf X, die kürzere auf Y — auch
/// wenn Z dadurch die längste Bauteilmaße wird. Das Bauteil ist
/// außerdem in X/Y um 0 ausgemittelt (−ext/2 … +ext/2); Z bleibt
/// unverändert (unten = 0). Der Basis-In-Ebene-Richtung dient die X-Achse
/// der automatischen Ausrichtung, entartet siethese (z. B. Endfläche
/// angeklickt), entscheidet der kleinste Flächenfußabdruck. Liefert
/// `(Alignment, Bauteilmaße)` oder `None` bei leerem Input oder
/// entarteter Richtung.
pub fn alignment_with_down(vertices: &[Vec3], down: Vec3) -> Option<(Alignment, Vec3)> {
    if vertices.is_empty() {
        return None;
    }
    let u = down.normalize_or_zero();
    if u.length_squared() < 0.5 {
        return None;
    }
    // Die angeklickte Fläche (Außennormale u) soll nach unten zeigen:
    // die neue +Z-Achse ist die Gegennormale.
    let new_z = -u;

    let n = vertices.len() as f32;
    let centroid = vertices.iter().fold(Vec3::ZERO, |acc, &v| acc + v) / n;

    // Basis-In-Ebene-Richtung: die X-Achse der automatischen Ausrichtung,
    // auf die neue Grundfläche projiziert (bleibt es die Längsachse).
    let auto = compute_alignment_refined(vertices).0;
    let auto_x = auto.rot.transpose().x_axis;
    let proj = auto_x - u * auto_x.dot(u);
    let (base_x, base_y) = if proj.length_squared() > 0.1 {
        let x = proj.normalize();
        (x, new_z.cross(x).normalize_or_zero())
    } else {
        // Rückfall (Projektion entartet, z. B. Endfläche unten):
        // in-Ebene-Richtung mit dem kleinsten Flächenfußabdruck (die
        // Z-Ausdehnung ist über alle Kandidaten konstant).
        let mut cov = Mat3::ZERO;
        for v in vertices {
            let p = *v - centroid;
            cov.x_axis += Vec3::new(p.x * p.x, p.x * p.y, p.x * p.z);
            cov.y_axis += Vec3::new(p.y * p.x, p.y * p.y, p.y * p.z);
            cov.z_axis += Vec3::new(p.z * p.x, p.z * p.y, p.z * p.z);
        }
        cov.x_axis /= n;
        cov.y_axis /= n;
        cov.z_axis /= n;
        let [e1, e2, e3] = eigen_symmetric(cov);
        let raw = [Vec3::X, Vec3::Y, Vec3::Z, e1, e2, e3];
        let mut best: Option<(f32, Vec3, Vec3)> = None;
        for &ax in &raw {
            // Projektion auf die Ebene; fast parallele Kandidaten (inkl.
            // Rundungsrauschen knapp unter der Schwelle) auslassen — erst
            // die Rohlänge prüfen, dann erst normalisieren.
            let xp = ax - u * ax.dot(u);
            if xp.length_squared() < 0.1 {
                continue;
            }
            let xp = xp.normalize_or_zero();
            let yp = new_z.cross(xp).normalize_or_zero();
            if yp.length_squared() < 0.1 {
                continue;
            }
            let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for v in vertices {
                let proi = Vec3::new(v.dot(xp), v.dot(yp), v.dot(new_z));
                mn = mn.min(proi);
                mx = mx.max(proi);
            }
            let ext = mx - mn;
            let footprint = ext.x * ext.y;
            if best.is_none_or(|(b, _, _)| footprint < b) {
                best = Some((footprint, xp, yp));
            }
        }
        let (_, x, y) = best?;
        (x, y)
    };

    // Sortierung: die längere Grundflächenseite liegt auf X, die kürzere
    // auf Y. Ein reines Vertauschen kippt das Achsen-Tripel ins
    // spiegelbildliche — deshalb bekommt die neue Y-Achse das Minuszeichen.
    let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for v in vertices {
        let p = Vec3::new(v.dot(base_x), v.dot(base_y), v.dot(new_z));
        mn = mn.min(p);
        mx = mx.max(p);
    }
    let (new_x, new_y) = if mx.y - mn.y > mx.x - mn.x {
        (base_y, -base_x)
    } else {
        (base_x, base_y)
    };

    let rot = Mat3::from_cols(new_x, new_y, new_z).transpose();

    let mut min_bound = Vec3::splat(f32::MAX);
    let mut max_bound = Vec3::splat(f32::MIN);
    for v in vertices {
        let p = rot * (*v - centroid);
        min_bound = min_bound.min(p);
        max_bound = max_bound.max(p);
    }
    let ext = max_bound - min_bound;

    Some((
        Alignment {
            rot,
            centroid,
            // X/Y um 0 ausgemittelt (Bauteilmitte = Ursprung): part läuft
            // von −ext/2 bis +ext/2; Z bleibt unten auf 0.
            offset: Vec3::new(
                min_bound.x + ext.x * 0.5,
                min_bound.y + ext.y * 0.5,
                min_bound.z,
            ),
        },
        ext,
    ))
}

/// Eigenwerte + orthonormale Eigenvektoren einer symmetrischen 3×3-Matrix
/// (Jacobi-Rotationen: deterministisch, keine Panik, genug siebt, um
/// Mesh-Kovarianzen zu diagonalisieren). Die Vektoren sind nach Eigenwert
/// absteigend geordnet und normalisiert — die eigentlichen Hauptachsen.
fn eigen_symmetric(cov: Mat3) -> [Vec3; 3] {
    let mut m = [
        [cov.x_axis.x, cov.x_axis.y, cov.x_axis.z],
        [cov.y_axis.x, cov.y_axis.y, cov.y_axis.z],
        [cov.z_axis.x, cov.z_axis.y, cov.z_axis.z],
    ];
    // Akkumulierte Rotation (Spalten = Eigenvektoren).
    let mut v = [[1.0_f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    const SWEEPS: usize = 16;
    const EPS: f32 = 1e-9;
    for _ in 0..SWEEPS {
        // Ist die Matrix praktisch diagonal, ist nichts mehr zu drehen
        // (Summe der Off-Diagonalen quadriert — die Matrix bleibt
        // symmetrisch, daher jede Paarung doppelt).
        let (a01, a02, a12) = (m[0][1], m[0][2], m[1][2]);
        let off = 2.0 * (a01 * a01 + a02 * a02 + a12 * a12);
        if off < EPS {
            break;
        }
        for p in 0..3 {
            for q in (p + 1)..3 {
                let api = m[p][q];
                if api.abs() < EPS {
                    continue;
                }
                let (mpp, mqq) = (m[p][p], m[q][q]);
                // Rotation A → M·A·Mᵀ mit M-Zeile p = c·e_p + s·e_q (p < q):
                // θ so gewählt, dass das gedrehte a_pq verschwindet (Numerical
                // Recipes, jacobi) — numerisch stabil; θ = 0 heißt 45°-Drehung
                // (t = 1), riesiges θ → Kleinstwinkel.
                let theta = (mpp - mqq) / (2.0 * api);
                let t = if theta.abs() > 1e9 {
                    1.0 / (2.0 * theta)
                } else if theta == 0.0 {
                    1.0
                } else {
                    theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt())
                };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;

                // Zeilen p und q drehen (M·A).
                let (rp, rq) = (m[p], m[q]);
                for k in 0..3 {
                    m[p][k] = c * rp[k] + s * rq[k];
                    m[q][k] = c * rq[k] - s * rp[k];
                }
                // Spalten p und q drehen (A·Mᵀ).
                let (cp, cq) = (
                    [m[0][p], m[1][p], m[2][p]],
                    [m[0][q], m[1][q], m[2][q]],
                );
                m[0][p] = c * cp[0] + s * cq[0];
                m[1][p] = c * cp[1] + s * cq[1];
                m[2][p] = c * cp[2] + s * cq[2];
                m[0][q] = c * cq[0] - s * cp[0];
                m[1][q] = c * cq[1] - s * cp[1];
                m[2][q] = c * cq[2] - s * cp[2];
                // Eigenvektor-Basis mitführen (gleiche Spaltendrehung).
                let (vp, vq) = (
                    [v[0][p], v[1][p], v[2][p]],
                    [v[0][q], v[1][q], v[2][q]],
                );
                v[0][p] = c * vp[0] + s * vq[0];
                v[1][p] = c * vp[1] + s * vq[1];
                v[2][p] = c * vp[2] + s * vq[2];
                v[0][q] = c * vq[0] - s * vp[0];
                v[1][q] = c * vq[1] - s * vp[1];
                v[2][q] = c * vq[2] - s * vp[2];
            }
        }
    }

    // Eigenwerte aus der Diagonale, Vektoren je Spalte — sortiert absteigend.
    let mut eigens = [
        (m[0][0], Vec3::new(v[0][0], v[1][0], v[2][0])),
        (m[1][1], Vec3::new(v[0][1], v[1][1], v[2][1])),
        (m[2][2], Vec3::new(v[0][2], v[1][2], v[2][2])),
    ];
    eigens.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = [Vec3::X, Vec3::Y, Vec3::Z];
    for (i, (_, vec)) in eigens.iter().enumerate() {
        out[i] = vec.normalize_or_zero();
        if out[i].length_squared() < 0.5 {
            out[i] = Vec3::X;
        }
    }
    out
}

/// Richtet alle Körper gemeinsam aus (gleiche Transformation) und berechnet
/// Normalen sowie Feature-Kanten neu. Der Nullpunkt liegt auf der Unterkante
/// des Bauteils (Z = 0).
pub fn orient_and_align_bodies(bodies: &mut [MeshData]) -> StockResult {
    let all: Vec<Vec3> = bodies
        .iter()
        .flat_map(|b| b.vertices.iter().copied())
        .collect();
    let Some((align, part_size)) = alignment_for_vertices(&all) else {
        return StockResult::default();
    };

    for body in bodies.iter_mut() {
        for v in &mut body.vertices {
            *v = align.rot * (*v - align.centroid) - align.offset;
        }
        body.normals = compute_corner_normals(&body.vertices, &body.triangles);
        body.feature_edges = build_feature_edges(&body.vertices, &body.triangles);
    }

    // Fertigungsregeln:
    // X: Bauteillänge + 5 mm Sägeaufmaß
    let stock_x = part_size.x + 5.0;
    // Y: Halbzeug-Staffel
    let stock_y = calculate_standard_y_stock(part_size.y);
    // Z: Standard-Plattenstärke in 10-mm-Stufen, mind. 4 mm Spannrand
    //    + 1 mm Planaufmaß
    let stock_z = calculate_standard_z_stock(part_size.z);
    let top_allowance_z = stock_z - part_size.z - 4.0;

    StockResult {
        part_size,
        stock_size: Vec3::new(stock_x, stock_y, stock_z),
        clamp_depth_z: 4.0,
        top_allowance_z,
        transform: align.rot,
        round: false,
    }
}

/// Bequeme Variante für ein einzelnes Netz (Demo-Bauteil, CLI).
#[allow(dead_code)]
pub fn orient_and_align_mesh(mesh: &mut MeshData) -> StockResult {
    orient_and_align_bodies(std::slice::from_mut(mesh))
}

/// Nächste Standard-Plattenstärke in Z: 10, 20, 30, ...
fn calculate_standard_z_stock(z_part: f32) -> f32 {
    let min_allowance = 5.0; // 4 mm Spannrand + 1 mm Planaufmaß
    ((z_part + min_allowance) / 10.0).ceil() * 10.0
}

/// Nächste Standard-Halbzeugbreite in Y:
/// bis 25 mm in 5er-Schritten, ab 30 mm in 10er-Schritten.
fn calculate_standard_y_stock(y_part: f32) -> f32 {
    let min_allowance = 1.0;
    let required = y_part + min_allowance;

    if required <= 25.0 {
        (required / 5.0).ceil() * 5.0
    } else if required <= 30.0 {
        30.0
    } else {
        (required / 10.0).ceil() * 10.0
    }
}

#[cfg(test)]
mod alignment_tests {
    use super::{alignment_for_vertices, alignment_for_vertices_refined};
    use glam::Vec3;
    /// Box-Kanten (achsenparallel, im Ursprung auf der unteren Ecke).
    fn box_vertices(x: f32, y: f32, z: f32) -> Vec<Vec3> {
        let mut out = Vec::new();
        for sx in [0.0, x] {
            for sy in [0.0, y] {
                for sz in [0.0, z] {
                    out.push(Vec3::new(sx, sy, sz));
                }
            }
        }
        out
    }

    /// Rotation um die Weltachse (Rodrigues).
    fn rotate(v: Vec3, axis: Vec3, angle: f32) -> Vec3 {
        let (c, s) = angle.sin_cos();
        let k = axis.normalize();
        v * c + k.cross(v) * s + k * k.dot(v) * (1.0 - c)
    }

    /// Der verfeinerte Pfad richtet eine um 30°/20° gedrehte Box wieder
    /// auf: Maße (100, 40, 20) — längste Achse X, kürzeste Z.
    #[test]
    fn rotated_box_is_aligned_again() {
        let raw = box_vertices(100.0, 40.0, 20.0);
        let rotated: Vec<Vec3> = raw
            .iter()
            .map(|v| rotate(rotate(*v, Vec3::Z, 30_f32.to_radians()), Vec3::X, 20_f32.to_radians()))
            .collect();

        let (_, size) = alignment_for_vertices_refined(&rotated).expect("ausrichtbar");
        // Sortierte Maße identisch; Toleranz für die f32-Approximation.
        let mut dims = [size.x, size.y, size.z];
        dims.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        assert!((dims[0] - 100.0).abs() < 0.05, "X: {dims:?}");
        assert!((dims[1] - 40.0).abs() < 0.05, "Y: {dims:?}");
        assert!((dims[2] - 20.0).abs() < 0.05, "Z: {dims:?}");
    }

    /// Achsenparallele Box: verfeinerter Pfad bleibt bei dem Ergebnis des
    /// Legacy-Pfads — die Kandidaten-Weltachsen gewinnen wie davor.
    #[test]
    fn axis_aligned_box_unchanged() {
        let raw = box_vertices(100.0, 40.0, 20.0);
        let (_, size_old) = alignment_for_vertices(&raw).expect("ausrichtbar");
        let (_, size_new) = alignment_for_vertices_refined(&raw).expect("ausrichtbar");
        assert_eq!(size_old, size_new);
    }

    /// Automatische Ausrichtung (verfeinerter Pfad): Das Bauteil liegen
    /// in X/Y um die Mitte ausgemittelt (−ext/2 … +ext/2); Z bleibt unten
    /// auf 0. Der Legacy-Pfad (frozen Regression) behält die Ecke (0,0).
    #[test]
    fn refined_alignment_centers_part_in_xy() {
        let raw = box_vertices(100.0, 40.0, 20.0);
        let (align, _) = alignment_for_vertices_refined(&raw).expect("ausrichtbar");
        let mapped: Vec<Vec3> = raw.iter().map(|&v| align.apply(v)).collect();
        let (mn, mx) = mapped.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(mn, mx), p| (mn.min(*p), mx.max(*p)),
        );
        assert!(
            (-mn.x - mx.x).abs() < 0.02 && (-mn.y - mx.y).abs() < 0.02,
            "X/Y ausgemittelt: {mn:?} … {mx:?}"
        );
        assert!((mx.x - 50.0).abs() < 0.02, "X-Ende: {mx:?}");
        assert!((mx.y - 20.0).abs() < 0.02, "Y-Ende: {mx:?}");
        assert!(mn.z.abs() < 0.02, "unten bleibt 0: {mn:?}");
        assert!((mx.z - 20.0).abs() < 0.02, "Höhe: {mx:?}");

        // Legacy-Pfad unverändert: Unterkante an der Ecke (0,0).
        let (legacy, _) = alignment_for_vertices(&raw).expect("ausrichtbar");
        let legacy_min = legacy
            .apply(raw[0])
            .min(legacy.apply(raw[raw.len() - 1]))
            .min(legacy.apply(raw[5]));
        assert!(
            legacy_min.x.abs() < 0.02 && legacy_min.y.abs() < 0.02,
            "Legacy-Ursprung: {legacy_min:?}"
        );
    }

    /// Eigenvektoren wirken: eine um 35° gedrehte Platte (i-Achse lang,
    /// dicke in z) landet mit ihrer langen Richtung auf X.
    #[test]
    fn eigenvectors_pair_with_covariance() {
        let rotated = |v: Vec3| rotate(v, Vec3::Z, 35_f32.to_radians());
        let cloud: Vec<Vec3> = (-20..=20)
            .flat_map(|i| {
                (-5..=5)
                    .flat_map(move |j| (-2..=2).map(move |k| Vec3::new(i as f32, j as f32, k as f32)))
            })
            .map(rotated)
            .collect();
        let (align, _) = alignment_for_vertices_refined(&cloud).expect("ausrichtbar");
        // rot ordnet Weltkoordinaten den ausgerichteten zu: Die lange
        // Richtung der Platte (i-Achse, 55 lang gedacht, hier ±20·2)
        // soll nach Bearbeitung fast rein auf X fallen.
        let i_dir = rotated(Vec3::X);
        let mapped = align.rot * i_dir;
        assert!(mapped.x.abs() > 0.98, "i-Achse landet in X: {mapped:?}");
        assert!(mapped.y.abs() < 0.05, "kein Rest in Y: {mapped:?}");
    }

    /// Vorgegebene „unten“-Richtung: Eine um 25° um X gekippte Box
    /// (100 × 40 × 20) soll auf ihre 40×100-Fläche gestellt werden —
    /// die Normale dieser Fläche zeigt nach unten, Z wird 20.
    #[test]
    fn alignment_with_down_puts_face_at_bottom() {
        let raw = box_vertices(100.0, 40.0, 20.0);
        // Ursprüngliche Top-Normale (+Z): nach der Kippung um X zeigt sie
        // schräg; genau diese Richtung soll unten liegen.
        let top_normal = rotate(Vec3::Z, Vec3::X, 25_f32.to_radians());
        let rotated: Vec<Vec3> = raw
            .iter()
            .map(|v| rotate(*v, Vec3::X, 25_f32.to_radians()))
            .collect();
        let (align, size) =
            super::alignment_with_down(&rotated, top_normal).expect("ausrichtbar");

        // Bauteilmaße: 40 (Y) × 100 (Z-Fläche quer) … die Grundfläche ist
        // 100×40, Höhe 20 — Z muss 20 sein.
        assert!((size.z - 20.0).abs() < 0.05, "Höhe: {size:?}");
        let mut dims = [size.x, size.y];
        dims.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        assert!((dims[0] - 100.0).abs() < 0.05, "Grundfläche: {dims:?}");
        assert!((dims[1] - 40.0).abs() < 0.05, "Grundfläche: {dims:?}");

        // top_normal fällt nach der Rotation auf −Z (Fläche unten).
        let mapped = align.rot * top_normal;
        assert!(mapped.z < -0.98, "Flächennormale zeigt unten: {mapped:?}");
        assert!(mapped.x.abs() < 0.05 && mapped.y.abs() < 0.05, "sauber: {mapped:?}");

        // Unten liegt die angeklickte Fläche exakt auf Z = 0; Höhe 20.
        let zmin = rotated.iter().map(|&v| align.apply(v).z).fold(f32::MAX, f32::min);
        let zmax = rotated.iter().map(|&v| align.apply(v).z).fold(f32::MIN, f32::max);
        assert!(zmin.abs() < 0.02, "Unterkante: {zmin}");
        assert!((zmax - zmin - 20.0).abs() < 0.05, "Höhe: {zmax}");
    }

    /// Seitenfläche unten: Die Längsachse (100 mm) bleibt entlang X
    /// sichtbar ausgerichtet — X dreht nicht schräg.
    #[test]
    fn alignment_with_down_keeps_long_axis_on_side_face() {
        let raw = box_vertices(100.0, 40.0, 20.0);
        // Längs-Frontfläche (Normale +Y) soll unten liegen.
        let (align, size) = super::alignment_with_down(&raw, Vec3::Y).expect("ausrichtbar");

        // Die Datei-X-Achse (100 mm) fällt nach der Ausrichtung rein auf
        // ±X — der längliche Umriss zeigt wieder gerade nach X.
        let mapped_long = align.rot * Vec3::X;
        assert!(
            mapped_long.x.abs() > 0.98,
            "Längsachse dreht schräg: {mapped_long:?}"
        );
        assert!(
            mapped_long.y.abs() < 0.05 && mapped_long.z.abs() < 0.05,
            "Längsachse sauber: {mapped_long:?}"
        );
        // Die Grundfläche ist 100 × 20, Höhe (ehemals Y) 40.
        assert!((size.z - 40.0).abs() < 0.05, "Höhe: {size:?}");
        let mapped_u = align.rot * Vec3::Y;
        assert!(mapped_u.z < -0.98, "Geklickte Fläche unten: {mapped_u:?}");
    }

    /// Endfläche unten (Normale längs der Längsachse): die X-Projektion
    /// entartet — der Rückfall wählt trotzdem eine gültige Ausrichtung,
    /// die geklickte Fläche liegt unten.
    #[test]
    fn alignment_with_down_falls_back_on_end_face() {
        let raw = box_vertices(100.0, 40.0, 20.0);
        let (align, size) = super::alignment_with_down(&raw, Vec3::X).expect("ausrichtbar");
        let mapped_u = align.rot * Vec3::X;
        assert!(mapped_u.z < -0.98, "Geklickte Fläche unten: {mapped_u:?}");
        // Höhe = 100 (die Längsrichtung steht jetzt senkrecht).
        assert!((size.z - 100.0).abs() < 0.05, "Höhe: {size:?}");
    }

    /// Zentrierung: Das manuell ausgerichtete Bauteil liegt in X/Y
    /// symmetrisch um (0,0) — die Ecke (0,0) gehört der Rohteil-Ecke, das
    /// Bauteil der Mitte. Und: die längere Grundflächenseite liegt auf X
    /// (100), die kürzere auf Y (20), auch wenn Z mit 40 länger ist.
    #[test]
    fn alignment_with_down_centers_part_in_xy() {
        let raw = box_vertices(100.0, 40.0, 20.0);
        let (align, size) = super::alignment_with_down(&raw, Vec3::Y).expect("ausrichtbar");
        assert!((size.x - 100.0).abs() < 0.05, "X = längere Seite: {size:?}");
        assert!((size.y - 20.0).abs() < 0.05, "Y = kürzere Seite: {size:?}");

        let mapped: Vec<Vec3> = raw.iter().map(|&v| align.apply(v)).collect();
        let (mn, mx) = mapped.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(mn, mx), p| (mn.min(*p), mx.max(*p)),
        );
        assert!(
            (-mn.x - mx.x).abs() < 0.02 && (-mn.y - mx.y).abs() < 0.02,
            "X/Y ausgemittelt: {mn:?} … {mx:?}"
        );
        assert!((mx.x - 50.0).abs() < 0.02, "X-Ende: {mx:?}");
        assert!((mx.y - 10.0).abs() < 0.02, "Y-Ende: {mx:?}");
        assert!(mn.z.abs() < 0.02, "unten bleibt 0: {mn:?}");
        assert!((mx.z - 40.0).abs() < 0.05, "Höhe: {mx:?}");
    }

    // ------------------------------------------------------------- //
    // Rundmaterial                                                   //
    // ------------------------------------------------------------- //

    /// Zylinder (Achse entlang `axis`) als Dreiecksnetz, geschlossen mit
    /// Deckelflächen — Datengrundlage für die Rundmaterial-Erkennung.
    fn cylinder_mesh(r: f32, length: f32, axis: usize, segments: u32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let mut positions = Vec::new();
        for xi in [0.0_f32, length] {
            for s in 0..segments {
                let t = f64::from(s) / f64::from(segments) * std::f64::consts::TAU;
                let u = t.cos() as f32 * r;
                let v = t.sin() as f32 * r;
                positions.push(match axis {
                    0 => [xi, u, v],
                    1 => [u, xi, v],
                    _ => [u, v, xi],
                });
            }
        }
        let mut out = Vec::new();
        for si in 0..(segments as usize) {
            let ni = (si + 1) % segments as usize;
            let a = si as u32;
            let b = ni as u32;
            let c = (segments + a) as u32;
            let d = (segments + b) as u32;
            out.push([a, b, c]);
            out.push([b, d, c]);
            // Deckel je Seite als Dreiecke (Fächer um den Mittelpunkt).
            out.push([2 * segments, a, b]);
            out.push([2 * segments + 1, c, d]);
        }
        positions.push([0.0, 0.0, 0.0]);
        positions.push(match axis {
            0 => [length, 0.0, 0.0],
            1 => [0.0, length, 0.0],
            _ => [0.0, 0.0, length],
        });
        (positions, out)
    }

    /// Katalogstufen: unter 30 mm 2er-, ab 30 mm 5er-Schritte.
    #[test]
    fn round_catalog_diameter_steps() {
        use super::catalog_diameter;
        assert!((catalog_diameter(25.0) - 26.0).abs() < 1e-3);
        assert!((catalog_diameter(26.0) - 26.0).abs() < 1e-3);
        assert!((catalog_diameter(27.3) - 28.0).abs() < 1e-3);
        assert!((catalog_diameter(30.0) - 30.0).abs() < 1e-3);
        assert!((catalog_diameter(31.0) - 35.0).abs() < 1e-3);
        assert!((catalog_diameter(52.0) - 55.0).abs() < 1e-3);
    }

    /// Rundstahl: Ø aus dem Querschnitt plus Aufmaß, Länge plus Sägeaufmaß.
    #[test]
    fn round_stock_sizes() {
        use super::{round_stock_from_part, StockParams};
        let result = round_stock_from_part(
            Vec3::new(120.0, 50.0, 50.0),
            StockParams { saw_x_mm: 5.0, round_add_mm: 2.0, ..StockParams::default() },
        );
        assert!(result.round);
        assert_eq!(result.stock_size.x, 125.0);
        assert_eq!(result.stock_size.y, 55.0);
        assert_eq!(result.stock_size.z, 55.0);
        assert!(result.clamp_depth_z.abs() < 1e-6);
        // Rechteck-Rohteil bleibt unbeeinflusst (round_stock = None).
        let rect = super::StockResult::from_part_size_with_params(
            Vec3::new(120.0, 50.0, 14.0),
            StockParams::default(),
        );
        assert!(!rect.round);
    }

    /// Erkennung: ein Zylinder entlang X wird als Rundmaterial erkannt,
    /// ein Quader nicht.
    #[test]
    fn detect_round_cylinder_and_box() {
        use super::detect_round_axis;
        let cylinder = cylinder_mesh(20.0, 90.0, 0, 48);
        assert_eq!(detect_round_axis(&cylinder.0, &cylinder.1), Some(0));
        // senkrecht stehend: Achse Z.
        let z_cyl = cylinder_mesh(15.0, 60.0, 2, 48);
        assert_eq!(detect_round_axis(&z_cyl.0, &z_cyl.1), Some(2));
        // Quader mit echten Dreiecken: keine Rundmaterial-Erkennung.
        let (bp, bt) = box_triangles(90.0, 40.0, 20.0);
        assert_eq!(detect_round_axis(&bp, &bt), None);
    }

    /// Quader als Flächen-Dreiecke (w×d×h über die Kanten).
    fn box_triangles(w: f32, d: f32, hgt: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [w, 0.0, 0.0],
            [w, d, 0.0],
            [0.0, d, 0.0],
            [0.0, 0.0, hgt],
            [w, 0.0, hgt],
            [w, d, hgt],
            [0.0, d, hgt],
        ];
        let quads: [[u32; 4]; 6] = [
            [0, 3, 2, 1],
            [4, 5, 6, 7],
            [0, 1, 5, 4],
            [3, 7, 6, 2],
            [0, 4, 7, 3],
            [1, 2, 6, 5],
        ];
        let mut tris = Vec::new();
        for q in &quads {
            tris.push([q[0], q[1], q[2]]);
            tris.push([q[0], q[2], q[3]]);
        }
        (positions, tris)
    }
}

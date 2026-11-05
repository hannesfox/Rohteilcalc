//! Rohteilberechnung: Ausrichtung des Bauteils und Ableitung der
//! Halbzeugmaße aus den Bauteilmaßen.

use crate::mesh::{build_feature_edges, compute_corner_normals, MeshData};
use glam::{Mat3, Vec3};

#[derive(Clone, Debug)]
pub struct StockResult {
    /// [Länge (X), Breite (Y), Höhe (Z)] des ausgerichteten Bauteils.
    #[allow(dead_code)] // bleibt Teil der Rohteil-Rechnung (Legacy-Pfad)
    pub part_size: Vec3,
    /// [Roh-X, Roh-Y, Roh-Z].
    pub stock_size: Vec3,
    /// 4.0 mm Spannrand unten.
    pub clamp_depth_z: f32,
    /// Mindestens 1.0 mm Planaufmaß oben.
    #[allow(dead_code)] // bleibt Teil der Rohteil-Rechnung (Legacy-Pfad)
    pub top_allowance_z: f32,
    /// Ausrichtungsmatrix (Bauteilkoordinaten -> ausgerichtete Koordinaten).
    /// Bleibt für spätere CAD-Operationen erhalten.
    #[allow(dead_code)]
    pub transform: Mat3,
}

impl Default for StockResult {
    fn default() -> Self {
        Self {
            part_size: Vec3::ZERO,
            stock_size: Vec3::ZERO,
            clamp_depth_z: 4.0,
            top_allowance_z: 1.0,
            transform: Mat3::IDENTITY,
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
}

impl Default for StockParams {
    fn default() -> Self {
        Self {
            saw_x_mm: 5.0,
            clamp_z_mm: 4.0,
            plan_top_mm: 1.0,
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
        // Z: Rohteilplatte in 10-mm-Stufen über Spanntiefe + Planaufmaß
        let stock_z = ((part.z + p.clamp_z_mm + p.plan_top_mm) / 10.0).ceil() * 10.0;
        let top_allowance_z = stock_z - part.z - p.clamp_z_mm;
        StockResult {
            part_size: part,
            stock_size: Vec3::new(stock_x, stock_y, stock_z),
            clamp_depth_z: p.clamp_z_mm,
            top_allowance_z,
            transform: Mat3::IDENTITY,
        }
    }
}

/// Ermittelt die Hauptachsen (Trägheitsachsen/Kovarianz) und die
/// Ausrichtungsmatrix: Längste Seite -> X, mittlere -> Y, kleinste -> Z.
/// Ausrichtungsmatrix aus bereits gesammelten Vertices (alle Körper zusammen):
/// für die SolidCraft-Szenen-Transformation (rigide) vor der Anzeige.
pub fn alignment_for_vertices(vertices: &[Vec3]) -> Option<(Alignment, Vec3)> {
    let (align, size) = compute_alignment(vertices);
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

    // Kandidaten: Weltachsen + Kovarianzachsen (Eigenvektor-Approximation).
    let candidates = [
        Vec3::X,
        Vec3::Y,
        Vec3::Z,
        cov.x_axis.normalize_or_zero(),
        cov.y_axis.normalize_or_zero(),
        cov.z_axis.normalize_or_zero(),
    ];

    // Startwert: achsenparallele Bounding Box
    let mut bb_min = Vec3::splat(f32::MAX);
    let mut bb_max = Vec3::splat(f32::MIN);
    for v in vertices {
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

    (
        Alignment {
            rot,
            centroid,
            offset: Vec3::new(min_bound.x, min_bound.y, min_bound.z),
        },
        max_bound - min_bound,
    )
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

//! Dreiecksnetz-Datenstruktur und meshbasierte Geometrieoperationen.
//!
//! Dieser Baustein ist unabhängig von STEP, Rendering und UI. Er hält das
//! Anzeigenetz (Positionen, Dreiecke, geglättete Normalen, Kanten) und
//! enthält die Qualitätsfilter für die Visualisierung.

use glam::Vec3;
use std::collections::HashMap;

/// Anzeigenetz eines Körpers. Die B-Rep-Daten bleiben in [`crate::step`]
/// erhalten; dieses Netz wird ausschließlich für die Darstellung erzeugt.
#[derive(Clone, Debug, Default)]
pub struct MeshData {
    pub vertices: Vec<Vec3>,
    pub triangles: Vec<[usize; 3]>,
    /// Geglättete Flächennormale je Dreiecksecke (parallel zu `triangles`).
    /// Damit werden gekrümmte Flächen (z. B. Bohrungswände) weich schattiert,
    /// während echte Kanten scharf bleiben.
    pub normals: Vec<[Vec3; 3]>,
    /// Scharfe Kanten (Vertex-Indexpaare) für die CAD-Kantennachzeichnung.
    pub feature_edges: Vec<[usize; 2]>,
}

/// Berechnet geglättete Eckennormalen. Flächen, die unter einem Winkel von
/// weniger als ca. 40° aufeinandertreffen, gelten als eine glatte Oberfläche
/// (Zylinder-/Kegelwand). Echte Kanten (>= 40°) bleiben scharf, damit z. B.
/// die Deckfläche nicht in die Bohrungswand "verläuft".
pub fn compute_corner_normals(
    vertices: &[Vec3],
    triangles: &[[usize; 3]],
) -> Vec<[Vec3; 3]> {
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

/// Ermittelt scharfe Kanten (Flächenwinkel > 40°) eines verschweißten Netzes.
///
/// Die Schwelle ist bewusst identisch mit [`compute_corner_normals`]: Genau
/// dort, wo die Normalen nicht geglättet werden, liegt eine echte CAD-Kante.
/// Bei 20° würden fein tessellierte Rundungen (Bohrungsfase, Freiformfläche)
/// in lauter kleine Teilstücke zerfallen und wie „Gekritzel“ aussehen.
///
/// Randkanten (`count == 1`) werden nicht gezeichnet: In der Praxis stammen
/// sie aus offenen Tessellationssäumen und sehen wie Kratzer aus.
/// Sehr schlanke Dreiecke (Gewinde, Federn) werden ausgefiltert, weil ihre
/// Kanten unruhig wirken.
pub fn build_feature_edges(vertices: &[Vec3], triangles: &[[usize; 3]]) -> Vec<[usize; 2]> {
    const FLAT_COS: f32 = 0.766; // cos(40°)

    let mut face_normals: Vec<Vec3> = Vec::with_capacity(triangles.len());
    for tri in triangles {
        let (a, b, c) = (vertices[tri[0]], vertices[tri[1]], vertices[tri[2]]);
        let n = (b - a).cross(c - a).normalize_or_zero();
        face_normals.push(if n.is_finite() { n } else { Vec3::ZERO });
    }

    let sliver: Vec<bool> = triangles
        .iter()
        .map(|tri| {
            let (a, b, c) = (vertices[tri[0]], vertices[tri[1]], vertices[tri[2]]);
            let cross = (b - a).cross(c - a).length();
            let max_edge = (b - a)
                .length_squared()
                .max((c - b).length_squared())
                .max((a - c).length_squared());
            cross < 0.01 * max_edge
        })
        .collect();

    // [min, max] -> (Anzahl, Normale 1, Normale 2, beide Dreiecke brauchbar)
    let mut map: HashMap<[usize; 2], (u32, Vec3, Vec3, bool)> =
        HashMap::with_capacity(triangles.len() * 3);
    for (ti, tri) in triangles.iter().enumerate() {
        if face_normals[ti] == Vec3::ZERO {
            continue;
        }
        for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            let key = if a <= b { [a, b] } else { [b, a] };
            let e = map
                .entry(key)
                .or_insert((0, Vec3::ZERO, Vec3::ZERO, true));
            e.0 += 1;
            if e.0 == 1 {
                e.1 = face_normals[ti];
            } else if e.0 == 2 {
                e.2 = face_normals[ti];
            }
            e.3 &= !sliver[ti];
        }
    }

    let mut out = Vec::new();
    for ([a, b], (count, n1, n2, clean)) in map {
        if count == 2 && clean && n1.dot(n2).abs() < FLAT_COS {
            out.push([a, b]);
        }
    }
    out
}

/// Verschmilzt räumlich identische Vertices (z. B. an Nahtstellen zwischen
/// Flächen) und entfernt danach entartete Dreiecke. Das schließt Haarrisse,
/// die sonst durch getrennte Vertex-Kopien entstehen.
pub fn weld_and_clean(vertices: Vec<Vec3>, triangles: Vec<[usize; 3]>) -> MeshData {
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

/// Entfernt nicht mehr verwendete Vertices und kompaktiert die Indizes.
pub fn compact_mesh(vertices: Vec<Vec3>, triangles: Vec<[usize; 3]>) -> MeshData {
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
        feature_edges: Vec::new(),
    };
    mesh.normals = compute_corner_normals(&mesh.vertices, &mesh.triangles);
    mesh
}

/// Erzeugt ein einfaches Demo-Bauteil (Quader) für den Start ohne STEP-Datei.
pub fn generate_sample_milled_part(lx: f32, ly: f32, lz: f32) -> MeshData {
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
        [0, 2, 3],
        [4, 6, 5],
        [4, 7, 6],
        [0, 4, 5],
        [0, 5, 1],
        [2, 6, 7],
        [2, 7, 3],
        [0, 3, 7],
        [0, 7, 4],
        [1, 5, 6],
        [1, 6, 2],
    ];
    let mut mesh = MeshData {
        vertices: v,
        triangles,
        normals: Vec::new(),
        feature_edges: Vec::new(),
    };
    mesh.normals = compute_corner_normals(&mesh.vertices, &mesh.triangles);
    mesh.feature_edges = build_feature_edges(&mesh.vertices, &mesh.triangles);
    mesh
}

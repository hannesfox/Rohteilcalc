//! Analytischer Tessellierer für elementare STEP-Flächen (Ebene, Zylinder,
//! Kegel).
//!
//! Rezept nach [Solidcraft] (MIT/Apache-2.0): Jede Randkurve wird exakt
//! geteilt, jeder Randpunkt analytisch in den `(u, v)`-Parameterraum seiner
//! eigenen Fläche abgebildet — niemals über eine Nearest-Parameter-Suche wie
//! bei truck-`robust_triangulation`, die bei getrimmten Kegel-/Zylinder-
//! flächen Punkte auf das falsche Flächenblatt legen kann. Gefüllt wird mit
//! einer Constrained-Delaunay-Triangulation (`spade`, dieselbe Bibliothek wie
//! in Solidcraft); Dreiecke außerhalb der Randschleifen (Löcher, Außenraum)
//! werden über eine Paritäts-Flutfüllung aussortiert.
//!
//! [Solidcraft]: https://github.com/tangency-labs/solidcraft

use spade::handles::{FixedFaceHandle, FixedVertexHandle, InnerTag};
use spade::{ConstrainedDelaunayTriangulation, Triangulation};
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use truck_stepio::r#in::alias::re_exports::{
    BoundedCurve, InnerSpace, ParameterDivision1D, Point3, Transform, Vector3,
};
use truck_stepio::r#in::alias::{Curve3D, ElementarySurface, Surface};
use truck_topology::compress::{CompressedEdge, CompressedEdgeIndex, CompressedFace};

type Cdt = ConstrainedDelaunayTriangulation<spade::Point2<f64>>;

/// Debug: `MESHER_DEBUG=1` macht jede abgelehnte Fläche mit Grund sichtbar.
macro_rules! dbg_none {
    ($($t:tt)*) => {{
        if std::env::var_os("MESHER_DEBUG").is_some() {
            eprintln!("[mesher-fail] {}", format_args!($($t)*));
        }
        return None;
    }};
}

/// Quadrat des Schwellwerts (mm) für Deduplizierung benachbarter Randpunkte.
const DUP_EPS2: f64 = 1e-16;
const TAU: f64 = std::f64::consts::TAU;

/// Tessellierung einer Fläche: Weltpunkte (Datei-Einheiten) + Dreiecke.
#[derive(Debug, Clone)]
pub struct FaceMesh {
    pub vertices: Vec<Point3>,
    pub triangles: Vec<[usize; 3]>,
}

/// Analytische Abbildung von Weltpunkten in `(u, v)` und zurück.
///
/// Rohe Parameter: Ebene `(s, t)` in mm; Rotationsfläche `(u entlang Mantel-
/// linie, φ)`. `u_scale`/`v_scale` skalieren auf mm-Maßstab für die CDT.
enum Map {
    Plane {
        origin: Point3,
        u_axis: Vector3,
        v_axis: Vector3,
    },
    Revolved {
        origin: Point3,
        /// Einheitsvektor der Rotationsachse (Weltkoordinaten).
        axis: Vector3,
        /// Orthonormales Fundament senkrecht zur Achse (für φ).
        x_axis: Vector3,
        y_axis: Vector3,
        /// Mantellinie: Profilpunkt bei u = 0 und u = 1.
        profile: (Point3, Point3),
        /// Höhenänderung entlang der Mantellinie (Projektion auf Achse).
        dh: f64,
        u_scale: f64,
        v_scale: f64,
    },
    Torus {
        origin: Point3,
        axis: Vector3,
        x_axis: Vector3,
        y_axis: Vector3,
        /// Großer Radius (Abstand Achse–Rohrzentrum) und Rohrradius.
        large: f64,
        small: f64,
        u_scale: f64,
        v_scale: f64,
    },
}

impl Map {
    /// Baut die Abbildung. Liefert zusätzlich die Orientierung der
    /// truck-Processor-Surface (nötig für die Winding-Entscheidung).
    /// `None`, wenn die Fläche ungeeignet oder entartet ist.
    fn from_surface(surface: &Surface) -> Option<(Map, bool)> {        let Surface::ElementarySurface(elementary) = surface else {
            dbg_none!("keine ElementarySurface");
        };
        match elementary.as_ref() {
            ElementarySurface::Plane(plane) => {
                // Die Ebene ist bereits in Weltkoordinaten eingebettet
                // (axis2 placement in die Punkte eingebacken).
                let map = Map::Plane {
                    origin: plane.origin(),
                    u_axis: plane.u_axis(),
                    v_axis: plane.v_axis(),
                };
                Some((map, true))
            }
            ElementarySurface::CylindricalSurface(cone) => Map::revolved(cone),
            ElementarySurface::ConicalSurface(cone) => Map::revolved(cone),
            ElementarySurface::ToroidalSurface(torus) => Map::toroidal(torus),
            _ => None,
        }
    }

    fn revolved(
        cone: &truck_stepio::r#in::alias::CylindricalSurface,
    ) -> Option<(Map, bool)> {
        let line = *cone.entity().entity_curve();
        let origin = cone.transform().transform_point(cone.entity().origin());
        let axis = cone.transform().transform_vector(cone.entity().axis());
        let axis_len = axis.magnitude();
        if !(axis_len.is_finite() && axis_len > 1e-9) {
            dbg_none!("Achse entartet");
        }
        let axis = axis / axis_len;
        let a = cone.transform().transform_point(line.0);
        let b = cone.transform().transform_point(line.1);
        let ruling_vector = diff(b, a);
        let dh = ruling_vector.dot(axis);
        if !dh.is_finite() || dh.abs() < 1e-9 {
            dbg_none!("Mantellinie ohne Hohenunterschied");
        }
        // Fundament senkrecht zur Achse für den Winkel φ.
        let reference = if axis.x.abs() < 0.9 {
            Vector3::new(1.0, 0.0, 0.0)
        } else {
            Vector3::new(0.0, 1.0, 0.0)
        };
        let x_axis = axis.cross(reference);
        let x_len = x_axis.magnitude();
        if !(x_len.is_finite() && x_len > 1e-9) {
            dbg_none!("Achsfundament entartet");
        }
        let x_axis = x_axis / x_len;
        let ruling = ruling_vector.magnitude();
        if !(ruling.is_finite() && ruling > 1e-9) {
            dbg_none!("Mantellinie null");
        }
        Some((
            Map::Revolved {
                origin,
                axis,
                x_axis,
                y_axis: axis.cross(x_axis),
                profile: (a, b),
                dh,
                u_scale: ruling,
                v_scale: 1.0,
            },
            cone.orientation(),
        ))
    }

    fn toroidal(
        torus: &truck_stepio::r#in::alias::ToroidalSurface,
    ) -> Option<(Map, bool)> {
        let t = torus.transform();
        let entity = torus.entity();
        let origin = t.transform_point(entity.center());
        let axis = t.transform_vector(Vector3::new(0.0, 0.0, 1.0));
        let axis_len = axis.magnitude();
        if !(axis_len.is_finite() && axis_len > 1e-9) {
            dbg_none!("Torus-Achse entartet");
        }
        let axis = axis / axis_len;
        let large = entity.large_radius();
        let small = entity.small_radius();
        if !(large.is_finite() && small.is_finite() && large.abs() > 1e-9 && small.abs() > 1e-9) {
            dbg_none!("Torus-Radien entartet");
        }
        let reference = if axis.x.abs() < 0.9 {
            Vector3::new(1.0, 0.0, 0.0)
        } else {
            Vector3::new(0.0, 1.0, 0.0)
        };
        let x_axis = axis.cross(reference);
        let x_len = x_axis.magnitude();
        if !(x_len.is_finite() && x_len > 1e-9) {
            dbg_none!("Torus-Fundament entartet");
        }
        Some((
            Map::Torus {
                origin,
                axis,
                x_axis: x_axis / x_len,
                y_axis: axis.cross(x_axis / x_len),
                large,
                small,
                u_scale: (large + small).abs().max(1.0),
                v_scale: small.abs(),
            },
            torus.orientation(),
        ))
    }

    /// φ-Skalierung auf den mittleren Grenzradius setzen (mm-Maßstab).
    fn set_angle_scale(&mut self, mean_radius: f64) {
        if let Map::Revolved { v_scale, .. } = self {
            *v_scale = mean_radius;
        }
    }

    /// Periodizität der (u, v)-Koordinaten für das Entfalten.
    fn periods(&self) -> (Option<f64>, Option<f64>) {
        match self {
            Map::Plane { .. } => (None, None),
            Map::Revolved { .. } => (None, Some(TAU)),
            Map::Torus { .. } => (Some(TAU), Some(TAU)),
        }
    }

    /// Roher Parameter eines Weltpunktes am Rand bzw. auf der Fläche.
    fn params(&self, point: Point3) -> Option<(f64, f64)> {
        match self {
            Map::Plane {
                origin,
                u_axis,
                v_axis,
            } => {
                let d = diff(point, *origin);
                let (aa, ab, bb) = (u_axis.dot(*u_axis), u_axis.dot(*v_axis), v_axis.dot(*v_axis));
                let det = aa * bb - ab * ab;
                if !det.is_finite() || det.abs() < 1e-24 {
                    return None;
                }
                let (da, db) = (d.dot(*u_axis), d.dot(*v_axis));
                let s = (bb * da - ab * db) / det;
                let t = (aa * db - ab * da) / det;
                if s.is_finite() && t.is_finite() {
                    Some((s, t))
                } else {
                    None
                }
            }
            Map::Revolved {
                origin,
                axis,
                x_axis,
                y_axis,
                profile,
                dh,
                ..
            } => {
                let rel = diff(point, *origin);
                let height = rel.dot(*axis);
                let radial = rel - height * *axis;
                let phi = radial.dot(*y_axis).atan2(radial.dot(*x_axis));
                let (a, _) = *profile;
                let height_a = diff(a, *origin).dot(*axis);
                let u = (height - height_a) / dh;
                if u.is_finite() && phi.is_finite() {
                    Some((u, phi))
                } else {
                    None
                }
            }
            Map::Torus {
                origin,
                axis,
                x_axis,
                y_axis,
                large,
                ..
            } => {
                let rel = diff(point, *origin);
                let height = rel.dot(*axis);
                let radial = rel - height * *axis;
                let radius = radial.magnitude();
                let phi = radial.dot(*y_axis).atan2(radial.dot(*x_axis));
                let v = height.atan2(radius - large);
                if v.is_finite() && phi.is_finite() {
                    Some((phi, v))
                } else {
                    None
                }
            }
        }
    }

    /// Weltpunkt zu rohem `(u, v)`.
    fn eval(&self, u: f64, v: f64) -> Option<Point3> {
        match self {
            Map::Plane {
                origin,
                u_axis,
                v_axis,
            } => Some(*origin + u * *u_axis + v * *v_axis),
            Map::Revolved {
                origin,
                axis,
                x_axis,
                y_axis,
                profile,
                ..
            } => {
                let (a, b) = *profile;
                let on_profile = a + diff(b, a) * u;
                let rel = diff(on_profile, *origin);
                let height = rel.dot(*axis);
                let radius = (rel - height * *axis).magnitude();
                let radial = (x_axis * v.cos() + y_axis * v.sin()) * radius;
                Some(*origin + axis * height + radial)
            }
            Map::Torus {
                origin,
                axis,
                x_axis,
                y_axis,
                large,
                small,
                ..
            } => {
                let ring = large + small * v.cos();
                let radial = (x_axis * u.cos() + y_axis * u.sin()) * ring;
                Some(*origin + axis * (small * v.sin()) + radial)
            }
        }
    }

    /// Mittelwert der Grenzradien (für die φ-Skalierung, mm-Maßstab).
    fn mean_radius(&self, points: &[Point3]) -> f64 {
        match self {
            Map::Plane { .. } => 1.0,
            Map::Revolved { origin, axis, .. } | Map::Torus { origin, axis, .. } => {
                let mut sum = 0.0;
                let mut n = 0usize;
                for p in points {
                    let rel = diff(*p, *origin);
                    let h = rel.dot(*axis);
                    let r = (rel - h * *axis).magnitude();
                    if r.is_finite() {
                        sum += r;
                        n += 1;
                    }
                }
                let mean = if n == 0 { 0.0 } else { sum / n as f64 };
                if mean.is_finite() && mean > 1e-7 {
                    mean
                } else {
                    1.0
                }
            }
        }
    }
}

/// Dreht einen Abstand in den Bereich `(-period/2, period/2]`.
fn wrap(dist: f64, period: f64) -> f64 {
    dist - period * (dist / period).round()
}

/// Tesselliert eine einzelne CompressedFace analytisch.
///
/// * `face` — die STEP-Fläche (Surface plus Randschleifen).
/// * `shell_vertices`/`shell_edges` — die zugehörigen Shell-Daten.
/// * `tol` — Tessellierungstoleranz in mm (Randkurven-Teilung).
///
/// `None` heißt: nicht analytisch behandelbar; der Aufrufer fällt für diese
/// Fläche auf truck zurück.
pub fn tessellate_face(
    face: &CompressedFace<Surface>,
    shell_vertices: &[Point3],
    shell_edges: &[CompressedEdge<Curve3D>],
    tol: f64,
    tag: &str,
) -> Option<FaceMesh> {
    let _ = shell_vertices;
    let (mut map, proc_orientation) = Map::from_surface(&face.surface)?;

    // Winding: truck-stepio übernimmt same_sense in die Processor-Orientierung
    // und dreht das Polygon um, wenn face.orientation falsch ist. Analytisch
    // erzeugen wir Dreiecke CCW um die truck-Oberflächennormale; gedreht wird
    // also genau dann, wenn truck ebenfalls drehen würde (siehe
    // truck-meshalgo::MeshedShape::to_polygon).
    let flip = proc_orientation && !face.orientation;

    // 1) Randkurven exakt in Welt-Punkte teilen.
    let mut loops3d: Vec<Vec<Point3>> = Vec::with_capacity(face.boundaries.len());
    for boundary in &face.boundaries {
        let mut loop_pts: Vec<Point3> = Vec::new();
        for edge_idx in boundary {
            if collect_edge_points(&shell_edges[edge_idx.index], edge_idx, tol, &mut loop_pts).is_none() {
                dbg_none!("Kante nicht teilbar");
            }
        }
        // Schließenden Duplikatpunkt entfernen.
        if loop_pts.len() > 1 {
            let first = loop_pts[0];
            let last = *loop_pts.last().unwrap();
            if dist2(first, last) < DUP_EPS2 {
                loop_pts.pop();
            }
        }
        if loop_pts.len() < 3 {
            dbg_none!("Schleife < 3 Punkte");
        }
        loops3d.push(loop_pts);
    }

    // 2) φ-Skalierung auf den mittleren Grenzradius; Skalen holen.
    let all_points: Vec<Point3> = loops3d.iter().flatten().copied().collect();
    map.set_angle_scale(map.mean_radius(&all_points));
    let scales = match &map {
        Map::Plane { .. } => (1.0, 1.0),
        Map::Revolved {
            u_scale, v_scale, ..
        }
        | Map::Torus {
            u_scale, v_scale, ..
        } => (*u_scale, *v_scale),
    };
    if !(scales.0.is_finite() && scales.0 > 1e-9 && scales.1.is_finite() && scales.1 > 1e-9) {
        dbg_none!("Skalen entartet");
    }

    // 3) Randpunkte nach (u, v); periodische Koordinaten einer Schleife
    //    fortlaufend entfalten, damit volle Windungen als Band erscheinen und
    //    die CDT zusammenhängt.
    let (period_u, period_v) = map.periods();
    let mut pts3d_aligned: Vec<Vec<Point3>> = loops3d.clone();    let mut loops_uv: Vec<Vec<spade::Point2<f64>>> = Vec::with_capacity(loops3d.len());
    for (li, loop_pts) in loops3d.iter().enumerate() {
        // a) entfaltete Rohparameter je Punkt
        let mut raw: Vec<(f64, f64)> = Vec::with_capacity(loop_pts.len());
        let mut prev_param: Option<(f64, f64)> = None;
        for &p in loop_pts {
            let (mut u, mut v) = match map.params(p) {
                Some(x) => x,
                None => dbg_none!("Punkt nicht mappable"),
            };
            if let Some((pu, pv)) = prev_param {
                if let Some(per) = period_u {
                    u = pu + wrap(u - pu, per);
                }
                if let Some(per) = period_v {
                    v = pv + wrap(v - pv, per);
                }
            }
            prev_param = Some((u, v));
            raw.push((u, v));
        }
        // Mehrfach umwickelte Schleifen (-crossing bores): die Paritäts-Flut-
        // fällung kann spiralige UV-Schleifen nicht sauber füllen; truck
        // tregt die Fläche korrekt.
        if let Some(per) = period_u {
            let span = raw.iter().map(|r| r.0).fold(
                (f64::INFINITY, f64::NEG_INFINITY),
                |mn, x| (mn.0.min(x), mn.1.max(x)),
            );
            if span.1 - span.0 > per + 1e-3 {
                dbg_none!("u-Schleife mehrfach umwickelt ({:.3}>{:.3})", span.1 - span.0, per);
            }
        }
        if let Some(per) = period_v {
            let span = raw.iter().map(|r| r.1).fold(
                (f64::INFINITY, f64::NEG_INFINITY),
                |mn, x| (mn.0.min(x), mn.1.max(x)),
            );
            if span.1 - span.0 > per + 1e-3 {
                dbg_none!("v-Schleife mehrfach umwickelt ({:.3}>{:.3})", span.1 - span.0, per);
            }
        }
        if std::env::var_os("MESHER_DEBUG").is_some() {
            eprintln!(
                "[mesher-uv {tag}] loop{li}: n={} uv_raw=({:.4},{:.4})..({:.4},{:.4})",
                raw.len(),
                raw.first().map_or(0.0, |r| r.0),
                raw.first().map_or(0.0, |r| r.1),
                raw.last().map_or(0.0, |r| r.0),
                raw.last().map_or(0.0, |r| r.1)
            );
            for (k, (u, v)) in raw.iter().enumerate() {
                eprintln!("[mesher-uv {tag}] loop{li}[{k}] = ({u:.4},{v:.4})");
            }
        }
        // b) Naht schließen: Die entfaltete Schleife endet bei `k` ganzen
        //    Perioden neben dem Start. Ohne Hilfspunkt liefe die Schlusskante
        //    quer übers Paramet.band (Naht), würde als Constraint scheitern
        //    und die Region-Flutfüllung leakte → Zelt-Artefakte. Der
        //    angehängte Punkt ist derselbe 3D-Punkt wie der Startpunkt, aber
        //    um `k` Perioden verschoben — die Schlusskante bleibt kurz.
        let (u0, v0) = raw[0];
        let (ul, vl) = raw[raw.len() - 1];
        let (ku, kv) = (
            period_u.map_or(0.0, |per| ((ul - u0) / per).round()),
            period_v.map_or(0.0, |per| ((vl - v0) / per).round()),
        );
        if ku != 0.0 || kv != 0.0 {
            let shifted = (
                u0 + ku * period_u.unwrap_or(0.0),
                v0 + kv * period_v.unwrap_or(0.0),
            );
            raw.push(shifted);
            pts3d_aligned[li].push(loop_pts[0]);
        }
        // c) skalieren
        let mut uv_loop = Vec::with_capacity(raw.len());
        for &(u, v) in &raw {
            let uv = spade::Point2::new(u * scales.0, v * scales.1);
            if !uv.x.is_finite() || !uv.y.is_finite() {
                dbg_none!("uv nicht endlich");
            }
            uv_loop.push(uv);
        }
        loops_uv.push(uv_loop);
    }

    // 3c) uv-Schleifen müssen einfach sein (siehe Funktionsdoku); Schlitze
    //     leaken flächenneutral und entstehen sonst unsichtbar.
    if let Err(what) = loops_uv_simple(&loops_uv) {
        if std::env::var_os("MESHER_DEBUG").is_some() {
            eprintln!("[mesher-why {tag}] {what}");
        }
        dbg_none!("uv-Schleife ueberlappt/kreuzt sich (Schlitz) -> truck");
    }

    // 4) CDT über alle Randschleifen, innere Region selektieren.
    let mut vertices: Vec<Point3> = Vec::new();
    let mut triangles: Vec<[usize; 3]> = Vec::new();
    triangulate_region(
        &map,
        &pts3d_aligned,
        &loops_uv,
        scales,
        tag,
        &mut vertices,
        &mut triangles,
    )?;

    if triangles.is_empty() {
        dbg_none!("keine Dreiecke");
    }
    if flip {
        for tri in &mut triangles {
            tri.swap(1, 2);
        }
    }
    if let Ok(path) = std::env::var("MESHER_UVDUMP") {
        use std::io::Write;
        let want = std::env::var("MESHER_UVDUMP_TAG").ok();
        let tag_ok = want.as_deref().map_or(true, |w| tag == w);
        if tag_ok {
            if let Ok(mut f) = std::fs::File::create(&path) {
            let _ = writeln!(f, "# loops_uv (skaliert), loops3d, vertices, triangles");
            for (l3, lu) in pts3d_aligned.iter().zip(loops_uv.iter()) {
                let _ = writeln!(f, "loop3d");
                for p in l3 {
                    let _ = writeln!(f, "p {} {} {}", p.x, p.y, p.z);
                }
                let _ = writeln!(f, "loopuv");
                for uv in lu {
                    let _ = writeln!(f, "uv {} {}", uv.x, uv.y);
                }
            }
            let _ = writeln!(f, "mesh");
            for (v, p) in vertices.iter().enumerate() {
                let uv = map.params(*p).map_or((f64::NAN, f64::NAN), |(u, w)| (u, w));
                let _ = writeln!(f, "v {} {} {} {} {} {}", v, p.x, p.y, p.z, uv.0, uv.1);
            }
                for t in &triangles {
                    let _ = writeln!(f, "t {} {} {}", t[0], t[1], t[2]);
                }
            }
        }
    }
    Some(FaceMesh {
        vertices,
        triangles,
    })
}

/// Teilt die Randpolyline einer Kante exakt und hängt sie sortiert an `out`.
fn collect_edge_points(
    edge: &CompressedEdge<Curve3D>,
    edge_idx: &CompressedEdgeIndex,
    tol: f64,
    out: &mut Vec<Point3>,
) -> Option<()> {
    let range = edge.curve.range_tuple();
    let (_, pts) = edge.curve.parameter_division(range, tol);
    if pts.len() < 2 {
        dbg_none!("parameter_division < 2");
    }
    let mut pts = pts;
    if !edge_idx.orientation {
        pts.reverse();
    }
    for p in pts {
        if !p.x.is_finite() || !p.y.is_finite() || !p.z.is_finite() {
            dbg_none!("Kantenpunkt nicht endlich");
        }
        if let Some(last) = out.last() {
            if dist2(*last, p) < DUP_EPS2 {
                continue;
            }
        }
        out.push(p);
    }
    Some(())
}

/// Quadrierter Abstand zweier Punkte.
fn dist2(a: Point3, b: Point3) -> f64 {
    let d = Vector3::new(a.x - b.x, a.y - b.y, a.z - b.z);
    d.magnitude2()
}

/// Differenzvektor a − b (Punkt-Differenz ohne cgmath-Operator-Mix).
fn diff(a: Point3, b: Point3) -> Vector3 {
    Vector3::new(a.x - b.x, a.y - b.y, a.z - b.z)
}

/// Orientierung von `c` relativ zur Geraden a→b (Kreuzprodukt).
fn orient2d(
    ax: f64,
    ay: f64,
    bx: f64,
    by: f64,
    c: &spade::Point2<f64>,
) -> f64 {
    (bx - ax) * (c.y - ay) - (by - ay) * (c.x - ax)
}

/// Prüft, dass die Randschleifen in uv einfache Polygone bilden.
///
/// Kreuzende oder kollinear überlappende Grenzsegmente (Schlitze, die dieselbe
/// Wandkante doppelt belegen — typisch bei aneinanderstoßenden
/// Kreuzbohrungskanten) leaken flächenneutral durch die Paritäts-Flutfüllung
/// und produzieren gequetschte Flächen; der truck-Fallback rendert sie korrekt.
/// Exakt gemeinsame Endpunkte (Figur-Acht, Naht-Nachbarsegmente) sind erlaubt.
fn loops_uv_simple(loops_uv: &[Vec<spade::Point2<f64>>]) -> Result<(), String> {
    // Segmente als (a, b) sammeln; Schleifen-/Segmentindex und Schleifenlänge
    // merken, um echte Nachbarsegmente zu erkennen.
    let mut segs: Vec<(usize, usize, usize, [f64; 2], [f64; 2])> = Vec::new();
    for (li, uv_loop) in loops_uv.iter().enumerate() {
        let n = uv_loop.len();
        if n < 2 {
            continue;
        }
        for k in 0..n {
            let a = [uv_loop[k].x, uv_loop[k].y];
            let b = [uv_loop[(k + 1) % n].x, uv_loop[(k + 1) % n].y];
            if a == b {
                continue; // entartet
            }
            segs.push((li, k, n, a, b));
        }
    }
    // Absolute Toleranz in uv-Einheiten (skaliert ≙ mm): unterhalb jeder
    // sichtbaren Skala, aber über Rechenrauschen von `params`.
    const EPS: f64 = 1e-6;
    let seg_len2 = |a: &[f64; 2], b: &[f64; 2]| -> f64 {
        let d = (b[0] - a[0], b[1] - a[1]);
        d.0 * d.0 + d.1 * d.1
    };
    for (i, &(li, ki, n, ai, bi)) in segs.iter().enumerate() {
        for &(lj, kj, nj, aj, bj) in segs[i + 1..].iter() {
            // Benachbarte Segmente derselben Schleife teilen legitime Ecken.
            if li == lj
                && n == nj
                && (ki + 1 == kj || kj + 1 == ki || (ki == 0 && kj == n - 1) || (kj == 0 && ki == n - 1))
            {
                continue;
            }
            let ab2 = seg_len2(&ai, &bi);
            let cd2 = seg_len2(&aj, &bj);
            if ab2 <= EPS * EPS || cd2 <= EPS * EPS {
                continue;
            }
            // Exakt geteilte Endpunkte sind legitime Ecken/Figur-Acht-Knoten;
            // die CDT dedupliziert sie zu einem Knoten und benachbarte
            // Teilsegmente werden vom Constraint-Splitting behandelt.
            if same_pt(&aj, &ai) || same_pt(&aj, &bi) || same_pt(&bj, &ai) || same_pt(&bj, &bi) {
                continue;
            }
            let lab = ab2.sqrt();
            let lcd = cd2.sqrt();
            let mk = |x: f64, y: f64| spade::Point2::new(x, y);
            // Vorzeichenbehafte Abstände der betrachteten Endpunkte.
            let hc = orient2d(ai[0], ai[1], bi[0], bi[1], &mk(aj[0], aj[1]));
            let hd = orient2d(ai[0], ai[1], bi[0], bi[1], &mk(bj[0], bj[1]));
            let ha = orient2d(aj[0], aj[1], bj[0], bj[1], &mk(ai[0], ai[1]));
            let hb = orient2d(aj[0], aj[1], bj[0], bj[1], &mk(bi[0], bi[1]));
            // Kollinear überlappende Paare (Schlitze): beide Enden des einen
            // Segments (fast) auf der Geraden des anderen und die
            // Projektionsintervalle überlappen über positive Länge.
            if hc.abs() <= EPS * lab && hd.abs() <= EPS * lab {
                let abx = bi[0] - ai[0];
                let aby = bi[1] - ai[1];
                let proj = |p: &[f64; 2]| (p[0] - ai[0]) * abx + (p[1] - ai[1]) * aby;
                let (mut ta, mut tb) = (proj(&ai), proj(&bi));
                if tb < ta {
                    std::mem::swap(&mut ta, &mut tb);
                }
                let (mut tc, mut td) = (proj(&aj), proj(&bj));
                if td < tc {
                    std::mem::swap(&mut tc, &mut td);
                }
                if tb.min(td) - ta.max(tc) > EPS * (tb - ta).max(1.0) {
                    return Err(format!(
                        "kollinear: ({:.4},{:.4})->({:.4},{:.4}) vs ({:.4},{:.4})->({:.4},{:.4}) loop {li}/{lj}",
                        ai[0], ai[1], bi[0], bi[1], aj[0], aj[1], bj[0], bj[1]
                    ));
                }
                continue; // parallel disjunkt: geprüft, fertig
            }
            // Echter Kreuzungspunkt strikt im Inneren beider Segmente
            // ((selbst-)kreuzendes Polygon: Paritäts-Füllung sinnlos).
            if hc * hd < 0.0
                && hc.abs() > EPS * lab
                && hd.abs() > EPS * lab
                && ha * hb < 0.0
                && ha.abs() > EPS * lcd
                && hb.abs() > EPS * lcd
            {
                return Err(format!(
                    "kreuzt: ({:.4},{:.4})->({:.4},{:.4}) vs ({:.4},{:.4})->({:.4},{:.4}) loop {li}/{lj}",
                    ai[0], ai[1], bi[0], bi[1], aj[0], aj[1], bj[0], bj[1]
                ));
            }
        }
    }
    Ok(())
}

fn same_pt(p: &[f64; 2], q: &[f64; 2]) -> bool {
    (p[0] - q[0]).abs() < 1e-12 && (p[1] - q[1]).abs() < 1e-12
}



/// Füllt die von den Randschleifen beschränkte Region mit einer
/// Constrained-Delaunay-Triangulation und hängt Dreiecke/Punkte an.
///
/// Randknoten erhalten exakt die 3D-Punkte der geteilten Randkurven;
/// Innenknoten werden auf der Fläche ausgewertet. Dreiecke außerhalb der
/// Randschleifen (Insel, Löcher) fallen der Paritäts-Flutfällung zum Opfer.
fn triangulate_region(
    map: &Map,
    loops3d: &[Vec<Point3>],
    loops_uv: &[Vec<spade::Point2<f64>>],
    scales: (f64, f64),
    tag: &str,
    vertices: &mut Vec<Point3>,
    triangles: &mut Vec<[usize; 3]>,
) -> Option<()> {
    // Zulässiger Parameterbereich (skalierte (u, v)):	die Boundingbox aller
    // Randschleifenpunkte. Innenknoten außerhalb davon liegen auf
    // extrapolierter Fläche (Segel-Artefakte bei gekoppelten Bohrungen) und
    // ihre Dreiecke werden verworfen.
    let mut uv_min = spade::Point2::new(f64::INFINITY, f64::INFINITY);
    let mut uv_max = spade::Point2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for uv_loop in loops_uv {
        for uv in uv_loop {
            uv_min.x = uv_min.x.min(uv.x);
            uv_min.y = uv_min.y.min(uv.y);
            uv_max.x = uv_max.x.max(uv.x);
            uv_max.y = uv_max.y.max(uv.y);
        }
    }
    let mut cdt = Cdt::new();
    // Punkt-Deduplizierung nach Bitmuster: berühren sich zwei Schleifen an
    // einem Punkt (Tangentenscheitel/Apex), teilen sie einen CDT-Knoten.
    let bits = |p: &spade::Point2<f64>| (p.x.to_bits(), p.y.to_bits());
    let mut position_index: HashMap<(u64, u64), FixedVertexHandle> = HashMap::new();
    // CDT-Handle → Index in `vertices`.
    let mut index_of: HashMap<FixedVertexHandle, usize> = HashMap::new();
    // Handle jedes Grenzknotens in Einfügereihenfolge (Schleifenfolge).
    let mut handles: Vec<Vec<FixedVertexHandle>> = Vec::with_capacity(loops_uv.len());

    for (li, uv_loop) in loops_uv.iter().enumerate() {
        let mut this_loop = Vec::with_capacity(uv_loop.len());
        for (k, uv) in uv_loop.iter().enumerate() {
            let h = match position_index.entry(bits(uv)) {
                Entry::Occupied(slot) => *slot.get(),
                Entry::Vacant(slot) => {
let h = match cdt.insert(*uv) { Ok(h) => h, Err(_) => dbg_none!("CDT-Einfuegespunkt doppelt/ungueltig") };
                    slot.insert(h);
                    h
                }
            };
            index_of.entry(h).or_insert_with(|| {
                let i = vertices.len();
                vertices.push(loops3d[li][k]);
                i
            });
            this_loop.push(h);
        }
        handles.push(this_loop);
    }
    if cdt.num_vertices() < 3 {
        dbg_none!("CDT zu klein");
    }

    // Schlitzseiten: eine Schleife kann am selben Kantenpaar je Richtung
    // entlanglaufen (Gravur-Glyphen, die doppelt gelaufene Umrandungen
    // haben). Solche Seiten trennen die Fläche auf keiner Seite; nur
    // unpaarig gelaufene Kanten sind echte Barrieren (zählt wie bei
    // solidcraft).
    let barrier = edge_runs(&handles);

    // Randschleifen als Constraints eintragen; Schlitze und kollidierende
    // Segmente weglassen — die Region-Füllung behandelt sie über `barrier`.
    for this_loop in &handles {
        let n = this_loop.len();
        for k in 0..n {
            let (a, b) = (this_loop[k], this_loop[(k + 1) % n]);
            if a == b || cdt.exists_constraint(a, b) {
                continue;
            }
            if !cdt.can_add_constraint(a, b) {
                // Ein kollinear überlappendes Segment (Gravur-Glyphen-Schlitze)
                // wird an allen bereits vorhandenen auf der Strecke liegenden
                // Knoten aufgeteilt und je Teil einzeln eingetragen. Ohne die
                // Aufteilung leakt die Paritäts-Flutfüllung durch die Lücke
                // und die Buchstaben werden nicht ausgestanzt.
                let pa = cdt.vertex(a).position();
                let pb = cdt.vertex(b).position();
                let seg = spade::Point2::new(pb.x - pa.x, pb.y - pa.y);
                let seg_len2 = seg.x * seg.x + seg.y * seg.y;
                let mut chain: Vec<(f64, FixedVertexHandle)> = Vec::new();
                for v in cdt.vertices() {
                    let h = v.fix();
                    if h == a || h == b {
                        continue;
                    }
                    let p = v.position();
                    let rel = spade::Point2::new(p.x - pa.x, p.y - pa.y);
                    let t = (rel.x * seg.x + rel.y * seg.y) / seg_len2;
                    if !(t > 1e-9 && t < 1.0 - 1e-9) {
                        continue;
                    }
                    let cross = rel.x * seg.y - rel.y * seg.x;
                    // kollinear genug, als Punkt der Strecke zu gelten
                    let eps = (seg_len2.sqrt() * 1e-4).max(1e-12);
                    if cross.abs() / seg_len2.sqrt() <= eps {
                        chain.push((t, h));
                    }
                }
                chain.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
                let mut prev = a;
                let mut split_any = false;
                for (_, h) in chain.drain(..) {
                    if h == prev || cdt.exists_constraint(prev, h) {
                        prev = h;
                        continue;
                    }
                    if cdt.can_add_constraint(prev, h) {
                        cdt.add_constraint(prev, h);
                        split_any = true;
                    }
                    prev = h;
                }
                if prev != b && cdt.can_add_constraint(prev, b) {
                    cdt.add_constraint(prev, b);
                    split_any = true;
                }
                if !split_any && std::env::var_os("MESHER_DEBUG").is_some() {
                    eprintln!(
                        "[mesher-skip {tag}] Constraint nicht eintragbar: a=({:.3},{:.3}) b=({:.3},{:.3})",
                        cdt.vertex(a).position().x,
                        cdt.vertex(a).position().y,
                        cdt.vertex(b).position().x,
                        cdt.vertex(b).position().y
                    );
                }
                continue;
            }
            cdt.add_constraint(a, b);
        }
    }

    // Region: Flutfüllung von der konvexen Hülle, Parität bei jedem
    // Durchqueren einer unpaarig gelaufenen Constraint-Kante umgekehrt
    // (Löcher bleiben außen, Schlitze verbinden).
    let region = region_of(&cdt, &barrier);

    // Leak-Prüfung: die selectierte Region darf nicht typischerweise größer
    // sein als die umrandete Schleifenfläche — sonst ist eine Constraint-
    // Lücke undichtig und die Region frisst Außenraum (Zelt-Artefakte).
    // Knoten außerhalb der Schleifen-Boundingbox liegen auf extrapoliertem
    // Flächenteil; deren Dreiecke zählen weder zur Region noch werden sie
    // ausgegeben.
    let within_box = |p: &spade::Point2<f64>| {
        p.x >= uv_min.x && p.x <= uv_max.x && p.y >= uv_min.y && p.y <= uv_max.y
    };
    let region_area: f64 = cdt
        .inner_faces()
        .filter(|f| region.get(f.index()).copied().unwrap_or(false))
        .filter(|f| f.vertices().iter().all(|v| within_box(&v.position())))
        .map(|f| {
            let p: [spade::Point2<f64>; 3] = f.vertices().map(|v| v.position());
            ((p[1].x - p[0].x) * (p[2].y - p[0].y) - (p[1].y - p[0].y) * (p[2].x - p[0].x)).abs() * 0.5
        })
        .sum();
    let mut loop_areas: Vec<f64> = loops_uv
        .iter()
        .map(|l| {
            let mut s = 0.0;
            for k in 0..l.len() {
                let (a, b) = (l[k], l[(k + 1) % l.len()]);
                s += a.x * b.y - b.x * a.y;
            }
            (s * 0.5).abs()
        })
        .collect();
    loop_areas.sort_by(|x, y| y.partial_cmp(x).unwrap_or(std::cmp::Ordering::Equal));
    let expected = loop_areas.first().copied().unwrap_or(0.0)
        - loop_areas.iter().skip(1).sum::<f64>();
    if region_area > expected.max(0.0) * 1.25 + 1e-6 {
        if std::env::var_os("MESHER_DEBUG").is_some() {
            eprintln!(
                "[mesher-leak {tag}] Region {region_area:.4} >> erwartet {expected:.4} ({})",
                loop_areas.len()
            );
        }
        return None;
    }
    // Zu kleine Region: die Flutfällung konnte Barrieren nicht schließen
    // (Gravur-Glyphen mit kollidierenden Constraints) — Dreiecke fehlen
    // sichtbar (Zelte/Ritzen). truck übernimmt.
    if expected > 1e-6 && region_area < expected * 0.8 {
        if std::env::var_os("MESHER_DEBUG").is_some() {
            eprintln!(
                "[mesher-leak {tag}] Region {region_area:.4} << erwartet {expected:.4} ({})",
                loop_areas.len()
            );
        }
        return None;
    }

    for face in cdt.inner_faces() {
        let fi = face.index();
        if !region.get(fi).copied().unwrap_or(false) {
            continue;
        }
        // Knoten außerhalb der Schleifen-Boundingbox → Dreieck verwerfen.
        let spans = face.vertices().map(|v| v.position());
        if spans.iter().any(|p| !within_box(p)) {
            continue;
        }
        let mut tri = [0usize; 3];
        for (slot, v) in tri.iter_mut().zip(face.vertices()) {
            let h = v.fix();
            *slot = match index_of.entry(h) {
                Entry::Occupied(e) => *e.get(),
                Entry::Vacant(e) => {
                    // Innenknoten exakt auf der Fläche auswerten.
                    let (u, v) = (v.position().x / scales.0, v.position().y / scales.1);
let p3 = match map.eval(u, v) { Some(q) => q, None => dbg_none!("Inneree Val fehlgeschlagen") };
                    if !p3.x.is_finite() || !p3.y.is_finite() || !p3.z.is_finite() {
                        dbg_none!("Inneree Punkt nicht endlich");
                    }
                    let i = vertices.len();
                    vertices.push(p3);
                    e.insert(i);
                    i
                }
            };
        }
        if tri[0] == tri[1] || tri[1] == tri[2] || tri[0] == tri[2] {
            continue;
        }
        triangles.push(tri);
    }
    Some(())
}

/// Sammelt die Kanten-Paare, die von den Schleifen gerade mal (in beide
/// Richtungen) gelaufen wurden — Schlitze, die keine Barrieren sind.
fn edge_runs(
    handles: &[Vec<FixedVertexHandle>],
) -> std::collections::HashSet<(FixedVertexHandle, FixedVertexHandle)> {
    let mut runs: HashMap<(FixedVertexHandle, FixedVertexHandle), usize> = HashMap::new();
    for loop_handles in handles {
        let n = loop_handles.len();
        for k in 0..n {
            let a = loop_handles[k];
            let b = loop_handles[(k + 1) % n];
            if a != b {
                *runs
                    .entry(if a.index() < b.index() { (a, b) } else { (b, a) })
                    .or_insert(0) += 1;
            }
        }
    }
    runs.into_iter().filter(|(_, n)| n % 2 == 0).map(|(e, _)| e).collect()
}

/// Markiert je innerer CDT-Fläche, ob sie von den Randschleifen umschlossen
/// ist (ungerader Barrieren-Parität gegenüber der konvexen Hülle).
fn region_of(
    cdt: &Cdt,
    barrier: &std::collections::HashSet<(FixedVertexHandle, FixedVertexHandle)>,
) -> Vec<bool> {
    let counts = |e: spade::handles::DirectedEdgeHandle<'_, _, _, _, _>| {
        let [a, b] = e.vertices().map(|v| v.fix());
        e.is_constraint_edge()
            && !barrier.contains(&if a.index() < b.index() { (a, b) } else { (b, a) })
    };
    let mut parity: Vec<Option<bool>> = vec![None; cdt.num_all_faces()];
    let mut todo: Vec<(FixedFaceHandle<InnerTag>, bool)> = Vec::new();
    for e in cdt.convex_hull() {
        // Hüllen-Kanten haben die Außenfläche auf der einen, eine innere
        // Fläche auf der anderen Seite.
        for side in [e, e.rev()] {
            if let Some(inner) = side.face().as_inner() {
                todo.push((inner.fix(), counts(side)));
            }
        }
    }
    while let Some((h, odd)) = todo.pop() {
        match parity.get_mut(h.index()) {
            Some(slot @ None) => *slot = Some(odd),
            _ => continue,
        }
        for e in cdt.face(h).adjacent_edges() {
            if let Some(next) = e.rev().face().as_inner() {
                todo.push((next.fix(), odd ^ counts(e)));
            }
        }
    }
    parity.into_iter().map(|p| p.unwrap_or(false)).collect()
}

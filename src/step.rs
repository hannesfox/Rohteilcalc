//! STEP-Import (ISO 10303-21) und Tessellierung.
//!
//! Die B-Rep-Quelle wird als [`StepDocument`] vollständig erhalten. Das
//! Anzeigenetz wird daraus bei Bedarf — auch in verschiedenen
//! Qualitätsstufen — neu erzeugt. Es ersetzt die CAD-Repräsentation nicht.

use crate::mesh::{weld_and_clean, MeshData};
use glam::Vec3;
use std::path::Path;
use truck_meshalgo::tessellation::RobustMeshableShape;
use truck_stepio::r#in::alias::{
    re_exports::{InnerSpace, Invertible, Point3, SquareMatrix, Transform, Transformed},
    ElementarySurface, Surface,
};
use truck_stepio::r#in::Table;

/// Tessellierungsqualität. Die Toleranz ist der maximale Abstand zwischen
/// echter Fläche und Dreiecksnetz in Millimetern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Low,
    Medium,
    High,
}

impl Quality {
    pub fn tolerance(self) -> f64 {
        match self {
            Quality::Low => 0.40,
            Quality::Medium => 0.05,
            Quality::High => 0.015,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Quality::Low => "Niedrig (schnell)",
            Quality::Medium => "Mittel",
            Quality::High => "Hoch (fein)",
        }
    }

    pub const ALL: [Quality; 3] = [Quality::Low, Quality::Medium, Quality::High];
}

/// Ein einzelner Körper (STEP-Shell) des Modells.
#[derive(Clone, Debug)]
pub struct Body {
    pub name: String,
    pub mesh: MeshData,
    pub visible: bool,
}

/// Geparste STEP-Datei: die B-Rep-Quelle bleibt erhalten und kann erneut
/// tesselliert werden, ohne die Datei nochmals zu lesen.
#[derive(Clone, Debug)]
pub struct StepDocument {
    pub name: String,
    pub source: String,
    /// Maßstabsfaktor in mm (STEP-Dateien können Zoll etc. verwenden).
    pub unit_scale: f32,
}

impl StepDocument {
    pub fn from_path(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path)
            .map_err(|e| format!("Datei konnte nicht gelesen werden: {e}"))?;
        let content = decode_step_bytes(bytes);
        let content = repair_step_text(&content);
        let (content, _) = prefer_3d_curves(&content);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "STEP-Datei".to_string());
        let unit_scale = detect_unit_scale(&content);
        Ok(Self {
            name,
            source: content,
            unit_scale,
        })
    }
}

/// Trianguliert ein räumliches N-Eck (n > 4) per Ear-Clipping.
///
/// Die Projektion erfolgt über die Newell-Normale in die Flächenebene, damit
/// auch leicht nicht-planare getrimmte Flächen sinnvoll zerlegt werden.
/// Konkave Konturen (L-Form, Freistich) bleiben dabei innerhalb des Polygons;
/// ein einfacher Fächer würde Dreiecke über die gesamte Modellgröße erzeugen.
fn triangulate_ngon(points: &[Vec3], ring: &[usize]) -> Vec<[usize; 3]> {
    let n = ring.len();
    if n < 3 {
        return Vec::new();
    }
    if n == 3 {
        return vec![[ring[0], ring[1], ring[2]]];
    }

    // Newell-Normale (robust auch für nicht exakt planare Ringe).
    let mut normal = Vec3::ZERO;
    for i in 0..n {
        let a = points[ring[i]];
        let b = points[ring[(i + 1) % n]];
        normal += Vec3::new(
            (a.y - b.y) * (a.z + b.z),
            (a.z - b.z) * (a.x + b.x),
            (a.x - b.x) * (a.y + b.y),
        );
    }
    let normal = normal.normalize_or_zero();
    let u = (points[ring[1]] - points[ring[0]]).normalize_or_zero();
    if normal == Vec3::ZERO || u == Vec3::ZERO {
        return fan(ring);
    }
    let v = normal.cross(u);
    let p2: Vec<(f32, f32)> = ring
        .iter()
        .map(|&i| (points[i].dot(u), points[i].dot(v)))
        .collect();

    let mut idx: Vec<usize> = (0..n).collect();
    let mut area = 0.0f32;
    for i in 0..n {
        let a = p2[i];
        let b = p2[(i + 1) % n];
        area += a.0 * b.1 - b.0 * a.1;
    }
    if area < 0.0 {
        idx.reverse();
    }

    let mut out = Vec::with_capacity(n.saturating_sub(2));
    let mut guard = 0usize;
    while idx.len() > 3 && guard < n * n + 8 {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        'ears: for i in 0..m {
            let ia = idx[(i + m - 1) % m];
            let ib = idx[i];
            let ic = idx[(i + 1) % m];
            let (a, b, c) = (p2[ia], p2[ib], p2[ic]);
            let cross = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
            if cross <= 1e-10 {
                continue;
            }
            for &k in &idx {
                if k == ia || k == ib || k == ic {
                    continue;
                }
                let q = p2[k];
                let d1 = (b.0 - a.0) * (q.1 - a.1) - (b.1 - a.1) * (q.0 - a.0);
                let d2 = (c.0 - b.0) * (q.1 - b.1) - (c.1 - b.1) * (q.0 - b.0);
                let d3 = (a.0 - c.0) * (q.1 - c.1) - (a.1 - c.1) * (q.0 - c.0);
                if d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0 {
                    continue 'ears;
                }
            }
            out.push([ring[ia], ring[ib], ring[ic]]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            // Entartetes/überschneidendes Polygon: lieber nichts zeichnen
            // als ein Modell durchspannendes Dreieck.
            break;
        }
    }
    if idx.len() == 3 {
        out.push([ring[idx[0]], ring[idx[1]], ring[idx[2]]]);
    }
    if out.is_empty() {
        fan(ring)
    } else {
        out
    }
}

/// Notfall-Fächer (nur wenn Ear-Clipping kein einziges Dreieck findet).
fn fan(ring: &[usize]) -> Vec<[usize; 3]> {
    (1..ring.len().saturating_sub(1))
        .map(|i| [ring[0], ring[i], ring[i + 1]])
        .collect()
}

/// Passt die Mantellinie einer Kegel- oder Zylinderfläche an den tatsächlichen
/// Rand an.
///
/// truck-stepio erzeugt die erzeugende Gerade einer `CONICAL_SURFACE` bzw.
/// `CYLINDRICAL_SURFACE` nur vom Basisradius bis zu einem festen Punkt
/// (Parameterbereich `[0, 1]`). Liegt der Rand der Fläche außerhalb dieses
/// Abschnitts (z. B. bis zur Kegelspitze bei Radius 0 oder bei einer Bohrung
/// über die erzeugte Höhe hinaus), findet die Parametertriangulation keinen
/// passenden Startpunkt oder klemmt die Randpunkte auf den Rand des
/// Parameterbereichs. Das Ergebnis sind fehlende oder zu flachen Fächern
/// entartete Flächen.
///
/// Hier wird die Gerade in lokalen Koordinaten so verschoben/verlängert, dass
/// alle Randpunkte der Fläche innerhalb `[0, 1]` liegen. Die Fläche selbst
/// bleibt unverändert; nur ihre Parametrisierung wird erweitert.
fn widen_revolved_surface(
    surface: &mut Surface,
    boundary_vertices: &[Point3],
    single_loop: bool,
) {
    let Surface::ElementarySurface(elementary) = surface else {
        return;
    };
    match elementary.as_mut() {
        ElementarySurface::ConicalSurface(revolved) => {
            widen_revolved_line(revolved, boundary_vertices, single_loop, true);
        }
        ElementarySurface::CylindricalSurface(revolved) => {
            widen_revolved_line(revolved, boundary_vertices, single_loop, false);
        }
        _ => {}
    }
}

fn widen_revolved_line(
    cone: &mut truck_stepio::r#in::alias::CylindricalSurface,
    boundary_vertices: &[Point3],
    single_loop: bool,
    is_conical: bool,
) {
    if boundary_vertices.is_empty() {
        return;
    }

    let Some(inverse) = cone.transform().invert() else {
        return;
    };
    let rev = cone.entity();
    let line = *rev.entity_curve();
    let (a, b) = (line.0, line.1);
    let origin = rev.origin();
    let axis = rev.axis();
    let (dh, dr) = {
        let a_rel = a - origin;
        let b_rel = b - origin;
        let ah = a_rel.dot(axis);
        let bh = b_rel.dot(axis);
        let ar = (a_rel - ah * axis).magnitude();
        let br = (b_rel - bh * axis).magnitude();
        (bh - ah, br - ar)
    };
    let denom = dh * dh + dr * dr;
    if denom < 1e-18 {
        return;
    }

    let (mut t_min, mut t_max) = (f64::INFINITY, f64::NEG_INFINITY);
    for &p in boundary_vertices {
        let local = inverse.transform_point(p);
        let rel = local - origin;
        let h = rel.dot(axis);
        let r = (rel - h * axis).magnitude();
        let t = (h * dh + r * dr) / denom;
        if !t.is_finite() {
            return;
        }
        t_min = t_min.min(t);
        t_max = t_max.max(t);
    }

    let a_rel = a - origin;
    let a_radius = (a_rel - a_rel.dot(axis) * axis).magnitude();
    let apex = if dr.abs() > 1e-18 {
        Some(-a_radius / dr)
    } else {
        None
    };
    // Eine Kegelfläche mit nur einer Randschleife ist eine Kappe, deren
    // Inneres bis zur Spitze (Radius 0) reicht. Die Spitze ist selbst keine
    // Randkurve und liegt daher nicht in `boundary_vertices`; ohne sie würde
    // die Triangulation den Kegel auf der falschen Seite der Randschleife
    // aufspannen (im Bild ein flächiger Fächer über der Bohrung).
    if is_conical && single_loop {
        if let Some(apex) = apex {
            t_min = t_min.min(apex);
            t_max = t_max.max(apex);
        }
    }

    // Nur eingreifen, wenn der Rand den erzeugten Abschnitt verlässt.
    let epsilon = 1e-9;
    if t_min >= -epsilon && t_max <= 1.0 + epsilon {
        return;
    }

    // Etwas Rand lassen, damit Randpunkte nicht exakt auf dem Parameterrand
    // liegen. Am Scheitel (Radius 0) darf die Gerade nicht vorbeilaufen, sonst
    // entstehen negative Radien und gespiegelte Flächenstücke.
    let margin = (t_max - t_min) * 0.01 + 1e-6;
    let mut low = t_min - margin;
    let mut high = t_max + margin;
    if let Some(apex) = apex {
        if dr > 0.0 {
            low = low.max(apex);
        } else {
            high = high.min(apex);
        }
    }
    if !(low < high) {
        return;
    }

    let lerp = |t: f64| {
        Point3::new(
            a.x + (b.x - a.x) * t,
            a.y + (b.y - a.y) * t,
            a.z + (b.z - a.z) * t,
        )
    };
    let widened = truck_stepio::r#in::alias::re_exports::RevolutedCurve::by_revolution(
        truck_stepio::r#in::alias::re_exports::Line(lerp(low), lerp(high)),
        origin,
        axis,
    );
    let mut replacement =
        truck_stepio::r#in::alias::re_exports::Processor::new(widened).transformed(*cone.transform());
    if !cone.orientation() {
        replacement.invert();
    }
    *cone = replacement;
}

/// Ergebnis der Tessellierung inklusive nicht-fatale Warnungen.
pub struct TessellatedPart {
    pub bodies: Vec<Body>,
    pub warnings: Vec<String>,
}

/// Tesselliert alle Shells des Dokuments. Fehler einzelner Shells führen
/// nicht zum Gesamtausfall, werden aber als Warnung zurückgegeben.
pub fn tessellate(doc: &StepDocument, quality: Quality) -> Result<TessellatedPart, String> {
    let table = Table::from_step(&doc.source)
        .ok_or_else(|| "STEP-Datei konnte nicht geparst werden (ungültiges Format?)".to_string())?;

    if table.shell.is_empty() {
        return Err("Keine B-rep-Shell in der STEP-Datei gefunden.".to_string());
    }

    // Sicherheitsnetz: Ein SURFACE_CURVE mit Pkurve als Master-Repräsentation
    // kann bei truck-stepio eine 2D-Kurve mit unbrauchbarem Parameterbereich
    // liefern. Deren Tessellierung läuft in eine NaN-Rekursion und kann den
    // Prozess durch Speichererschöpfung beenden. `from_path` ersetzt solche
    // Marker normalerweise; bleibt einer übrig (z. B. ungewöhnliche
    // Schreibweise), lieber mit einer Meldung abbrechen als OOM zu riskieren.
    if contains_pcurve_master(&doc.source) {
        return Err(
            "STEP enthält weiterhin eine SURFACE_CURVE-Pkurve als Master-Repräsentation. \
             Die Tessellierung wurde aus Sicherheitsgründen abgebrochen, um einen \
             Speicherüberlauf zu vermeiden."
                .to_string(),
        );
    }

    let mut bodies = Vec::new();
    let mut warnings = Vec::new();
    let mut shell_index = 0usize;

    for shell in table.shell.values() {
        shell_index += 1;
        let mut cshell = match table.to_compressed_shell(shell) {
            Ok(c) => c,
            Err(e) => {
                warnings.push(format!("Shell {shell_index} konnte nicht gelesen werden: {e:?}"));
                continue;
            }
        };

        // Referenz-Bounding-Box der Shell: Dreiecke, die weit außerhalb
        // liegen, stammen aus fehlerhaften STEP-Flächen (Spikes).
        let mut shell_min = Vec3::splat(f32::MAX);
        let mut shell_max = Vec3::splat(f32::MIN);
        for p in &cshell.vertices {
            let v = Vec3::new(p.x as f32, p.y as f32, p.z as f32);
            shell_min = shell_min.min(v);
            shell_max = shell_max.max(v);
        }
        // Bewusst großzügig: ein zu enger Filter reißt Löcher in legitime
        // Flächen, das Bauteil wirkt dann von manchen Seiten durchsichtig.
        let tolerance = ((shell_max - shell_min).length() * 0.35).max(5.0);
        let inside = |p: Vec3| {
            p.x >= shell_min.x - tolerance
                && p.x <= shell_max.x + tolerance
                && p.y >= shell_min.y - tolerance
                && p.y <= shell_max.y + tolerance
                && p.z >= shell_min.z - tolerance
                && p.z <= shell_max.z + tolerance
        };

        // Kegel- und Zylinderflächen vor der Triangulation an ihren
        // tatsächlichen Rand anpassen (siehe `widen_revolved_surface`).
        {
            let shell_vertices = &cshell.vertices;
            let shell_edges = &cshell.edges;
            for face in &mut cshell.faces {
                let mut boundary_vertices = Vec::new();
                for boundary in &face.boundaries {
                    for edge_idx in boundary {
                        let edge = &shell_edges[edge_idx.index];
                        boundary_vertices.push(shell_vertices[edge.vertices.0]);
                        boundary_vertices.push(shell_vertices[edge.vertices.1]);
                    }
                }
                let single_loop = face.boundaries.len() == 1;
                widen_revolved_surface(&mut face.surface, &boundary_vertices, single_loop);
            }
        }

        let tessellated = cshell.robust_triangulation(quality.tolerance());

        // Analytischer Vorauslauf (Rezept nach solidcraft): Ebenen,
        // Zylinder und Kegel werden pro Fläche exakt im (u, v)-Parameter-
        // bereich geteilt und per Constrained-Delaunay gefüllt. Das ist bei
        // getrimmten Kegel-/Zylinderflächen robuster als trucks
        // Nearest-Parameter-Suche. Nicht-analytische Flächen (B-Spline,
        // Torus, …) oder Fehler einer Fläche fallen auf truck zurück.
        let analytic: Vec<Option<crate::mesher::FaceMesh>> = if std::env::var_os("ROHTEIL_NO_ANALYTIC").is_some() {
            Vec::new()
        } else {
            cshell
                .faces
                .iter()
                .enumerate()
                .map(|(face_i, face)| {
                    crate::mesher::tessellate_face(
                        face,
                        &cshell.vertices,
                        &cshell.edges,
                        quality.tolerance(),
                        &format!("face{}", face_i),
                    )
                })
                .collect()
        };

        let mut vertices: Vec<Vec3> = Vec::new();
        let mut triangles: Vec<[usize; 3]> = Vec::new();
        let mut skipped_faces = 0usize;

        for (face_i, face) in tessellated.faces.iter().enumerate() {
            // Debug-Sichter: ROHTEIL_FACE_MASK="12,40-60" rendert nur die
            // genannten Flächen.
            let mask_ok = match std::env::var("ROHTEIL_FACE_MASK") {
                Ok(spec) => spec.split(',').any(|part| {
                    let mut it = part.splitn(2, '-');
                    let lo: usize = it.next().and_then(|s| s.trim().parse().ok()).unwrap_or(usize::MAX);
                    let hi: usize = it.next().and_then(|s| s.trim().parse().ok()).unwrap_or(lo);
                    face_i >= lo && face_i <= hi
                }),
                Err(_) => true,
            };
            if !mask_ok {
                continue;
            }
            // Analytisch getessellierte Fläche vorziehen; truck liefert hier
            // sonst die Nearest-Parameter-Artefakte (Kegel-Fächer).
            if let Some(amesh) = analytic.get(face_i).and_then(|m| m.as_ref()) {
                let base = vertices.len();
                vertices.extend(amesh.vertices.iter().map(|p| {
                    Vec3::new(p.x as f32, p.y as f32, p.z as f32)
                }));
                let world = |idx: usize| vertices[base + idx];
                for tri in &amesh.triangles {
                    let (pa, pb, pc) = (world(tri[0]), world(tri[1]), world(tri[2]));
                    if inside(pa) && inside(pb) && inside(pc) {
                        triangles.push([base + tri[0], base + tri[1], base + tri[2]]);
                    }
                }
                continue;
            }

            if let Some(poly) = &face.surface {
                let base = vertices.len();
                let positions = poly.positions();
                let world_positions: Vec<Vec3> = positions
                    .iter()
                    .map(|p| Vec3::new(p.x as f32, p.y as f32, p.z as f32))
                    .collect();
                vertices.extend(world_positions.iter().copied());

                let world = |idx: usize| world_positions[idx];
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
                    // N-Ecke (n > 4) sind bei getrimmten CAD-Flächen oft konkav
                    // (L-förmige Deckfläche, Freistich). Ein Fächer von einem
                    // Eckpunkt aus erzeugt dabei Dreiecke, die weit über die
                    // Fläche hinausragen — im Drahtgitter als lange Diagonalen
                    // sichtbar. Darum echtes Ear-Clipping in der Flächenebene.
                    let ring: Vec<usize> = other.iter().map(|v| v.pos).collect();
                    let local = triangulate_ngon(&world_positions, &ring);
                    for tri in local {
                        push_tri(tri[0], tri[1], tri[2]);
                    }
                }
                continue;
            }

            // Fallback für einzelne Flächen, deren Parameterbereich nicht zu
            // ihren Randkurven passt. Typischer Fall: eine Kegelfläche, deren
            // Rand bis zur Spitze reicht, obwohl truck-stepio die erzeugende
            // Mantellinie nur in eine Richtung parametrisiert. Dann findet die
            // Parametertriangulation keinen Startpunkt. Bei nur einer
            // Randschleife lässt sich die Fläche direkt aus den bereits
            // tessellierten Randpolylinien aufbauen; mehrere Schleifen
            // (Flächen mit Löchern) werden weiterhin übersprungen.
            let Some(boundary) = face
                .boundaries
                .first()
                .filter(|_| face.boundaries.len() == 1)
            else {
                skipped_faces += 1;
                continue;
            };

            let mut ring_points: Vec<Vec3> = Vec::new();
            for edge_idx in boundary {
                let edge = &tessellated.edges[edge_idx.index];
                let mut points = edge.curve.0.clone();
                if !edge_idx.orientation {
                    points.reverse();
                }
                for p in points {
                    let v = Vec3::new(p.x as f32, p.y as f32, p.z as f32);
                    let duplicate = ring_points
                        .last()
                        .is_some_and(|last| (*last - v).length_squared() < 1e-18);
                    if !duplicate {
                        ring_points.push(v);
                    }
                }
            }
            if ring_points.len() > 1 {
                let first = ring_points[0];
                let last = *ring_points.last().unwrap();
                if (first - last).length_squared() < 1e-18 {
                    ring_points.pop();
                }
            }
            if ring_points.len() < 3 {
                skipped_faces += 1;
                continue;
            }

            let base = vertices.len();
            vertices.extend(ring_points.iter().copied());
            let ring: Vec<usize> = (0..ring_points.len()).collect();
            let local = triangulate_ngon(&ring_points, &ring);
            if local.is_empty() {
                vertices.truncate(base);
                skipped_faces += 1;
                continue;
            }

            let world = |idx: usize| ring_points[idx];
            let mut push_tri = |a: usize, b: usize, c: usize| {
                let (pa, pb, pc) = (world(a), world(b), world(c));
                if inside(pa) && inside(pb) && inside(pc) {
                    triangles.push([base + a, base + b, base + c]);
                }
            };
            for tri in local {
                push_tri(tri[0], tri[1], tri[2]);
            }
        }

        if skipped_faces > 0 {
            warnings.push(format!(
                "{skipped_faces} Fläche(n) in Shell {shell_index} konnten nicht trianguliert werden."
            ));
        }

        if triangles.is_empty() {
            warnings.push(format!(
                "Shell {shell_index} enthielt keine triangulierbaren Flächen."
            ));
            continue;
        }

        if doc.unit_scale != 1.0 {
            for v in &mut vertices {
                *v *= doc.unit_scale;
            }
        }

        let mesh = weld_and_clean(vertices, triangles);
        if mesh.triangles.is_empty() {
            continue;
        }
        bodies.push(Body {
            name: format!("Körper {shell_index}"),
            mesh,
            visible: true,
        });
    }

    if bodies.is_empty() {
        let hint = warnings
            .last()
            .map(|w| format!(" Letzter Fehler: {w}"))
            .unwrap_or_default();
        return Err(format!(
            "STEP enthielt keine anzeigbare Geometrie.{hint}"
        ));
    }

    Ok(TessellatedPart { bodies, warnings })
}

/// Liest eine STEP-Datei als Bytes und dekodiert sie tolerant:
///
/// STEP ist im Kern ASCII, aber viele Exporteure (CATIA, NX, HOOPS, ältere
/// CAM-Systeme) schreiben Namen und Kommentare in ISO-8859-1. Solche
/// Dateien sind kein valides UTF-8 und scheitern sonst mit
/// „stream did not contain valid UTF-8“. Da außerhalb von String-Literalen
/// nur ASCII-Geometrie steht, ist der Latin-1-Fallback verlustfrei.
fn decode_step_bytes(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => e.into_bytes().iter().map(|&b| b as char).collect(),
    }
}

/// Ersetzt leere Parameterlisten `()` durch `($)`.
///
/// ruststep 0.4 (Parser-Basis von truck-stepio 0.3) kann leere Listen wie
/// `DRAUGHTING_MODEL('',(),#10)` nicht lexen und bricht die ganze Datei ab.
/// `$` ist das Standardelement für „nicht angegeben“ und für die
/// Geometrieextraktion neutral.
///
/// Die Ersetzung ist string-bewusst (`''` in Strings zählt nicht als Ende)
/// und läuft iterativ, damit verschachtelte leere Listen von innen nach
/// außen aufgelöst werden.
fn repair_step_text(input: &str) -> String {
    let mut current = input.to_string();
    loop {
        let mut out = String::with_capacity(current.len() + 16);
        let mut chars = current.chars().peekable();
        let mut changed = false;
        let mut in_str = false;
        while let Some(c) = chars.next() {
            if in_str {
                out.push(c);
                if c == '\'' {
                    if chars.peek() == Some(&'\'') {
                        out.push('\'');
                        chars.next();
                    } else {
                        in_str = false;
                    }
                }
                continue;
            }
            match c {
                '\'' => {
                    in_str = true;
                    out.push(c);
                }
                '(' if chars.peek() == Some(&')') => {
                    chars.next();
                    out.push_str("($)");
                    changed = true;
                }
                other => out.push(other),
            }
        }
        if !changed {
            return out;
        }
        current = out;
        if current.len() > input.len() * 4 + 16 {
            return current;
        }
    }
}

/// Bevorzugt bei SURFACE_CURVE-Entitäten die 3D-Kurve statt einer Pkurve.
///
/// Viele CAD-Exportere (OpenCASCADE/CadQuery, SolidWorks, …) setzen die
/// Master-Repräsentation von Kanten auf `.PCURVE_S1.`/`.PCURVE_S2.`. Für
/// kreisförmige Kanten (z. B. Rundungen an einer Box) liefert truck-stepio
/// dann eine 2D-Pkurve mit falschem Parameterbereich und einer überall
/// NaN-Kurve. Deren Parameterteilung rekursiert in truck-geotrait bis zur
/// maximalen Tiefe und alloziert bis zum Speicherüberlauf. Die 3D-Kurve ist
/// in denselben Entitäten bereits vorhanden und geometrisch äquivalent;
/// darum werden nur die Master-Marker ersetzt. String-Literale bleiben
/// unangetastet, damit z. B. Dateinamen nicht verändert werden.
fn prefer_3d_curves(input: &str) -> (String, usize) {
    let mut spans = Vec::new();
    for_each_pcurve_master(input, |start, len| spans.push((start, len)));
    if spans.is_empty() {
        return (input.to_string(), 0);
    }

    let mut out = String::with_capacity(input.len());
    let mut copied = 0usize;
    for (start, len) in &spans {
        out.push_str(&input[copied..*start]);
        out.push_str(".CURVE_3D.");
        copied = start + len;
    }
    out.push_str(&input[copied..]);
    (out, spans.len())
}

/// Prüft, ob außerhalb von String-Literalen noch ein Pkurven-Master-Marker
/// vorhanden ist (Sicherheitsnetz in [`tessellate`]).
fn contains_pcurve_master(input: &str) -> bool {
    let mut found = false;
    for_each_pcurve_master(input, |_, _| found = true);
    found
}

/// Ruft `f(start, len)` für jeden Pkurven-Master-Marker außerhalb von
/// STEP-String-Literalen auf. Die Erkennung ist bewusst unabhängig von der
/// Groß-/Kleinschreibung, da STEP-Schlüsselwörter case-insensitiv sind.
fn for_each_pcurve_master(input: &str, mut f: impl FnMut(usize, usize)) {
    const TOKENS: [&str; 2] = [".PCURVE_S1.", ".PCURVE_S2."];
    let mut chars = input.char_indices().peekable();
    let mut in_str = false;
    while let Some((i, c)) = chars.next() {
        if in_str {
            if c == '\'' {
                if input[i + 1..].starts_with('\'') {
                    chars.next();
                } else {
                    in_str = false;
                }
            }
            continue;
        }
        match c {
            '\'' => in_str = true,
            '.' => {
                let rest = &input[i..];
                let token_len = TOKENS.iter().find_map(|token| {
                    rest.get(..token.len())
                        .filter(|s| s.eq_ignore_ascii_case(token))
                        .map(|_| token.len())
                });
                if let Some(len) = token_len {
                    f(i, len);
                    // ASCII-Token vollständig überspringen.
                    for _ in 1..len {
                        chars.next();
                    }
                }
            }
            _ => {}
        }
    }
}

/// Ermittelt den Maßstab der DATEI-Einheit in Millimeter.
///
/// truck-stepio liefert Geometrie in Dateieinheiten. Für die CNC-Ausgabe
/// rechnen wir auf Millimeter um. Nicht unterstützte Einheiten werden als
/// Millimeter interpretiert (Standardfall der getesteten Exporte).
fn detect_unit_scale(source: &str) -> f32 {
    // Whitespace entfernen, damit sowohl `SI_UNIT(.MILLI.,.METRE.)` als auch
    // `SI_UNIT ( .MILLI., .METRE. )` erkannt werden.
    let upper: String = source
        .to_ascii_uppercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();

    if upper.contains("CONVERSION_BASED_UNIT('INCH'") {
        return 25.4;
    }
    if upper.contains("CONVERSION_BASED_UNIT('FOOT'") {
        return 304.8;
    }
    if upper.contains("SI_UNIT(.MILLI.,.METRE.)") {
        return 1.0;
    }
    if upper.contains("SI_UNIT(.CENTI.,.METRE.)") {
        return 10.0;
    }
    if upper.contains("SI_UNIT($,.METRE.)") {
        return 1000.0;
    }
    1.0
}

/// Headless-Hilfsfunktion: STEP laden, tessellieren und ausrichten.
pub fn load_aligned(path: &Path, quality: Quality) -> Result<(Vec<Body>, crate::stock::StockResult), String> {
    let doc = StepDocument::from_path(path)?;
    let part = tessellate(&doc, quality)?;
    let mut bodies = part.bodies;
    let mut meshes: Vec<MeshData> = bodies.drain(..).map(|b| b.mesh).collect();
    let stock = crate::stock::orient_and_align_bodies(&mut meshes);
    let bodies = meshes
        .into_iter()
        .enumerate()
        .map(|(i, mesh)| Body {
            name: format!("Körper {}", i + 1),
            mesh,
            visible: true,
        })
        .collect();
    Ok((bodies, stock))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const ROUNDED_BOX: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/rounded_box.step.txt"
    );
    const CONICAL_APEX_FACE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/conical_apex_face.step.txt"
    );
    const BLIND_CONE_HOLE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/blind_cone_hole.step.txt"
    );

    #[test]
    fn replaces_pcurve_master_representation() {
        let input = "SURFACE_CURVE('',#1,(#2),.PCURVE_S1.);\n\
                     SURFACE_CURVE('',#3,(#4),.PCURVE_S2.);\n";
        let (output, count) = prefer_3d_curves(input);
        assert_eq!(count, 2);
        assert_eq!(
            output,
            "SURFACE_CURVE('',#1,(#2),.CURVE_3D.);\n\
             SURFACE_CURVE('',#3,(#4),.CURVE_3D.);\n"
        );
        assert_eq!(prefer_3d_curves(&output), (output.clone(), 0));
    }

    #[test]
    fn pcurve_replacement_keeps_normal_text_and_string_literals() {
        let input = "FILE_NAME('.PCURVE_S1. .PCURVE_S2.',#1);\n\
                     /* normaler Kommentar */\n\
                     EDGE_CURVE('',#2,#3,#4,.T.);\n";
        let (output, count) = prefer_3d_curves(input);
        assert_eq!(count, 0);
        assert_eq!(output, input);
        assert!(!contains_pcurve_master(input));
    }

    #[test]
    fn pcurve_master_detection_is_case_insensitive() {
        assert!(contains_pcurve_master("X=.pcurve_s1.;"));
        assert!(contains_pcurve_master("X=.PCURVE_S2.;"));
        assert!(!contains_pcurve_master("X='.PCURVE_S1.';"));
        let (output, count) = prefer_3d_curves("X=.pcurve_s2.;");
        assert_eq!(count, 1);
        assert_eq!(output, "X=.CURVE_3D.;");
    }

    #[test]
    fn conical_apex_face_is_reconstructed_from_boundary() {
        let doc = StepDocument::from_path(Path::new(CONICAL_APEX_FACE)).expect("STEP laden");
        let part = tessellate(&doc, Quality::Medium).expect("tessellieren");
        let triangle_count: usize = part
            .bodies
            .iter()
            .map(|body| body.mesh.triangles.len())
            .sum();
        assert!(triangle_count > 0, "Kegelfläche lieferte keine Dreiecke");
        assert!(
            !part
                .warnings
                .iter()
                .any(|w| w.contains("konnten nicht trianguliert werden")),
            "Kegelfläche mit Spitze wurde übersprungen: {:?}",
            part.warnings
        );
    }

    #[test]
    fn blind_cone_hole_reaches_apex() {
        let doc = StepDocument::from_path(Path::new(BLIND_CONE_HOLE)).expect("STEP laden");
        let part = tessellate(&doc, Quality::Medium).expect("tessellieren");
        let apex = Vec3::new(0.0, 0.0, 4.0);
        let reached_apex = part.bodies.iter().any(|body| {
            body.mesh
                .vertices
                .iter()
                .any(|v| (*v - apex).length() < 0.2)
        });
        assert!(
            reached_apex,
            "Kegelboden endet nicht an der Spitze — Mantellinie reicht nicht bis zum Scheitel"
        );
    }

    /// Führt die eigentliche Regression in einem separaten Prozess aus. So
    /// kann ein alter, hängender oder den Speicher sprengender Tessellierer
    /// den Testprozess nicht mitreißen.
    #[test]
    fn rounded_box_regression_does_not_hang() {
        let exe = std::env::current_exe().expect("Test-Binary");
        let mut child = Command::new(exe)
            .args([
                "--exact",
                "step::tests::rounded_box_worker",
                "--ignored",
                "--nocapture",
            ])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("Regression-Worker starten");

        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(status) = child.try_wait().expect("Worker-Status") {
                assert!(
                    status.success(),
                    "Tessellierungs-Worker ist fehlgeschlagen: {status}"
                );
                return;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Tessellierung des gerundeten Kastens hat 60 s überschritten");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    #[ignore = "wird von rounded_box_regression_does_not_hang als Unterprozess gestartet"]
    fn rounded_box_worker() {
        let raw = std::fs::read_to_string(ROUNDED_BOX).expect("STEP-Fixture");
        assert!(
            raw.contains(".PCURVE_S1.") || raw.contains(".PCURVE_S2."),
            "Fixture muss SURFACE_CURVE-Pkurven enthalten"
        );

        let doc = StepDocument::from_path(Path::new(ROUNDED_BOX)).expect("STEP laden");
        assert!(
            !contains_pcurve_master(&doc.source),
            "Pkurven-Master-Marker wurden nicht ersetzt"
        );

        let part = tessellate(&doc, Quality::Medium).expect("tessellieren");
        let mut triangle_count = 0usize;
        for body in &part.bodies {
            triangle_count += body.mesh.triangles.len();

            let mut edge_counts: HashMap<(usize, usize), usize> = HashMap::new();
            for tri in &body.mesh.triangles {
                for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                    let key = if a < b { (a, b) } else { (b, a) };
                    *edge_counts.entry(key).or_default() += 1;
                }
            }
            let boundary_edges = edge_counts.values().filter(|&&n| n != 2).count();
            assert_eq!(
                boundary_edges, 0,
                "Netz hat {boundary_edges} offene/nicht paarige Kanten; Warnungen: {:?}",
                part.warnings
            );
        }
        assert!(
            triangle_count > 0,
            "Tessellierung lieferte keine Dreiecke; Warnungen: {:?}",
            part.warnings
        );
    }
}

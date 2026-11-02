//! STEP-Import (ISO 10303-21) und Tessellierung.
//!
//! Die B-Rep-Quelle wird als [`StepDocument`] vollständig erhalten. Das
//! Anzeigenetz wird daraus bei Bedarf — auch in verschiedenen
//! Qualitätsstufen — neu erzeugt. Es ersetzt die CAD-Repräsentation nicht.

use crate::mesh::{weld_and_clean, MeshData};
use glam::Vec3;
use std::path::Path;
use truck_meshalgo::tessellation::RobustMeshableShape;
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

    let mut bodies = Vec::new();
    let mut warnings = Vec::new();
    let mut shell_index = 0usize;

    for shell in table.shell.values() {
        shell_index += 1;
        let cshell = match table.to_compressed_shell(shell) {
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

        let tessellated = cshell.robust_triangulation(quality.tolerance());

        let mut vertices: Vec<Vec3> = Vec::new();
        let mut triangles: Vec<[usize; 3]> = Vec::new();
        let mut skipped_faces = 0usize;

        for face in &tessellated.faces {
            let Some(poly) = face.surface.clone() else {
                // truck kann einzelne getrimmte Flächen nicht triangulieren.
                // Ein Ersatz aus dem Randpolygon erzeugt bei nicht-planaren
                // Rändern riesige, das Modell durchspannende Dreiecke (im
                // Drahtgitter sichtbar) und ist für die Anzeige schädlicher
                // als die fehlende Fläche. Darum bewusst überspringen.
                skipped_faces += 1;
                continue;
            };

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

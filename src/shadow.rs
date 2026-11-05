//! Schattenkontur: die Außenkontur des Bauteils, auf die XY-Ebene projiziert
//! („Schatten von oben"), parallel nach außen um einen einstellbaren Abstand
//! vergrößert — exportierbar als DXF (mm).
//!
//! Verfahren (rasterbasiert, robust): alle Netzdreiecke werden in die
//! XY-Ebene gerastert, daraus entsteht ein Abstandsfeld (Chamfer-Transformation)
//! zum besetzten Gebiet; die Kontur bei Abstand = Vorgabewert wird per
//! Marching Squares extrahiert, vereinfacht (Ramer-Douglas-Peucker) und nur
//! die größte geschlossene Kontur — die Außenkontur — behalten. Überlappende
//! Projektionen und streifige Netzkanten erzeugen so weder
//! Selbstüberschneidungen noch Bruchstücke.
//!
//! Genauigkeit: die Kontur folgt dem Raster (`res`, typisch ≈ 0,05 mm) plus
//! Simplifikationstoleranz — für die Abstände und Fertigungsfragen dieser
//! Anwendung ausreichend, dafür bei jeder Geometrie formstabil.

use std::collections::HashMap;

use crate::renderer::ScModel;

/// Fertige Schattenkontur (alles in mm).
#[derive(Clone)]
pub struct ShadowResult {
    /// Abstand, um den die Kontur nach außen vergrößert ist.
    pub expand: f32,
    /// Die Kontur als geschlossene Polygonliste (gegen den Uhrzeigersinn),
    /// Koordinatenursprung an der unteren linken Ecke des Bauteils (0,0).
    pub polygon: Vec<[f32; 2]>,
    /// Umfang (mm) und Fläche (mm²) der Kontur.
    pub perimeter: f64,
    pub area: f64,
    /// Rasterauflösung in mm — die Genauigkeit dieser Kontur.
    pub res: f32,
    /// Welt-Ecke der Bauteilprojektion (min x,y) und Ausdehnung — für die
    /// 3D-Vorschau und den Verfalls-Check bei neuem Modell.
    pub origin: [f32; 2],
    pub extent: [f32; 2],
}

/// Schattenkontur aus dem aktuellen Anzeigemodell erzeugen.
pub fn build(model: &ScModel, expand: f32) -> Result<ShadowResult, String> {
    if model.part_triangles.is_empty() {
        return Err("keine Bauteilgeometrie zum Projizieren".to_string());
    }
    from_mesh(&model.part_positions, &model.part_triangles, expand)
}

/// Schattenkontur aus rohen Dreiecken (mm, XY-Projektion) erzeugen.
pub fn from_mesh(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    expand: f32,
) -> Result<ShadowResult, String> {
    let expand = expand.max(0.0);

    // Ausdehnung der Projektion messen.
    let mut bmin = [f32::MAX; 2];
    let mut bmax = [f32::MIN; 2];
    for &[x, y, _] in positions {
        bmin[0] = bmin[0].min(x);
        bmin[1] = bmin[1].min(y);
        bmax[0] = bmax[0].max(x);
        bmax[1] = bmax[1].max(y);
    }
    let ext = [bmax[0] - bmin[0], bmax[1] - bmin[1]];
    // Rasterauflösung: maximal ~1600 Zellen in der größeren Richtung,
    // zwischen 0,02 und 0,25 mm (fein genug für Millimeter-Abstände).
    let res = (ext[0].max(ext[1]) / 1600.0).clamp(0.02, 0.25);
    let iso_cells = (expand / res).ceil() as i64 + 1;
    let pad = iso_cells + 3;

    let w: i64 = (ext[0] / res).ceil() as i64 + 2 * pad;
    let h: i64 = (ext[1] / res).ceil() as i64 + 2 * pad;

    // Besetzte Zellen markieren (0 = Bauteil, sonst frei).
    const INF: u16 = u16::MAX;
    let mut field = vec![INF; (w * h) as usize];
    for t in triangles {
        fill_triangle(&mut field, w, h, res, bmin, pad, [
            &positions[t[0] as usize],
            &positions[t[1] as usize],
            &positions[t[2] as usize],
        ]);
    }

    // Chamfer-Abstandsfeld (zwei Durchläufe, 3/4-Kosten) in Zellen.
    let mut dist = {
        let mut d = vec![INF; field.len()];
        for (k, f) in field.iter().enumerate() {
            if *f == 0 {
                d[k] = 0;
            }
        }
        d
    };
    let get = |d: &Vec<u16>, x: i64, y: i64| -> u16 {
        if x >= 0 && y >= 0 && x < w && y < h {
            d[(y * w + x) as usize]
        } else {
            INF
        }
    };
    for y in 0..h {
        for x in 0..w {
            let k = (y * w + x) as usize;
            if dist[k] == 0 {
                continue;
            }
            let d = dist[k]
                .min(get(&dist, x - 1, y).saturating_add(3))
                .min(get(&dist, x, y - 1).saturating_add(3))
                .min(get(&dist, x - 1, y - 1).saturating_add(4))
                .min(get(&dist, x + 1, y - 1).saturating_add(4));
            dist[k] = d;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let k = (y * w + x) as usize;
            let d = dist[k]
                .min(get(&dist, x + 1, y).saturating_add(3))
                .min(get(&dist, x, y + 1).saturating_add(3))
                .min(get(&dist, x + 1, y + 1).saturating_add(4))
                .min(get(&dist, x - 1, y + 1).saturating_add(4));
            dist[k] = d;
        }
    }
    // Jenseits der Kontur wird der Abstand geklemmt, so dass das Feld am
    // Rasterrand garantiert über der Kontur liegt (Baschtigkeit bleibt).
    // Achtung Einheiten: das Chamfer-Feld rechnet mit 3/4-Kosten, d. h.
    // der Wert 3 entspricht EINER Zelle echtem Abstand (4 = Diagonale).
    // Alles Vergleichen mit Zellen-/mm-Werten muss deshalb durch 3 teilen.
    let clamp_units = (((iso_cells + 2) * 3).min(i64::from(u16::MAX - 1))) as u16;
    for d in dist.iter_mut() {
        *d = (*d).min(clamp_units);
    }

    let node = |i: i64, j: i64| -> [f32; 2] {
        [bmin[0] + (i as f32 - pad as f32) * res, bmin[1] + (j as f32 - pad as f32) * res]
    };
    let corners = |i: i64, j: i64| -> f32 {
        let vals = [
            get(&dist, i - 1, j - 1),
            get(&dist, i, j - 1),
            get(&dist, i - 1, j),
            get(&dist, i, j),
        ]
        .map(|v| v.min(clamp_units));
        // Chamfer-Einheiten → Zellen (/3) → mm (* res).
        (u32::from(vals[0])
            + u32::from(vals[1])
            + u32::from(vals[2])
            + u32::from(vals[3])) as f32
            / 4.0
            / 3.0
            * res
    };

    // Marching Squares bei Abstand = Vorgabe: Kreuzungen der Zellkanten
    // werden linear interpoliert und zu Segmenten gepaart.
    let iso = expand;
    let mut segments: Vec<([f32; 2], [f32; 2])> = Vec::new();
    for j in 0..h {
        for i in 0..w - 1 {
            let (p00, p10, p11, p01) = (node(i, j), node(i + 1, j), node(i + 1, j + 1), node(i, j + 1));
            let (v00, v10, v11, v01) = (corners(i, j), corners(i + 1, j), corners(i + 1, j + 1), corners(i, j + 1));
            let cross = |a: [f32; 2], va: f32, b: [f32; 2], vb: f32| -> Option<[f32; 2]> {
                if (va <= iso) == (vb <= iso) {
                    return None;
                }
                let t = (iso - va) / (vb - va);
                Some([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t])
            };
            let bottom = cross(p00, v00, p10, v10);
            let right = cross(p10, v10, p11, v11);
            let top = cross(p01, v01, p11, v11);
            let left = cross(p00, v00, p01, v01);
            let hits = [bottom, right, top, left];
            let count = hits.iter().filter(|h| h.is_some()).count();
            match count {
                0 => {}
                2 => {
                    let pts: Vec<[f32; 2]> = hits.iter().filter_map(|h| *h).collect();
                    segments.push((pts[0], pts[1]));
                }
                _ => {
                    // Doppelschnitt (Diagonale): Segmente an den Ecken, die
                    // im vergrößerten Gebiet liegen.
                    if v00 <= iso {
                        if let (Some(l), Some(b)) = (left, bottom) {
                            segments.push((l, b));
                        }
                    }
                    if v10 <= iso {
                        if let (Some(b), Some(r)) = (bottom, right) {
                            segments.push((b, r));
                        }
                    }
                    if v11 <= iso {
                        if let (Some(r), Some(t)) = (right, top) {
                            segments.push((r, t));
                        }
                    }
                    if v01 <= iso {
                        if let (Some(t), Some(l)) = (top, left) {
                            segments.push((t, l));
                        }
                    }
                }
            }
        }
    }
    if segments.is_empty() {
        return Err("Kontur nicht extrahierbar (Abstand oder Geometrie unpassend)".to_string());
    }

    // Segmente zu geschlossenen Konturen fügen: Endpunkte symmetrisch
    // verknüpfen — die extractierte Richtung erster Paare wechselt mit dem
    // Marching-Squares-Fall, so dass ungerichtet gelaufen wird.
    let key = |p: [f32; 2]| -> (i64, i64) {
        ((p[0] * 200.0).round() as i64, (p[1] * 200.0).round() as i64)
    };
    type NearEntry = (usize, [f32; 2]);
    let mut near: HashMap<(i64, i64), Vec<NearEntry>> = HashMap::new();
    for (idx, (a, b)) in segments.iter().enumerate() {
        near.entry(key(*a)).or_default().push((idx, *b));
        near.entry(key(*b)).or_default().push((idx, *a));
    }
    let mut used = vec![false; segments.len()];
    let mut loops: Vec<Vec<[f32; 2]>> = Vec::new();
    for start in 0..segments.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let (first, second) = segments[start];
        let mut chain = vec![first, second];
        let origin = key(first);
        loop {
            let tail = key(*chain.last().unwrap());
            let Some(list) = near.get(&tail) else { break };
            let Some(&(id, point)) = list.iter().find(|(s, _)| !used[*s]) else { break };
            if key(point) == origin {
                break; // geschlossen — Startpunkt nicht doppelt nehmen
            }
            used[id] = true;
            chain.push(point);
        }
        if chain.len() >= 3 {
            loops.push(chain);
        }
    }
    if loops.is_empty() {
        return Err("Kontur nicht extrahierbar: keine geschlossene Linie".to_string());
    }

    // Vereinfachen, größte Kontur (die Außenkontur) nehmen, gegen den
    // Uhrzeigersinn orientieren.
    let tol = res * 0.8;
    let mut best: Option<(f64, Vec<[f32; 2]>)> = None;
    for l in &loops {
        let simplified = simplify(l, tol);
        let a = area(&simplified).abs();
        if best.as_ref().is_none_or(|(ba, _)| a > *ba) {
            best = Some((a, simplified));
        }
    }
    let mut polygon = best.ok_or("leere Kontur")?.1;
    if area(&polygon) < 0.0 {
        polygon.reverse();
    }

    // Ursprung aufs Bauteil legen (untere linke Ecke = 0,0).
    for p in polygon.iter_mut() {
        p[0] -= bmin[0];
        p[1] -= bmin[1];
    }

    let perimeter: f64 = polygon
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let b = poly_next(&polygon, i);
            let dx = f64::from(b[0] - a[0]);
            let dy = f64::from(b[1] - a[1]);
            (dx * dx + dy * dy).sqrt()
        })
        .sum();
    Ok(ShadowResult {
        expand,
        area: area(&polygon).abs(),
        polygon,
        perimeter,
        res,
        origin: bmin,
        extent: ext,
    })
}

impl ShadowResult {
    /// Passt die gespeicherte Kontur noch zu dieser Geometrie? (Die Bounding-
    /// box der Projekion muss unverändert sein, sonst läuft das Modell von der
    /// Vorschau gefolgt — z. B. nach dem Laden einer neuen STEP-Datei.)
    pub fn same_footprint(&self, positions: &[[f32; 3]]) -> bool {
        if positions.is_empty() {
            return false;
        }
        let mut bmin = [f32::MAX; 2];
        let mut bmax = [f32::MIN; 2];
        for &[x, y, _] in positions {
            bmin[0] = bmin[0].min(x);
            bmin[1] = bmin[1].min(y);
            bmax[0] = bmax[0].max(x);
            bmax[1] = bmax[1].max(y);
        }
        (0..2).all(|i| {
            (self.origin[i] - bmin[i]).abs() < 1e-2 && (self.extent[i] - (bmax[i] - bmin[i])).abs() < 1e-2
        })
    }
}

fn poly_next(poly: &[[f32; 2]], i: usize) -> [f32; 2] {
    poly[(i + 1) % poly.len()]
}

/// Die Shoelace-Fläche (Vorzeichen zeigt die Orientierung).
fn area(poly: &[[f32; 2]]) -> f64 {
    let mut sum = 0_f64;
    let n = poly.len();
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        sum += f64::from(a[0]) * f64::from(b[1]) - f64::from(b[0]) * f64::from(a[1]);
    }
    sum * 0.5
}

/// Scanline-Rasterfüllung eines Dreiecks: jede Zelle, deren Mitte in dem
/// Dreieck liegt, markiert. Zellmitten坐标系 = Zellindex + 0,5.
fn fill_triangle(
    field: &mut [u16],
    w: i64,
    h: i64,
    res: f32,
    bmin: [f32; 2],
    pad: i64,
    tri: [&[f32; 3]; 3],
) -> Option<()> {
    let to_grid = |p: &[f32; 3]| -> [f64; 2] {
        [
            (f64::from(p[0]) - f64::from(bmin[0])) / f64::from(res) + pad as f64,
            (f64::from(p[1]) - f64::from(bmin[1])) / f64::from(res) + pad as f64,
        ]
    };
    let a = to_grid(tri[0]);
    let b = to_grid(tri[1]);
    let c = to_grid(tri[2]);
    let area2 = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    if area2.abs() < 1e-12 {
        return None;
    }
    let min_y = a[1].min(b[1]).min(c[1]).floor() as i64;
    let max_y = a[1].max(b[1]).max(c[1]).ceil() as i64;
    let min_x = a[0].min(b[0]).min(c[0]).floor() as i64;
    let max_x = a[0].max(b[0]).max(c[0]).ceil() as i64;
    // Orientierung des Tests normalisieren (Fläche positiv).
    let flip = area2 < 0.0;
    let edge = |p: [f64; 2], v: [f64; 2], w: [f64; 2]| -> f64 {
        let s = (w[0] - v[0]) * (p[1] - v[1]) - (w[1] - v[1]) * (p[0] - v[0]);
        if flip {
            -s
        } else {
            s
        }
    };
    for j in min_y..=max_y {
        if j < 0 || j >= h {
            continue;
        }
        for i in min_x..=max_x {
            if i < 0 || i >= w {
                continue;
            }
            // Zellmitte; halbe Zelle Aufschlag, damit Grenzstreifen, die
            // exakt auf Kanten liegen, sicher drin landen.
            let p = [i as f64 + 0.5, j as f64 + 0.5];
            if edge(p, a, b) >= 0.0 && edge(p, b, c) >= 0.0 && edge(p, c, a) >= 0.0 {
                field[(j * w + i) as usize] = 0;
            }
        }
    }
    Some(())
}

/// Ramer-Douglas-Peucker vereinfachen (rekursiv über Stack, toleranzdick).
fn simplify(poly: &[[f32; 2]], tol: f32) -> Vec<[f32; 2]> {
    if poly.len() <= 3 {
        return poly.to_vec();
    }
    let tol2 = tol * tol;
    let mut keep = vec![false; poly.len()];
    keep[0] = true;
    keep[poly.len() - 1] = true;
    let mut stack = vec![(0_usize, poly.len() - 1)];
    while let Some((first, last)) = stack.pop() {
        let a = poly[first];
        let b = poly[last];
        let ab = [f64::from(b[0] - a[0]), f64::from(b[1] - a[1])];
        let ab_len2 = ab[0] * ab[0] + ab[1] * ab[1];
        let mut best = (0_usize, -1_f64);
        for (i, p) in poly.iter().enumerate().skip(first + 1).take(last - first - 1) {
            let p = *p;
            let ap = [f64::from(p[0] - a[0]), f64::from(p[1] - a[1])];
            let t = if ab_len2 > 0.0 {
                (ap[0] * ab[0] + ap[1] * ab[1]) / ab_len2
            } else {
                0.0
            };
            let t = t.clamp(0.0, 1.0);
            let foot = [a[0] as f64 + t * ab[0], a[1] as f64 + t * ab[1]];
            let dx = f64::from(p[0]) - foot[0];
            let dy = f64::from(p[1]) - foot[1];
            let d2 = dx * dx + dy * dy;
            if d2 > best.1 {
                best = (i, d2);
            }
        }
        if best.1 > f64::from(tol2) {
            keep[best.0] = true;
            stack.push((first, best.0));
            stack.push((best.0, last));
        }
    }
    poly.iter().zip(&keep).filter_map(|(p, &k)| k.then_some(*p)).collect()
}

// ---------------------------------------------------------
// DXF-Export (R12: minimale, breit lesbare Entities)
// ---------------------------------------------------------

/// Kontur als geschlossene Polylinie in eine DXF-Datei (R12, Millimeter)
/// schreiben, Schicht „SCHATTENKONTUR“.
pub fn write_dxf(path: &std::path::Path, poly: &[[f32; 2]]) -> Result<(), String> {
    if poly.len() < 3 {
        return Err("Kontur ist zu kurz für eine DXF-Polylinie".to_string());
    }
    let mut out = String::with_capacity(poly.len() * 64 + 256);
    let mut w = |code: &str, val: &str| {
        out.push_str(code);
        out.push('\n');
        out.push_str(val);
        out.push('\n');
    };
    w("0", "SECTION");
    w("2", "HEADER");
    w("9", "$INSUNITS");
    w("70", "4"); // Millimeter
    w("9", "$MEASUREMENT");
    w("70", "1"); // metrisch
    w("0", "ENDSEC");
    w("0", "SECTION");
    w("2", "ENTITIES");
    w("0", "POLYLINE");
    w("8", "SCHATTENKONTUR");
    w("66", "1"); // Vertex-Flag: Vertices folgen
    w("70", "1"); // geschlossen
    w("10", "0.0");
    w("20", "0.0");
    w("30", "0.0");
    for p in poly {
        w("0", "VERTEX");
        w("8", "SCHATTENKONTUR");
        w("10", &format!("{:.4}", p[0]));
        w("20", &format!("{:.4}", p[1]));
        w("30", "0.0");
    }
    w("0", "SEQEND");
    w("8", "SCHATTENKONTUR");
    w("0", "ENDSEC");
    w("0", "EOF");
    std::fs::write(path, out).map_err(|e| format!("DXF nicht schreibbar: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ein Quader (w×d×h mm) als Mesh, Ecken unten links auf
    /// (0, 0, 0) — wie die Rohteil-Box, gleichgültig wie gebaut.
    fn box_mesh(w: f32, d: f32, hgt: f32) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
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
            [0, 3, 2, 1], // unten
            [4, 5, 6, 7], // oben
            [0, 1, 5, 4], // vorne (y = 0)
            [3, 7, 6, 2], // hinten
            [0, 4, 7, 3], // links
            [1, 2, 6, 5], // rechts
        ];
        let mut triangles = Vec::new();
        for q in &quads {
            triangles.push([q[0], q[1], q[2]]);
            triangles.push([q[0], q[2], q[3]]);
        }
        (positions, triangles)
    }

    #[test]
    fn box_10mm_expanded_by_3mm() {
        // Quader 20×10×5; um 3 mm: gerundetes Rechteck 26×16 mit
        // Eckenradius 3 → Fläche 26·16 − (4−π)·3² ≈ 408,3 mm²,
        // Umfang 2·(26+16) − 8·3 + 2π·3 ≈ 78,85 mm.
        let (positions, triangles) = box_mesh(20.0, 10.0, 5.0);
        let result = from_mesh(&positions, &triangles, 3.0).unwrap();
        assert!(
            (401.0..=415.0).contains(&result.area),
            "Fläche {} mm²",
            result.area
        );
        assert!(
            (76.0..=81.0).contains(&result.perimeter),
            "Umfang {} mm",
            result.perimeter
        );
        assert!(result.polygon.len() > 8, "zu wenige Punkte");
        // Ursprung an der Bauteil-Ecke (0,0): die Kontur liegt außenrum,
        // nach unten/links also bis −Abstand (nachgedacht: gerundet).
        let minx = result.polygon.iter().map(|p| p[0]).fold(f32::MAX, f32::min);
        let miny = result.polygon.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
        let maxx = result.polygon.iter().map(|p| p[0]).fold(f32::MIN, f32::max);
        let maxy = result.polygon.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
        assert!(minx >= -3.05 && miny >= -3.05, "minx={minx} miny={miny}");
        assert!(
            (minx + 3.1).abs() < 0.6 && (miny + 3.1).abs() < 0.6,
            "Kontur sollte ~−Abstand unter der Bauteilecke liegen: minx={minx} miny={miny}"
        );
        assert!(
            (maxx - 23.1).abs() < 0.6 && (maxy - 13.1).abs() < 0.6,
            "Kontur sollte bis ~Bauteil+Abstand reichen: maxx={maxx} maxy={maxy}"
        );
    }

    #[test]
    fn dxf_contains_one_polyline_with_all_vertices() {
        let (positions, triangles) = box_mesh(10.0, 10.0, 2.0);
        let result = from_mesh(&positions, &triangles, 1.0).unwrap();
        let path = std::env::temp_dir().join("rohteil_shadow_test.dxf");
        write_dxf(&path, &result.polygon).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(text.matches("POLYLINE").count(), 1);
        assert_eq!(text.matches("SEQEND").count(), 1);
        assert_eq!(text.matches("VERTEX").count(), result.polygon.len());
        assert!(text.contains("$INSUNITS"));
    }
}

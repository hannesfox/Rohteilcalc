//! Schattenkontur: die Außenkontur des Bauteils, auf die XY-Ebene projiziert
//! („Schatten von oben“), parallel nach außen um einen einstellbaren Abstand
//! vergrößert — exportierbar als DXF (mm).
//!
//! Verfahren (rastergeführt, exakt ausgleichend): alle Netzdreiecke werden
//! in die XY-Ebene gerastert und daraus ein Chamfer-Abstandsfeld gebaut —
//! aber nur als Führungsfeld. Im Band um die spätere Kontur wird die
//! Führung durch den **analytischen Abstand** zu allen projizierten
//! Dreiecken ersetzt (räumlich gebucketet, exakte Punkt-Dreieck-Distanz in
//! f64), und jeder Marching-Squares-Schnittpunkt wird per 1D-Sekantensuche
//! auf die exakte Isolinie „Abstand = Vorgabe“ korrigiert. Die Kontur ist
//! damit exakt konturparallel: an konvexen Ecken liegt sie exakt auf dem
//! Kreisbogen Radius = Abstand (wie eine Fräserbahn), an konkaven Ecken
//! auf dem verlängerten Miter-Schnittpunkt der Parallelen; die
//! Gitterauflösung steuert nur noch die Punktedichte, nicht mehr die
//! Genauigkeit.
//!
//! Rasterung + Führung halten die Extraktion bei jeder Geometrie robust —
//! überlappende Projektionen, streifige Netzkanten und offene Schalen
//! erzeugen weder Selbstüberschneidungen noch Bruchstücke.

use std::collections::HashMap;

use crate::renderer::ScModel;

/// Fertige Schattenkontur (alles in mm).
#[derive(Clone)]
pub struct ShadowResult {
    /// Abstand, um den die Außenkontur nach außen vergrößert ist.
    pub expand: f32,
    /// Die Außenkontur als geschlossene Polygonliste (gegen den Uhrzeigersinn),
    /// Ursprung in der Bauteilmitte: die Koordinaten sind in X/Y symmetrisch
    /// um (0,0). Einheit mm.
    pub polygon: Vec<[f32; 2]>,
    /// Innenkonturen (z. B. Bohrungen/Taschen), ebenfalls geschlossen und in
    /// denselben lokalen Koordinaten wie `polygon`. Sie werden mit einem
    /// Versatz nach innen (in die Materialseite) in die DXF geschrieben.
    pub inner_polygons: Vec<Vec<[f32; 2]>>,
    /// Versatz nach innen (mm), der auf Innenkonturen angewendet wurde.
    pub inner_offset: f32,
    /// Umfang (mm) und Fläche (mm²) der Außenkontur.
    pub perimeter: f64,
    pub area: f64,
    /// Rasterauflösung in mm — die Punktedichte dieser Kontur (die Abstände
    /// selbst sind exakt, siehe Moduldokumentation).
    pub res: f32,
    /// Weltlage des Polygon-Ursprungs (Bauteilmitte in X/Y) und Ausdehnung —
    /// für die 3D-Vorschau und den Verfalls-Check bei neuem Modell.
    pub origin: [f32; 2],
    pub extent: [f32; 2],
}

/// Schattenkontur aus dem aktuellen Anzeigemodell erzeugen.
/// `inner_min_mm`: Mindestgröße für automatisch erkannte Innenkonturen
/// (0 = keine). `inner_offset_mm`: Versatz nach innen für diese Konturen.
pub fn build(
    model: &ScModel,
    expand: f32,
    inner_min_mm: f32,
    inner_offset_mm: f32,
) -> Result<ShadowResult, String> {
    if model.part_triangles.is_empty() {
        return Err("keine Bauteilgeometrie zum Projizieren".to_string());
    }
    from_mesh(
        &model.part_positions,
        &model.part_triangles,
        expand,
        inner_min_mm,
        inner_offset_mm,
    )
}

/// Schattenkontur aus rohen Dreiecken (mm, XY-Projektion) erzeugen.
pub fn from_mesh(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    expand: f32,
    inner_min_mm: f32,
    inner_offset_mm: f32,
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
    // zwischen 0,02 und 0,25 mm (steuert nur die Punktedichte).
    let res = (ext[0].max(ext[1]) / 1600.0).clamp(0.02, 0.25);
    // Bei Zulage 0 liegt die Zielkontur genau auf dem Plateau des
    // Abstandsfelds (Bauteilinnen = 0) — Marching Squares würde dann an
    // den Gitterknoten entlang treppen. Deshalb mit einem Mikro-Abstand
    // rechnen (Bruchteil einer Rasterzelle, unterhalb jeder sichtbaren
    // oder toleranzrelevanten Größe): Die Kontur bleibt glatt und liegt
    // praktisch exakt auf der Silhouette.
    let iso = if expand > 0.0 { expand } else { res * 0.02 };
    let iso_cells = (iso / res).ceil() as i64 + 1;
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

    // Chamfer-Abstandsfeld (zwei Durchläufe, 3/4-Kosten) in Zellen — nur
    // Führungsfeld für die Bandauswahl, die Genauigkeit kommt später.
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
    // Rasterrand garantiert über der Kontur liegt (Beständigkeit bleibt).
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
        ];
        // Chamfer-Einheiten → Zellen (/3) → mm (* res).
        (u32::from(vals[0])
            + u32::from(vals[1])
            + u32::from(vals[2])
            + u32::from(vals[3])) as f32
            / 4.0
            / 3.0
            * res
    };

    // -------------------------------------------------------
    // Exakter Abstand zu den projizierten Dreiecken: Der Chamfer-Wert ist
    // nur Führung. Im Band um die spätere Kontur wird jeder Knotenwert
    // durch den analytischen Abstand zum projizierten Dreieckssatz ersetzt.
    // -------------------------------------------------------
    // Abfrage-Radius der exakten Suche: die Kontur (iso) plus eine
    // Rasterzelle Reserve für die Sekanten-Probehandler.
    let qradius = iso + 4.0 * res + 1e-3;
    let bins = TriBins::new(positions, triangles, bmin, bmax, qradius);

    // Knotentabelle: Abstand an jedem Zell-Eckpunkt. `f32::NAN` heißt
    // „außerhalb des Bandes — Chamfer-Mittelwert benutzen“.
    let mut nodes = vec![f32::NAN; ((w + 1) * (h + 1)) as usize];
    let node_pos = |i: i64, j: i64| -> [f64; 2] {
        [
            f64::from(bmin[0]) + f64::from((i - pad) as f32) * f64::from(res),
            f64::from(bmin[1]) + f64::from((j - pad) as f32) * f64::from(res),
        ]
    };
    let band = 8.0 * res;
    for j in 0..=h {
        for i in 0..=w {
            let est = corners(i, j);
            if (est - iso).abs() > band {
                // Tief drin: alle vier Zellen besetzt = Abstand 0, ohne
                // teure Bucket-Abfrage.
                if get(&field, i - 1, j - 1) == 0
                    && get(&field, i, j - 1) == 0
                    && get(&field, i - 1, j) == 0
                    && get(&field, i, j) == 0
                {
                    nodes[(j * (w + 1) + i) as usize] = 0.0;
                }
                continue;
            }
            let d = bins.dist(node_pos(i, j));
            nodes[(j * (w + 1) + i) as usize] = d as f32;
        }
    }
    let node_val = |i: i64, j: i64| -> f32 {
        let n = nodes[(j * (w + 1) + i) as usize];
        if n.is_nan() { corners(i, j) } else { n }
    };

    // -------------------------------------------------------
    // Marching Squares bei Abstand = Vorgabe.
    // -------------------------------------------------------
    // Ein Schnittpunkt liegt erst linear interpoliert, dann (deterministisch,
    // nur Funktionsauswertungen) per Sekantensuche exakt auf der Isolinie.
    // Argumentreihenfolge der Kanten ist zellübergreifend identisch
    // (unten einer Zelle = oben der Nachbarzelle, gleiche Richtung), so
    // dass Nachbarschnittpunkte bitidentisch sind und die Kontur sich
    // fehlerfrei schließt.
    let crossing = |pa: [f32; 2], va: f32, pb: [f32; 2], vb: f32| -> Option<[f32; 2]> {
        if (va <= iso) == (vb <= iso) {
            return None;
        }
        let t = ((iso - va) / (vb - va)) as f64;
        let lin = [
            f64::from(pa[0]) + f64::from(pb[0] - pa[0]) * t,
            f64::from(pa[1]) + f64::from(pb[1] - pa[1]) * t,
        ];
        let f = |p: [f64; 2]| -> f64 { bins.dist(p) - f64::from(iso) };
        // Exakte Endpunkte: Vorzeichenwechsel entlang der Zellkante suchen.
        let fa = f([f64::from(pa[0]), f64::from(pa[1])]);
        let fb = f([f64::from(pb[0]), f64::from(pb[1])]);
        if fa == 0.0 {
            return Some(pa);
        }
        if fb == 0.0 {
            return Some(pb);
        }
        if (fa < 0.0) != (fb < 0.0) {
            // Richtung straddelt exakt — Ursprung ist gültig.
            let (mut t0, mut f0) = (0.0, fa);
            let (mut t1, mut f1) = (1.0, fb);
            for _ in 0..40 {
                let df = f1 - f0;
                let t_new = if df.abs() > 1e-30 {
                    (t1 - f1 * (t1 - t0) / df).clamp(0.0, 1.0)
                } else {
                    (t0 + t1) * 0.5
                };
                let fn_new = f([
                    f64::from(pa[0]) + f64::from(pb[0] - pa[0]) * t_new,
                    f64::from(pa[1]) + f64::from(pb[1] - pa[1]) * t_new,
                ]);
                if fn_new.abs() < 1e-4 * f64::from(res) {
                    return Some([
                        (f64::from(pa[0]) + f64::from(pb[0] - pa[0]) * t_new) as f32,
                        (f64::from(pa[1]) + f64::from(pb[1] - pa[1]) * t_new) as f32,
                    ]);
                }
                // Intervall auf den Vorzeichenwechsel nicht vergrößern.
                if fn_new * f1 < 0.0 {
                    t0 = t_new;
                    f0 = fn_new;
                } else if fn_new * f0 < 0.0 {
                    t1 = t_new;
                    f1 = fn_new;
                } else {
                    // Seltenes Plateau (Ungleichheit unklar): halbieren.
                    t1 = t_new;
                    f1 = fn_new;
                }
            }
            let point = if f0.abs() < f1.abs() { (t0, f0) } else { (t1, f1) };
            return Some([
                (f64::from(pa[0]) + f64::from(pb[0] - pa[0]) * point.0) as f32,
                (f64::from(pa[1]) + f64::from(pb[1] - pa[1]) * point.0) as f32,
            ]);
        }
        // Exakt kein Vorzeichenwechsel auf der Kante (Chamfer-Werte leicht
        // verschmiert): lineaire Schätzung bleibt — deterministisch und
        // zellübergreifend identisch.
        Some([lin[0] as f32, lin[1] as f32])
    };
    let mut segments: Vec<([f32; 2], [f32; 2])> = Vec::new();
    for j in 0..h {
        for i in 0..w - 1 {
            let (p00, p10, p11, p01) = (node(i, j), node(i + 1, j), node(i + 1, j + 1), node(i, j + 1));
            let (v00, v10, v11, v01) = (node_val(i, j), node_val(i + 1, j), node_val(i + 1, j + 1), node_val(i, j + 1));
            let bottom = crossing(p00, v00, p10, v10);
            let right = crossing(p10, v10, p11, v11);
            let top = crossing(p01, v01, p11, v11);
            let left = crossing(p00, v00, p01, v01);
            let hits = [bottom, right, top, left];
            let count = hits.iter().filter(|hh| hh.is_some()).count();
            match count {
                0 => {}
                2 => {
                    let pts: Vec<[f32; 2]> = hits.iter().filter_map(|hh| *hh).collect();
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
    // verknüpfen — die extrahierte Richtung erster Paare wechselt mit dem
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

    // Größte Kontur = Außenkontur; übrige (bei inner_min_mm > 0, gefiltert)
    // = Innenkonturen (Silhouetten-Löcher/Bohrungen in XY).
    let mut best_area = -1.0_f64;
    let mut best_idx: Option<usize> = None;
    for (i, l) in loops.iter().enumerate() {
        let a = area(l).abs();
        if a > best_area {
            best_area = a;
            best_idx = Some(i);
        }
    }
    let best_idx = best_idx.ok_or("leere Kontur")?;
    let mut polygon = loops[best_idx].clone();
    if area(&polygon) < 0.0 {
        polygon.reverse();
    }

    // Ursprung in die Bauteilmitte legen: die Polygon-Koordinaten sind in
    // X/Y symmetrisch um (0,0); die Weltlage wandert in `origin`.
    let mid = [bmin[0] + ext[0] * 0.5, bmin[1] + ext[1] * 0.5];
    for p in polygon.iter_mut() {
        p[0] -= mid[0];
        p[1] -= mid[1];
    }

    let mut inner_polygons = Vec::new();
    if inner_min_mm > 0.0 {
        let min = inner_min_mm as f64;
        for (i, l) in loops.iter().enumerate() {
            if i == best_idx {
                continue;
            }
            let mut p = l.clone();
            // Größe prüfen (vor dem Zentrieren egal, bbox relativ)
            let mut bx = [f32::MAX, f32::MIN];
            let mut by = [f32::MAX, f32::MIN];
            for pt in &p {
                bx[0] = bx[0].min(pt[0]);
                bx[1] = bx[1].max(pt[0]);
                by[0] = by[0].min(pt[1]);
                by[1] = by[1].max(pt[1]);
            }
            let dx = (bx[1] - bx[0]) as f64;
            let dy = (by[1] - by[0]) as f64;
            let eq_d = 2.0 * (area(&p).abs() / std::f64::consts::PI).sqrt();
            let keep = (dx >= min && dy >= min) || (eq_d >= min);
            if !keep {
                continue;
            }
            for pt in p.iter_mut() {
                pt[0] -= mid[0];
                pt[1] -= mid[1];
            }
            match prepare_inner(&p, inner_offset_mm, res) {
                Some(clean) => inner_polygons.push(clean),
                None => continue, // Versatz frisst die Kontur auf
            }
        }
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
        inner_polygons,
        inner_offset: if inner_min_mm > 0.0 { inner_offset_mm } else { 0.0 },
        perimeter,
        res,
        origin: mid,
        extent: ext,
    })
}

impl ShadowResult {
    /// Passt die gespeicherte Kontur noch zu dieser Geometrie? (Die Lage
    /// der Projektion — Ursprung und Ausdehnung der Bounding-box — muss
    /// unverändert sein, sonst läuft das Modell von der Vorschau gefolgt —
    /// z. B. nach dem Laden einer neuen STEP-Datei.)
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
            (self.origin[i] - (bmin[i] + bmax[i]) * 0.5).abs() < 1e-2
                && (self.extent[i] - (bmax[i] - bmin[i])).abs() < 1e-2
        })
    }
}

fn poly_next(poly: &[[f32; 2]], i: usize) -> [f32; 2] {
    poly[(i + 1) % poly.len()]
}

/// Die Shoelace-Fläche (Vorzeichen zeigt die Orientierung).
/// Kreisausgleich (kleinste Quadrate, Kasa-Methode): Center und Radius eines
/// Punktzugs. Liefert `None`, wenn der Zug nicht kreisnah ist (RMS-Abweichung
/// zu groß) oder keine vollständige Umrundung vorliegt.
fn fit_circle(poly: &[[f32; 2]]) -> Option<([f64; 2], f64)> {
    let n = poly.len();
    if n < 12 {
        return None;
    }
    // Lineares Ausgleichssystem: r² = x²+y² + a·x + b·y + c
    let mut s = [0.0_f64; 6]; // Sx, Sy, Sz(=n), Sxx, Sxy, Syy
    let mut t = [0.0_f64; 4]; // Sxz, Syz, Sx·, Sy· (Momenten-Summen)
    for p in poly {
        let x = f64::from(p[0]);
        let y = f64::from(p[1]);
        let z = x * x + y * y;
        s[0] += x;
        s[1] += y;
        s[2] += 1.0;
        s[3] += x * x;
        s[4] += x * y;
        s[5] += y * y;
        t[0] += x * z;
        t[1] += y * z;
        t[2] += z;
    }
    // [Sxx Sxy Sx; Sxy Syy Sy; Sx Sy Sz] · [a b c]' = [Sxz Syz Sz]'
    let m = [
        [s[3], s[4], s[0]],
        [s[4], s[5], s[1]],
        [s[0], s[1], s[2]],
    ];
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-9 {
        return None;
    }
    // Cramer'sche Regel.
    let solve = |rhs: [f64; 3]| -> [f64; 3] {
        [
            (rhs[0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
                - m[0][1] * (rhs[1] * m[2][2] - m[1][2] * rhs[2])
                + m[0][2] * (rhs[1] * m[2][1] - m[1][1] * rhs[2]))
                / det,
            (m[0][0] * (rhs[1] * m[2][2] - m[1][2] * rhs[2])
                - rhs[0] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
                + m[0][2] * (m[1][0] * rhs[2] - rhs[1] * m[2][0]))
                / det,
            (m[0][0] * (m[1][1] * rhs[2] - rhs[1] * m[2][1])
                - m[0][1] * (m[1][0] * rhs[2] - rhs[1] * m[2][0])
                + rhs[0] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]))
                / det,
        ]
    };
    let [a, b, c] = solve([t[0], t[1], t[2]]);
    let cx = a * 0.5;
    let cy = b * 0.5;
    let r2 = c + cx * cx + cy * cy;
    if r2 <= 1e-6 {
        return None;
    }
    let r = r2.sqrt();
    if !r.is_finite() || r < 0.5 || r > 5000.0 {
        return None;
    }
    // Güte: RMS-Abweichung der Punkte zum Kreis (Pastillierung/Rasterstufen
    // dürfen drin sein — der Fit mittelt sie weg).
    let mut var = 0.0_f64;
    for p in poly {
        let dx = f64::from(p[0]) - cx;
        let dy = f64::from(p[1]) - cy;
        let d = (dx * dx + dy * dy).sqrt();
        let e = d - r;
        var += e * e;
    }
    let rms = (var / n as f64).sqrt();
    if rms > (r * 0.06).max(0.35) {
        return None;
    }
    // Vollständige Umrundung? Größte Lücke der Winkel qualsiasi.
    let mut angles: Vec<f64> = poly
        .iter()
        .map(|p| (f64::from(p[1]) - cy).atan2(f64::from(p[0]) - cx))
        .collect();
    angles.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
    let mut gap = angles[0] + std::f64::consts::TAU - angles[n - 1];
    for w in angles.windows(2) {
        gap = gap.max(w[1] - w[0]);
    }
    if gap > std::f64::consts::FRAC_PI_2 {
        return None;
    }
    Some(([cx, cy], r))
}

/// Exakter Kreis als Polygonliste (Segmentzahl so, dass die Sehnen-Pfeilhöhe
/// unter 0,01 mm bleibt — glatt für Vorschau und DXF).
fn circle_points(center: [f64; 2], radius: f64) -> Vec<[f32; 2]> {
    let r = radius.max(0.2);
    let err = 0.01_f64.min(r * 0.05);
    let n = (((std::f64::consts::PI
        / (1.0 - err / r).acos().max(1e-6)) as usize)
        .clamp(48, 512))
    .next_power_of_two();
    let n = n.min(512);
    (0..n)
        .map(|i| {
            let t = i as f64 / n as f64 * std::f64::consts::TAU;
            [
                (center[0] + r * t.cos()) as f32,
                (center[1] + r * t.sin()) as f32,
            ]
        })
        .collect()
}

/// Ramer–Douglas–Peucker: fast kollineareurate Punkte eines geschlossenen
/// Zugs entfernen (Toleranz in mm). Gegen Zeitstempelmuster/Rechenlast:
/// einfache iterative Variante mit Stack über Teilintervalle.
fn simplify_rdp(poly: &[[f32; 2]], tol: f32) -> Vec<[f32; 2]> {
    let n = poly.len();
    let tol2 = (tol * tol) as f64;
    if n < 5 || tol <= 1e-4 {
        return poly.to_vec();
    }
    // Doppelte Schlusskante vermeiden: offenen Zug 0..n behandeln.
    let mut keep = vec![false; n];
    keep[0] = true;
    let mut stack: Vec<(usize, usize)> = vec![(0, n - 1)];
    while let Some((i0, i1)) = stack.pop() {
        if i1 <= i0 + 1 {
            continue;
        }
        // Größte Abweichung vom Segment (i0, i1)
        let ax = f64::from(poly[i0][0]);
        let ay = f64::from(poly[i0][1]);
        let bx = f64::from(poly[i1][0]);
        let by = f64::from(poly[i1][1]);
        let dx = bx - ax;
        let dy = by - ay;
        let len2 = dx * dx + dy * dy;
        let mut best_i = i0;
        let mut best_d2 = -1.0_f64;
        for i in (i0 + 1)..i1 {
            let px = f64::from(poly[i][0]);
            let py = f64::from(poly[i][1]);
            let d2 = if len2 < 1e-18 {
                (px - ax) * (px - ax) + (py - ay) * (py - ay)
            } else {
                let t = (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0);
                let qx = ax + t * dx - px;
                let qy = ay + t * dy - py;
                qx * qx + qy * qy
            };
            if d2 > best_d2 {
                best_d2 = d2;
                best_i = i;
            }
        }
        if best_d2 > tol2 {
            keep[best_i] = true;
            stack.push((i0, best_i));
            stack.push((best_i, i1));
        }
    }
    let out: Vec<[f32; 2]> = (0..n).filter(|&i| keep[i]).map(|i| poly[i]).collect();
    if out.len() >= 3 {
        out
    } else {
        poly.to_vec()
    }
}

/// Einfacher polygonaler Versatz „nach innen" (zur Polygonmitte hin, Miter).
/// `d` > 0 verkleinert die Kontur. Bei degeneriertem Ergebnis (zu klein/
/// Selbstüberschneidungstendenz) wird ein leerer Vektor zurückgegeben.
pub fn offset_polygon_inward(poly: &[[f32; 2]], d: f32) -> Vec<[f32; 2]> {
    let n = poly.len();
    if n < 3 || d.abs() < 1e-6 {
        return poly.to_vec();
    }
    let d = d as f64;
    let a = area(poly);
    // CCW (a>0): Innere ist links; CW: Innere ist rechts.
    let sign = if a >= 0.0 { 1.0 } else { -1.0 };
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let p0 = [f64::from(poly[(i + n - 1) % n][0]), f64::from(poly[(i + n - 1) % n][1])];
        let p1 = [f64::from(poly[i][0]), f64::from(poly[i][1])];
        let p2 = [f64::from(poly[(i + 1) % n][0]), f64::from(poly[(i + 1) % n][1])];
        let e1 = [p1[0] - p0[0], p1[1] - p0[1]];
        let e2 = [p2[0] - p1[0], p2[1] - p1[1]];
        let l1 = (e1[0] * e1[0] + e1[1] * e1[1]).sqrt();
        let l2 = (e2[0] * e2[0] + e2[1] * e2[1]).sqrt();
        if l1 < 1e-9 || l2 < 1e-9 {
            out.push(poly[i]);
            continue;
        }
        // Linksnormale (CCW-Innere): (-dy, dx)
        let n1 = [-e1[1] / l1 * sign, e1[0] / l1 * sign];
        let n2 = [-e2[1] / l2 * sign, e2[0] / l2 * sign];
        let p1o = [p1[0] + n1[0] * d, p1[1] + n1[1] * d];
        let p2o = [p1[0] + n2[0] * d, p1[1] + n2[1] * d];
        // Schnitt der versetzten Kanten (Miter)
        let det = e1[0] * e2[1] - e1[1] * e2[0];
        let pt = if det.abs() > 1e-9 {
            let t = ((p2o[0] - p1o[0]) * e2[1] - (p2o[1] - p1o[1]) * e2[0]) / det;
            [p1o[0] + t * e1[0], p1o[1] + t * e1[1]]
        } else {
            [(p1o[0] + p2o[0]) * 0.5, (p1o[1] + p2o[1]) * 0.5]
        };
        // Miter-Limit gegen Ausreißer
        let mlen = ((pt[0] - p1[0]).powi(2) + (pt[1] - p1[1]).powi(2)).sqrt();
        let limit = d.abs() * 4.0 + (l1.min(l2)) * 0.5;
        let pt = if mlen > limit && mlen > 1e-9 {
            [p1[0] + (pt[0] - p1[0]) / mlen * limit, p1[1] + (pt[1] - p1[1]) / mlen * limit]
        } else {
            pt
        };
        out.push([pt[0] as f32, pt[1] as f32]);
    }
    // Degeneriert? (Fläche kollabiert oder Selbstschnitt grob erkannt)
    let a2 = area(&out).abs();
    if !a2.is_finite() || a2 < 1e-4 {
        return Vec::new();
    }
    out
}

/// Versucht, aus einer zylindrischen Fläche (z. B. Bohrung) einen Kreis in
/// XY abzuleiten. Liefert `Some((mittelpunkt_lokal, radius))` wenn die
/// Fläche hinreichend zylindrisch ist.
pub fn extract_cylinder_xy(
    mesh: &solidcraft_geom::Mesh,
    tri_face: u32,
    origin: [f32; 2],
) -> Option<Vec<[f32; 2]>> {
    // Punkte und Normalen der Dreiecke dieser Fläche sammeln.
    let mut pts: Vec<[f64; 3]> = Vec::new();
    let mut nrm: Vec<[f64; 3]> = Vec::new();
    for (ti, tri) in mesh.triangles.iter().enumerate() {
        if mesh.tri_face.get(ti).copied().unwrap_or(u32::MAX) != tri_face {
            continue;
        }
        for &vi in tri {
            if let Some(p) = mesh.positions.get(vi as usize) {
                pts.push([p.x, p.y, p.z]);
            }
            if let Some(n) = mesh.normals.get(vi as usize) {
                let l = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
                if l > 1e-9 {
                    nrm.push([n.x / l, n.y / l, n.z / l]);
                }
            }
        }
    }
    if pts.len() < 12 || nrm.len() < 12 {
        return None;
    }
    // Achse: Richtung a, für die |n·a| über alle Normalen minimal ist
    // (Zylindernormalen stehen senkrecht zur Achse). Grobe Suche über
    // Kandidaten (Weltachsen + aus Normalpaaren) genügt für CAD-Bohrungen.
    let mut candidates: Vec<[f64; 3]> = vec![
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
    ];
    for i in 0..nrm.len().min(24) {
        for j in (i + 1)..nrm.len().min(24) {
            let c = [
                nrm[i][1] * nrm[j][2] - nrm[i][2] * nrm[j][1],
                nrm[i][2] * nrm[j][0] - nrm[i][0] * nrm[j][2],
                nrm[i][0] * nrm[j][1] - nrm[i][1] * nrm[j][0],
            ];
            let l = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
            if l > 1e-6 {
                candidates.push([c[0] / l, c[1] / l, c[2] / l]);
            }
        }
    }
    let mut best_axis = [0.0, 0.0, 1.0];
    let mut best_score = f64::INFINITY;
    for a in &candidates {
        let score: f64 = nrm.iter().map(|n| (n[0] * a[0] + n[1] * a[1] + n[2] * a[2]).abs()).sum::<f64>()
            / nrm.len() as f64;
        if score < best_score {
            best_score = score;
            best_axis = *a;
        }
    }
    // Zylindrisch, wenn Normalen im Mittel nahezu senkrecht zur Achse stehen.
    if best_score > 0.25 {
        return None;
    }
    // Radius und Achspunkt: projiziere Punkte in Ebene senkrecht zur Achse,
    // schätze Zentrum als Mittelwert p - r*n (r via Abstand zum Mittel) und
    // verfeinere einfach.
    let a = best_axis;
    let mean = [
        pts.iter().map(|p| p[0]).sum::<f64>() / pts.len() as f64,
        pts.iter().map(|p| p[1]).sum::<f64>() / pts.len() as f64,
        pts.iter().map(|p| p[2]).sum::<f64>() / pts.len() as f64,
    ];
    // Grobes Zentrum: projiziere jeden Punkt entlang radialer Komponente
    // (Punkt minus Achsanteil relativ zu mean) und mitteln.
    let mut center = [0.0_f64; 3];
    let mut r_sum = 0.0_f64;
    let mut r_n = 0usize;
    for p in &pts {
        let v = [p[0] - mean[0], p[1] - mean[1], p[2] - mean[2]];
        let along = v[0] * a[0] + v[1] * a[1] + v[2] * a[2];
        let radial = [v[0] - along * a[0], v[1] - along * a[1], v[2] - along * a[2]];
        let r = (radial[0] * radial[0] + radial[1] * radial[1] + radial[2] * radial[2]).sqrt();
        if r > 1e-6 {
            // nächster Achspunkt für dieses p: p - radial
            center[0] += p[0] - radial[0];
            center[1] += p[1] - radial[1];
            center[2] += p[2] - radial[2];
            r_sum += r;
            r_n += 1;
        }
    }
    if r_n < 6 {
        return None;
    }
    center[0] /= r_n as f64;
    center[1] /= r_n as f64;
    center[2] /= r_n as f64;
    let radius = r_sum / r_n as f64;
    if !radius.is_finite() || radius < 0.2 || radius > 5000.0 {
        return None;
    }
    // Prüfe Zylinder-Güte (Abstandsstreuung)
    let mut var = 0.0_f64;
    let mut ok_n = 0usize;
    for p in &pts {
        let v = [p[0] - center[0], p[1] - center[1], p[2] - center[2]];
        let along = v[0] * a[0] + v[1] * a[1] + v[2] * a[2];
        let radial = [v[0] - along * a[0], v[1] - along * a[1], v[2] - along * a[2]];
        let r = (radial[0] * radial[0] + radial[1] * radial[1] + radial[2] * radial[2]).sqrt();
        let d = (r - radius).abs();
        var += d * d;
        ok_n += 1;
    }
    if ok_n == 0 {
        return None;
    }
    let rms = (var / ok_n as f64).sqrt();
    if rms > (radius * 0.08).max(0.25) {
        return None;
    }

    // Kreis in XY: Mittelpunkt = Achsdurchstoßpunkt projiziert, Radius =
    // Zylinderradius / cos(Neigung) wäre für geneigte Achsen korrekt für den
    // Schnitt; für DXF/Schatten ist die XY-Projektion des Zylinders meist
    // als Kreis mit nominalem Radius gewünscht, solange Achse ~Z. Wir nehmen
    // nominalen Radius und projizieren das Zentrum auf XY.
    let cx = center[0] as f32 - origin[0];
    let cy = center[1] as f32 - origin[1];
    let r = radius as f32;
    let segs = 64usize;
    let mut poly = Vec::with_capacity(segs);
    for i in 0..segs {
        let t = (i as f32) / (segs as f32) * std::f32::consts::TAU;
        poly.push([cx + r * t.cos(), cy + r * t.sin()]);
    }
    Some(poly)
}

/// Geschlossene XY-Konturen aus den Begrenzungskanten einer B-rep-Fläche
/// extrahieren (Tessellierung). Kantenpolylinien werden in Z projiziert
/// (`x-origin.x, y-origin.y`), nach Endpunktnähe verkettet und als
/// geschlossene 2D-Loops zurückgegeben.
pub fn extract_face_xy_loops(
    mesh: &solidcraft_geom::Mesh,
    tri_face: u32,
    origin: [f32; 2],
) -> Vec<Vec<[f32; 2]>> {
    let edge_ids = mesh.face_edges(tri_face);
    if edge_ids.is_empty() {
        return Vec::new();
    }
    let tol = 0.05_f32; // mm, Endpunkt-Toleranz beim Verketten
    let tol2 = tol * tol;
    let mut segs: Vec<(Vec<[f32; 2]>, [f32; 2], [f32; 2])> = Vec::new();
    for ei in edge_ids {
        let Some(pl) = mesh.edges.get(ei) else { continue };
        if pl.len() < 2 {
            continue;
        }
        let pts: Vec<[f32; 2]> = pl
            .iter()
            .map(|p| [p.x as f32 - origin[0], p.y as f32 - origin[1]])
            .collect();
        let a = *pts.first().unwrap();
        let b = *pts.last().unwrap();
        segs.push((pts, a, b));
    }
    let mut loops = Vec::new();
    let mut used = vec![false; segs.len()];
    for start in 0..segs.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut cur = segs[start].0.clone();
        let mut end = segs[start].2;
        let closed_tol2 = (tol * 2.0).powi(2);
        for _ in 0..(segs.len() + 2) {
            let start_pt = *cur.first().unwrap();
            if cur.len() >= 3 && (end[0] - start_pt[0]).powi(2) + (end[1] - start_pt[1]).powi(2) < closed_tol2 {
                break;
            }
            let mut found = None;
            for (j, seg) in segs.iter().enumerate() {
                if used[j] {
                    continue;
                }
                let da = (seg.1[0] - end[0]).powi(2) + (seg.1[1] - end[1]).powi(2);
                let db = (seg.2[0] - end[0]).powi(2) + (seg.2[1] - end[1]).powi(2);
                if da < tol2 {
                    found = Some((j, false));
                    break;
                } else if db < tol2 {
                    found = Some((j, true));
                    break;
                }
            }
            if let Some((j, rev)) = found {
                used[j] = true;
                if rev {
                    for p in segs[j].0.iter().rev().skip(1) {
                        cur.push(*p);
                    }
                    end = segs[j].1;
                } else {
                    for p in segs[j].0.iter().skip(1) {
                        cur.push(*p);
                    }
                    end = segs[j].2;
                }
            } else {
                break;
            }
        }
        if cur.len() >= 2 {
            let a = *cur.first().unwrap();
            let b = *cur.last().unwrap();
            if (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) < closed_tol2 {
                cur.pop();
            }
        }
        if cur.len() >= 3 && area(&cur).abs() > 0.5 {
            loops.push(cur);
        }
    }
    loops
}

/// Kleinste Kanten zusammenziehen: Mikro-Arcs (z. B. Eckrundung mit Radius
/// ≈ Rasterauflösung) haben normalparallele Mikro-Normalen und stören sonst
/// den Miter-Versatz (Eckpunkt wandert um ~0,5·d falsch). Jeder Punkt, dessen
/// eingehende Kante kürzer als `min_len` ist, entfällt.
fn collapse_small_edges(poly: &[[f32; 2]], min_len: f32) -> Vec<[f32; 2]> {
    let n = poly.len();
    if min_len <= 1e-6 || n < 5 {
        return poly.to_vec();
    }
    let min2 = min_len * min_len;
    let mut out: Vec<[f32; 2]> = Vec::with_capacity(n);
    for i in 0..n {
        let prev = poly[(i + n - 1) % n];
        let cur = poly[i];
        let d = cur[0] - prev[0];
        let e = cur[1] - prev[1];
        if d * d + e * e < min2 {
            continue; // eingehende Mikrokante: Punkt zusammenziehen
        }
        out.push(cur);
    }
    if out.len() < 3 {
        poly.to_vec()
    } else {
        out
    }
}

/// Roher Innenzug (bereits zentriert) → fertige Kontur für Vorschaubahne/DXF:
/// Kreis (Bohrung) wird erkannt und als exakter Kreis ausgegeben, Versatz
/// über den Radius; alles andere wird geglättet (RDP) und mit dem Versatz
/// nach innen versehen. `None`, wenn der Versatz die Kontur auf isst.
pub fn prepare_inner(poly: &[[f32; 2]], offset_mm: f32, res: f32) -> Option<Vec<[f32; 2]>> {
    // Kreis (Bohrung)? Dann exakter, sauberer Kreis ausgeben — Versatz
    // über den Radius (keine Miter-/Rasterartefakte).
    if let Some((c, r)) = fit_circle(poly) {
        let mut rr = r;
        if offset_mm.abs() > 1e-6 {
            let d = f64::from(offset_mm.max(0.0));
            if rr - d < 0.5 {
                return None; // Versatz frisst die Kontur auf
            }
            rr -= d;
        }
        return Some(circle_points(c, rr));
    }
    // Übrige Konturen glätten (Rasterstufen/nah-kollineare Punkte
    // herausnehmen), bevor der Versatz sie verstärken könnte.
    let tol = (res * 0.5).max(0.05);
    let mut clean = simplify_rdp(poly, tol);
    if clean.len() < 3 {
        clean = poly.to_vec();
    }
    if offset_mm.abs() > 1e-6 {
        // Mikro-Arcs zusammenziehen, bevor der Miter-Versatz ihre Mikro-
        // Normalen als Ecken fehlinterpretiert.
        let min_len = 0.02 + offset_mm.abs() * 0.2;
        clean = collapse_small_edges(&clean, min_len);
        if clean.len() < 3 {
            clean = poly.to_vec();
        }
        let off = offset_polygon_inward(&clean, offset_mm);
        if off.len() >= 3 {
            clean = off;
        } else {
            return None; // Versatz frisst die Kontur auf
        }
    }
    Some(clean)
}

/// Alle Innenzüge der Silhouette (ohne Größenfilter) plus Ursprung und
/// Rasterauflösung — Basis für die manuelle Auswahl per Klick, wenn der
/// angeklickte Wandzug selbst keine nutzbare Kontur liefert (z. B. Wand
/// einer Nut): der Innenzug, der dem Klickpunkt am nächsten liegt, wird
/// dort verwendet. Die Koordinaten sind lokal zu `origin` (Bauteilmitte).
pub fn silhouette_inner_loops(
    positions: &[[f32; 3]],
    triangles: &[[u32; 3]],
    expand: f32,
) -> Result<(Vec<Vec<[f32; 2]>>, [f32; 2], f32), String> {
    if triangles.is_empty() {
        return Err("keine Bauteilgeometrie zum Projizieren".to_string());
    }
    let r = from_mesh(positions, triangles, expand, 1.0e-3, 0.0)?;
    Ok((r.inner_polygons, r.origin, r.res))
}

/// Abstand eines Punkts zu einem geschlossenen Polygonzug (Mindestabstand
/// zu den Kanten), gleiche Einheit wie die Koordinaten.
pub fn point_poly_dist(p: [f32; 2], poly: &[[f32; 2]]) -> f32 {
    let n = poly.len();
    if n < 2 {
        return f32::INFINITY;
    }
    let mut best = f32::INFINITY;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let e = [b[0] - a[0], b[1] - a[1]];
        let l2 = e[0] * e[0] + e[1] * e[1];
        let v = [p[0] - a[0], p[1] - a[1]];
        let t = if l2 > 1e-18 {
            ((v[0] * e[0] + v[1] * e[1]) / l2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q = [v[0] - t * e[0], v[1] - t * e[1]];
        best = best.min((q[0] * q[0] + q[1] * q[1]).sqrt());
    }
    best
}

/// 2D-Polygonfläche, Vorzeichen = Orientierung (CCW positiv).
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

/// Absolutfläche eines 2D-Polygons (mm²).
pub fn area_abs(poly: &[[f32; 2]]) -> f64 {
    area(poly).abs()
}

/// Scanline-Rasterfüllung eines Dreiecks: jede Zelle, deren Mitte in dem
/// Dreieck liegt, markiert. Zellmitte-Koordinaten = Zellindex + 0,5.
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

// ---------------------------------------------------------
// Exakter Abstand zu den projizierten Dreiecken (f64)
// ---------------------------------------------------------

/// Projizierte Dreiecke in ein Gitter aus gleichförmigen Buckets einsortiert;
/// eine Abfrage beantwortet den exakten Abstand eines Punkts zur Vereinigung
/// aller XY-Projektionen — so lange die Buckets im FRINGE den Abstand
/// [`TriBins::radius`] garantieren (größere Werte können überschätzt
/// unterschritten werden, also nur obere Schranken liefern).
struct TriBins {
    cell: f32,
    ox: f32,
    oy: f32,
    nx: i64,
    ny: i64,
    radius: f32,
    tris: Vec<[[f64; 2]; 3]>,
    cells: Vec<Vec<u32>>,
}

impl TriBins {
    fn new(
        positions: &[[f32; 3]],
        triangles: &[[u32; 3]],
        bmin: [f32; 2],
        bmax: [f32; 2],
        radius: f32,
    ) -> Self {
        let cell = radius.max(1.0);
        let nx = (((bmax[0] - bmin[0]) / cell).ceil() as i64 + 1).max(1);
        let ny = (((bmax[1] - bmin[1]) / cell).ceil() as i64 + 1).max(1);
        let mut tris = Vec::with_capacity(triangles.len());
        let mut cells: Vec<Vec<u32>> = vec![Vec::new(); (nx * ny) as usize];
        for t in triangles {
            let a = [
                f64::from(positions[t[0] as usize][0]),
                f64::from(positions[t[0] as usize][1]),
            ];
            let b = [
                f64::from(positions[t[1] as usize][0]),
                f64::from(positions[t[1] as usize][1]),
            ];
            let c = [
                f64::from(positions[t[2] as usize][0]),
                f64::from(positions[t[2] as usize][1]),
            ];
            let proj = [a, b, c];
            let (tmin, tmax) = (
                [a[0].min(b[0]).min(c[0]), a[1].min(b[1]).min(c[1])],
                [a[0].max(b[0]).max(c[0]), a[1].max(b[1]).max(c[1])],
            );
            let idx = tris.len() as u32;
            tris.push(proj);
            // Jeden überlappenden Bucket markieren.
            let x0 = (((tmin[0] as f32 - bmin[0]) / cell).floor() as i64).clamp(0, nx - 1);
            let x1 = (((tmax[0] as f32 - bmin[0]) / cell).floor() as i64).clamp(0, nx - 1);
            let y0 = (((tmin[1] as f32 - bmin[1]) / cell).floor() as i64).clamp(0, ny - 1);
            let y1 = (((tmax[1] as f32 - bmin[1]) / cell).floor() as i64).clamp(0, ny - 1);
            for j in y0..=y1 {
                for i in x0..=x1 {
                    cells[(j * nx + i) as usize].push(idx);
                }
            }
        }
        Self { cell, ox: bmin[0], oy: bmin[1], nx, ny, radius, tris, cells }
    }

    /// Das Bucket eines Punkts (geklemmt an Ränder).
    fn bin_of(&self, p: [f64; 2]) -> (i64, i64) {
        (
            (((p[0] as f32 - self.ox) / self.cell).floor() as i64).clamp(0, self.nx - 1),
            (((p[1] as f32 - self.oy) / self.cell).floor() as i64).clamp(0, self.ny - 1),
        )
    }

    /// Exakter Abstand des Punkts zum projizierten Dreieckssatz, sofern
    /// innerhalb `radius`; außerhalb gibt das Ergebnis die höchste bekannte
    /// untere Grenze (`>= radius` heißt „größer“).
    fn dist(&self, p: [f64; 2]) -> f64 {
        // Ring, der die Abfrageschicht garantiert abdeckt.
        let k = ((self.radius / self.cell).ceil() as i64).max(1) + 1;
        let (bx, by) = self.bin_of(p);
        let mut best = f64::INFINITY;
        for dy in -k..=k {
            for dx in -k..=k {
                let (cx, cy) = (bx + dx, by + dy);
                if cx < 0 || cy < 0 || cx >= self.nx || cy >= self.ny {
                    continue;
                }
                for &idx in &self.cells[(cy * self.nx + cx) as usize] {
                    let d = dist_pt_tri(p, &self.tris[idx as usize]);
                    if d < best {
                        best = d;
                        if best == 0.0 {
                            return 0.0;
                        }
                    }
                }
            }
        }
        best
    }
}

/// Quadrat-Distanz Punkt↔Segment in 2D.
fn seg_dist2(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let ap = [p[0] - a[0], p[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if len2 > 0.0 {
        ((ap[0] * ab[0] + ap[1] * ab[1]) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let foot = [a[0] + t * ab[0], a[1] + t * ab[1]];
    let dx = p[0] - foot[0];
    let dy = p[1] - foot[1];
    dx * dx + dy * dy
}

/// Abstand Punkt↔projiziertes Dreieck: 0 im Dreieck, sonst das Minimum über
/// die drei Kanten. Entartete (collineare) Dreiecke — vertikale Wände
/// kollabieren in der Projektion zur Strecke — messen nur über die Kanten,
/// damit Punkte auf ihrer Trägergeraden nicht als „innen“ gelten.
fn dist_pt_tri(p: [f64; 2], tri: &[[f64; 2]; 3]) -> f64 {
    let (a, b, c) = (tri[0], tri[1], tri[2]);
    let edges = || -> f64 {
        seg_dist2(p, a, b).min(seg_dist2(p, b, c)).min(seg_dist2(p, c, a)).sqrt()
    };
    // Entartet: nur Distanz zu der Strecke.
    let area2 = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
    if area2.abs() < 1e-12 {
        return edges();
    }
    // Innentest über die Dreiecksorientierung (Vorzeichen).
    let d1 = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
    let d2 = (c[0] - b[0]) * (p[1] - b[1]) - (c[1] - b[1]) * (p[0] - b[0]);
    let d3 = (a[0] - c[0]) * (p[1] - c[1]) - (a[1] - c[1]) * (p[0] - c[0]);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    if !(has_neg && has_pos) {
        return 0.0;
    }
    edges()
}

// ---------------------------------------------------------
// DXF-Export (R12: minimale, breit lesbare Entities)
// ---------------------------------------------------------

/// Konturen als geschlossene Polylinien in eine DXF-Datei (R12, Millimeter)
/// schreiben, Schicht „SCHATTENKONTUR“. `outer` ist die Außenkontur, `inners`
/// sind Innenkonturen (z. B. Bohrungen/Taschen, bereits mit Versatz nach
/// innen). Optional ein Beschriftungstext (z. B. „S=50mm“), mittig über der
/// Außenkontur.
pub fn write_dxf(
    path: &std::path::Path,
    outer: &[[f32; 2]],
    inners: &[Vec<[f32; 2]>],
    label: Option<&str>,
) -> Result<(), String> {
    if outer.len() < 3 {
        return Err("Kontur ist zu kurz für eine DXF-Polylinie".to_string());
    }
    let inner_pts: usize = inners.iter().map(|p| p.len()).sum();
    let mut out = String::with_capacity((outer.len() + inner_pts) * 64 + 384);
    let mut w = |code: &str, val: &str| {
        out.push_str(code);
        out.push('\n');
        out.push_str(val);
        out.push('\n');
    };
    let write_polyline = |w: &mut dyn FnMut(&str, &str), poly: &[[f32; 2]], layer: &str| {
        w("0", "POLYLINE");
        w("8", layer);
        w("66", "1"); // Vertex-Flag: Vertices folgen
        w("70", "1"); // geschlossen
        w("10", "0.0");
        w("20", "0.0");
        w("30", "0.0");
        for p in poly {
            w("0", "VERTEX");
            w("8", layer);
            w("10", &format!("{:.4}", p[0]));
            w("20", &format!("{:.4}", p[1]));
            w("30", "0.0");
        }
        w("0", "SEQEND");
        w("8", layer);
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
    write_polyline(&mut w, outer, "SCHATTENKONTUR");
    for inner in inners {
        if inner.len() >= 3 {
            write_polyline(&mut w, inner, "SCHATTENKONTUR");
        }
    }
    if let Some(text) = label.map(str::trim).filter(|s| !s.is_empty()) {
        let min_x = outer.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);
        let min_y = outer.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
        let max_x = outer.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max);
        let max_y = outer.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max);
        let span = (max_x - min_x).abs().max(1.0);
        let text_h = (span * 0.06).clamp(3.0, 12.0);
        let cx = (min_x + max_x) * 0.5;
        let cy = (min_y + max_y) * 0.5;
        // R12 TEXT: (10/20) ist der Einfügepunkt; mit Ausrichtung Mitte-Mitte
        // (72=1 horizontal, 73=2 vertikal) wird der Text um (cx,cy) zentriert.
        w("0", "TEXT");
        w("8", "SCHATTENKONTUR");
        w("10", &format!("{:.4}", cx));
        w("20", &format!("{:.4}", cy));
        w("30", "0.0");
        w("40", &format!("{:.4}", text_h));
        w("1", text);
        w("72", "1");
        w("11", &format!("{:.4}", cx));
        w("21", &format!("{:.4}", cy));
        w("31", "0.0");
        w("73", "2");
    }
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

    /// Abstand eines Punkts zum zentrierten Rechteck (x in [−w/2, +w/2],
    /// y in [−d/2, +d/2]) — die Kontur-Koordinaten sind auf die
    /// Bauteilmitte bezogen.
    fn rect_dist(p: [f32; 2], w: f32, d: f32) -> f64 {
        let (hw, hd) = (w * 0.5, d * 0.5);
        let edges = [
            ([-hw, -hd], [hw, -hd]),
            ([hw, -hd], [hw, hd]),
            ([hw, hd], [-hw, hd]),
            ([-hw, hd], [-hw, -hd]),
        ];
        edges
            .iter()
            .map(|(a, b)| seg_dist2([f64::from(p[0]), f64::from(p[1])], [f64::from(a[0]), f64::from(a[1])], [f64::from(b[0]), f64::from(b[1])]).sqrt())
            .fold(f64::INFINITY, f64::min)
    }

    #[test]
    fn box_10mm_expanded_by_3mm() {
        // Quader 20×10×5; um 3 mm: gerundetes Rechteck 26×16 mit
        // Eckenradius 3 → Fläche 26·16 − (4−π)·3² ≈ 408,27 mm²,
        // Umfang 2·(26+16) − 8·3 + 2π·3 ≈ 78,85 mm.
        let (positions, triangles) = box_mesh(20.0, 10.0, 5.0);
        let result = from_mesh(&positions, &triangles, 3.0, 0.0, 0.0).unwrap();
        assert!(
            (407.0..=409.5).contains(&result.area),
            "Fläche {} mm²",
            result.area
        );
        assert!(
            (78.0..=79.7).contains(&result.perimeter),
            "Umfang {} mm",
            result.perimeter
        );
        assert!(result.polygon.len() > 8, "zu wenige Punkte");
        // Kontur-Extrempunkte liegen exakt auf Bauteilmitte ± (Hälfte +
        // Abstand) — Ursprung ist die Bauteilmitte.
        let minx = result.polygon.iter().map(|p| p[0]).fold(f32::MAX, f32::min);
        let miny = result.polygon.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
        let maxx = result.polygon.iter().map(|p| p[0]).fold(f32::MIN, f32::max);
        let maxy = result.polygon.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
        assert!((minx + 13.0).abs() < 0.01, "minx={minx}");
        assert!((miny + 8.0).abs() < 0.01, "miny={miny}");
        assert!((maxx - 13.0).abs() < 0.01, "maxx={maxx}");
        assert!((maxy - 8.0).abs() < 0.01, "maxy={maxy}");
    }

    /// Ein verrauschter Kreis (wie von der Silhouette-Rasterung) wird als
    /// Kreis erkannt und als exakter, glatter Kreis mit reduziertem Radius
    /// (= Versatz nach innen) ausgegeben.
    #[test]
    fn inner_circle_autodetect_is_clean() {
        let mut poly: Vec<[f32; 2]> = Vec::new();
        let n = 200usize;
        for i in 0..n {
            let t = i as f32 / n as f32 * std::f32::consts::TAU;
            let jitter = 0.12_f32 * ((i * 13 % 5) as f32 - 2.0) / 2.0;
            poly.push([30.0 * t.cos() + jitter, 30.0 * t.sin() + jitter]);
        }
        let (c, r) = fit_circle(&poly).expect("Kreis sollte erkannt werden");
        assert!((r - 30.0).abs() < 0.3, "radius {r}");
        assert!(c[0].hypot(c[1]) < 0.2, "center {c:?}");

        // Wie in from_mesh: Kreis mit Versatz = Radius verkleinert.
        let pts = circle_points(c, r - 2.0);
        assert!(pts.len() >= 64, "zu wenige Segmente: {}", pts.len());
        let mut worst = 0.0_f64;
        for p in &pts {
            let d = ((f64::from(p[0]) - c[0]).powi(2) + (f64::from(p[1]) - c[1]).powi(2)).sqrt();
            worst = worst.max((d - (r - 2.0)).abs());
        }
        assert!(worst < 0.02, "Kreis nicht glatt: {worst}");

        // RDP: fast kollineare Punkte eines weichen Bogens reduzieren den
        // Zug deutlich, ohne die Form spürbar zu verändern.
        let soft: Vec<[f32; 2]> = (0..360)
            .map(|i| {
                let t = i as f32 / 360.0 * std::f32::consts::TAU;
                [20.0 * t.cos(), 15.0 * t.sin()]
            })
            .collect();
        let simp = simplify_rdp(&soft, 0.3);
        assert!(simp.len() < soft.len() / 4, "RDP zu wortkarg: {}", simp.len());
        for p in &simp {
            // Ellipse-Punkt-Nähe grob: Abstand zur (20,15)-Skalen-Ellipse
            let e = ((f64::from(p[0]) / 20.0).powi(2) + (f64::from(p[1]) / 15.0).powi(2)).sqrt();
            assert!((e - 1.0).abs() < 0.02, "Form driftet: {e}");
        }
    }

    /// Nut: Baukasten aus vier Quadern (Platte 100×40, Loch 20×30 mittig).
    /// Ein Klick auf die Nut-Wand (der Wandzug selbst umbaut nichts) wird
    /// über die Silhouette gelöst: der Innenzug des Lochs wird gefunden,
    /// geglättet und mit Versatz nach innen ausgegeben.
    #[test]
    fn slot_wall_pick_finds_silhouette_loop() {
        fn push_box(
            positions: &mut Vec<[f32; 3]>,
            triangles: &mut Vec<[u32; 3]>,
            o: [f32; 3],
            s: [f32; 3],
        ) {
            let corners = [
                [o[0], o[1], o[2]],
                [o[0] + s[0], o[1], o[2]],
                [o[0] + s[0], o[1] + s[1], o[2]],
                [o[0], o[1] + s[1], o[2]],
                [o[0], o[1], o[2] + s[2]],
                [o[0] + s[0], o[1], o[2] + s[2]],
                [o[0] + s[0], o[1] + s[1], o[2] + s[2]],
                [o[0], o[1] + s[1], o[2] + s[2]],
            ];
            let base = positions.len() as u32;
            positions.extend(corners);
            let quads: [[u32; 4]; 6] = [
                [0, 3, 2, 1],
                [4, 5, 6, 7],
                [0, 1, 5, 4],
                [3, 7, 6, 2],
                [0, 4, 7, 3],
                [1, 2, 6, 5],
            ];
            for q in &quads {
                triangles.push([
                    base + q[0],
                    base + q[1],
                    base + q[2],
                ]);
                triangles.push([
                    base + q[0],
                    base + q[2],
                    base + q[3],
                ]);
            }
        }
        let mut positions: Vec<[f32; 3]> = Vec::new();
        let mut triangles: Vec<[u32; 3]> = Vec::new();
        // Platte mit geschlossener Nut: x −10..10, y 5..35 bleibt frei.
        push_box(&mut positions, &mut triangles, [-50.0, 0.0, 0.0], [40.0, 40.0, 10.0]);
        push_box(&mut positions, &mut triangles, [10.0, 0.0, 0.0], [40.0, 40.0, 10.0]);
        push_box(&mut positions, &mut triangles, [-10.0, 0.0, 0.0], [20.0, 5.0, 10.0]);
        push_box(&mut positions, &mut triangles, [-10.0, 35.0, 0.0], [20.0, 5.0, 10.0]);

        let (loops, origin, res) = silhouette_inner_loops(&positions, &triangles, 0.0).unwrap();
        assert!(res.is_finite() && res > 0.0);
        let hole = loops
            .iter()
            .find(|l| {
                let mut bx = [f32::MAX, f32::MIN];
                let mut by = [f32::MAX, f32::MIN];
                for p in *l {
                    bx[0] = bx[0].min(p[0]);
                    bx[1] = bx[1].max(p[0]);
                    by[0] = by[0].min(p[1]);
                    by[1] = by[1].max(p[1]);
                }
                (bx[1] - bx[0]) > 15.0 && (by[1] - by[0]) > 25.0
            })
            .expect("Nut-Loch sollte als Innenzug erscheinen");
        // Klickpunkt „auf der Wand": knapp außerhalb des Lochs, in der Mitte
        // der unteren Nutwand (Welt y=5 → lokal −15).
        let click = [0.0, -16.0];
        let d = point_poly_dist(click, hole);
        assert!(d < 2.0, "Wall-Klick sollte nahe am Loch sein: {d}");
        // Behauptet: das Loch ist so nah wie möglich (anderer Zug wäre weit weg).
        for l in &loops {
            if !std::ptr::eq(l, hole) {
                assert!(point_poly_dist(click, l) > d);
            }
        }
        // Von allen loops ist das Loch das nächstgelegene. Prepare + Versatz
        // glättet die Kontur und verkleinert sie.
        let mut rbx = [f32::MAX as f64, f32::MIN as f64];
        let mut rby = [f32::MAX as f64, f32::MIN as f64];
        for p in hole {
            rbx[0] = rbx[0].min(f64::from(p[0]));
            rbx[1] = rbx[1].max(f64::from(p[0]));
            rby[0] = rby[0].min(f64::from(p[1]));
            rby[1] = rby[1].max(f64::from(p[1]));
        }
        let clean = prepare_inner(hole, 2.0, res).unwrap();
        let mut bx = [f32::MAX as f64, f32::MIN as f64];
        let mut by = [f32::MAX as f64, f32::MIN as f64];
        for p in &clean {
            bx[0] = bx[0].min(f64::from(p[0]));
            bx[1] = bx[1].max(f64::from(p[0]));
            by[0] = by[0].min(f64::from(p[1]));
            by[1] = by[1].max(f64::from(p[1]));
        }
        assert!(
            ((bx[1] - bx[0]) - 16.0).abs() < 0.8 && ((by[1] - by[0]) - 26.0).abs() < 0.8,
            "Nutkontur: {bx:?} × {by:?}"
        );
        assert!(!clean.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()));
        // origin bleibt Bauteilmitte in X/Y: hier (0, 20).
        assert!((origin[0].abs()) < 1e-3 && (origin[1] - 20.0).abs() < 1e-3);
    }

    /// Exakt konturparallel: jeder Konturpunkt ist genau „expand“ vom
    /// Bauteilrechteck entfernt (gerade Stücke wie Kreisbögen).
    #[test]
    fn contour_is_exactly_parallel() {        let (positions, triangles) = box_mesh(20.0, 10.0, 5.0);
        let result = from_mesh(&positions, &triangles, 3.0, 0.0, 0.0).unwrap();
        let mut worst = 0.0_f64;
        for p in &result.polygon {
            let d = rect_dist(*p, 20.0, 10.0);
            let err = (d - f64::from(result.expand)).abs();
            worst = worst.max(err);
        }
        assert!(
            worst < 0.01,
            "Kontur liegt nicht konturparallel: größter Fehler {worst} mm"
        );
    }

    /// Zulage 0: Die Kontur liegt glatt im Mikro-Abstand vor der Silhouette
    /// (kein Treppen-Rastern am Feldplateau, kein Punkt inside).
    #[test]
    fn contour_at_zero_allowance_is_smooth() {
        let (positions, triangles) = box_mesh(20.0, 10.0, 5.0);
        let result = from_mesh(&positions, &triangles, 0.0, 0.0, 0.0).unwrap();
        // Mikro-Abstand wie in from_mesh: res · 0,02.
        let eps = f64::from(result.res) * 0.02;
        let mut dmin = f64::MAX;
        let mut dmax = 0.0_f64;
        for p in &result.polygon {
            let d = rect_dist(*p, 20.0, 10.0);
            dmin = dmin.min(d);
            dmax = dmax.max(d);
        }
        assert!(
            dmin >= eps * 0.4,
            "Kontur rastert auf dem Plateau (Treppen): kleinster Abstand {dmin} mm"
        );
        assert!(
            dmax <= eps * 3.0 + 1e-4,
            "Kontur zu weit weg von der Silhouette: größter Abstand {dmax} mm"
        );
        assert!(result.area > 200.0, "Fläche {} mm² unter Rechteckfläche", result.area);
    }

    #[test]
    fn dxf_contains_one_polyline_with_all_vertices() {
        let (positions, triangles) = box_mesh(10.0, 10.0, 2.0);
        let result = from_mesh(&positions, &triangles, 1.0, 0.0, 0.0).unwrap();
        let path = std::env::temp_dir().join("rohteil_shadow_test.dxf");
        write_dxf(&path, &result.polygon, &[], Some("S=5mm")).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(text.matches("POLYLINE").count(), 1);
        assert_eq!(text.matches("SEQEND").count(), 1);
        assert_eq!(text.matches("VERTEX").count(), result.polygon.len());
        assert!(text.contains("S=5mm"));
        assert!(text.contains("$INSUNITS"));
    }
}


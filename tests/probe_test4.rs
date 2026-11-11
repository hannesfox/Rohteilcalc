//! Probe: Warum wird test4.SteP anders/schlechter ausgerichtet als andere
//! Teile? Lädt die Datei, tesselliert sie und gibt Roh-Ausdehnung, Ergebnis
//! der verfeinerten Ausrichtung und die Kovarianz-Hauptachsen aus.

#[path = "../src/step.rs"]
mod step;
#[path = "../src/stock.rs"]
mod stock;
#[path = "../src/mesh.rs"]
mod mesh;
#[path = "../src/mesher.rs"]
mod mesher;

use glam::Vec3;
use std::path::Path;

#[test]
fn probe_test4_alignment() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test4.SteP");
    let doc = step::StepDocument::from_path(&path).unwrap();
    println!("tessellieren …");
    let part = step::tessellate(&doc, step::Quality::Medium).unwrap();
    println!("bodies: {}", part.bodies.len());
    for w in &part.warnings {
        println!("warn: {w}");
    }

    // Alle Körper zusammen (wie orient_and_align_bodies).
    let mut all: Vec<Vec3> = Vec::new();
    let mut raw_min = Vec3::splat(f32::MAX);
    let mut raw_max = Vec3::splat(f32::MIN);
    let mut tris: usize = 0;
    for b in &part.bodies {
        all.extend(b.mesh.vertices.iter().copied());
        tris += b.mesh.triangles.len();
    }
    for v in &all {
        raw_min = raw_min.min(*v);
        raw_max = raw_max.max(*v);
    }
    println!("vertices: {} · triangles: {tris}", all.len());
    let raw_ext = raw_max - raw_min;
    println!("RAW extent: x={:.2} y={:.2} z={:.2}", raw_ext.x, raw_ext.y, raw_ext.z);
    println!("RAW min {raw_min:?} max {raw_max:?}");

    // Software-Render (Punkt-Plot mit Shading) → PPM → PNG, damit man das
    // Teil wirklich sieht. Vier Ortho-Ansichten.
    render_ppm(&part.bodies, &all, raw_min, raw_max);

    // ASCII-Projektionen des Roh-Teils (Form erkennen).
    for (name, fx, fy) in [("XY (top)", 0usize, 1usize), ("XZ", 0, 2), ("YZ", 1, 2)] {
        println!("--- Projektion {name} ---");
        let mut grid = vec![vec![0usize; 60]; 24];
        let (mut a0, mut a1) = (f32::MAX, f32::MIN);
        let (mut b0, mut b1) = (f32::MAX, f32::MIN);
        for v in &all {
            a0 = a0.min(v[fx]);
            a1 = a1.max(v[fx]);
            b0 = b0.min(v[fy]);
            b1 = b1.max(v[fy]);
        }
        for v in &all {
            let i = (((v[fx] - a0) / (a1 - a0)) * 59.0) as usize;
            let j = (((v[fy] - b0) / (b1 - b0)) * 23.0) as usize;
            grid[23 - j][i] += 1;
        }
        for row in &grid {
            let line: String = row
                .iter()
                .map(|&c| match c {
                    0 => '.',
                    1..=2 => '·',
                    3..=6 => '+',
                    _ => '#',
                })
                .collect();
            println!("{line}");
        }
    }

    // Verfeinerte Ausrichtung (der Pfad, den die App benutzt).
    let Some((align, size)) = stock::alignment_for_vertices_refined(&all) else {
        println!("keine Ausrichtung möglich");
        return;
    };
    println!("ALIGNED size: x={:.2} y={:.2} z={:.2}", size.x, size.y, size.z);

    // Ausgerichtete Ausdehnung nochmal direkt messen (Vertrauenscheck).
    let mut amin = Vec3::splat(f32::MAX);
    let mut amax = Vec3::splat(f32::MIN);
    for v in &all {
        let p = align.rot * (*v - align.centroid);
        amin = amin.min(p);
        amax = amax.max(p);
    }
    let aext = amax - amin;
    println!(
        "ALIGNED measured: x={:.2} y={:.2} z={:.2} (Volumen {:.0} mm³)",
        aext.x,
        aext.y,
        aext.z,
        (aext.x * aext.y * aext.z) as f64
    );

    // Kovarianz + Hauptachsen (was die Kandidaten sind).
    let n = all.len() as f32;
    let centroid = all.iter().fold(Vec3::ZERO, |a, &v| a + v) / n;
    let mut cov = glam::Mat3::ZERO;
    for v in &all {
        let p = *v - centroid;
        cov.x_axis += Vec3::new(p.x * p.x, p.x * p.y, p.x * p.z);
        cov.y_axis += Vec3::new(p.y * p.x, p.y * p.y, p.y * p.z);
        cov.z_axis += Vec3::new(p.z * p.x, p.z * p.y, p.z * p.z);
    }
    cov /= n;
    let ev = stock::eigen_symmetric(cov);
    println!("cov diag: {:.2} {:.2} {:.2}", cov.x_axis.x, cov.y_axis.y, cov.z_axis.z);
    for (i, e) in ev.iter().enumerate() {
        println!("eigenvec {i}: {e:?}");
    }

    // Vergleich: globales Minimum der Box-Volumen? Grobe Rastersuche über
    // Euler-Rotationen (Yaw um Z, dann Roll um X, Pitch um Y), 10°-Schritte,
    // um zu prüfen, ob ein Kandidat der Ausrichtung die Fläche verpasst.
    println!("Rastersuche 10° …");
    let rot_from = |yaw: f32, pitch: f32, roll: f32| {
        glam::Mat3::from_rotation_z(yaw)
            * glam::Mat3::from_rotation_x(pitch)
            * glam::Mat3::from_rotation_y(roll)
    };
    let mut best_vol = f32::INFINITY;
    let mut best_ext = Vec3::ZERO;
    let mut best_yaw = 0.0_f32;
    let mut best_pitch = 0.0_f32;
    let mut best_roll = 0.0_f32;
    let d = 10.0_f32.to_radians();
    let mut k = 0usize;
    for yaw_i in 0..36 {
        for pitch_i in 0..19 {
            for roll_i in 0..19 {
                let yaw = yaw_i as f32 * d;
                let pitch = pitch_i as f32 * d - std::f32::consts::FRAC_PI_2;
                let roll = roll_i as f32 * d - std::f32::consts::FRAC_PI_2;
                let r = rot_from(yaw, pitch, roll);
                let mut mn = Vec3::splat(f32::MAX);
                let mut mx = Vec3::splat(f32::MIN);
                for v in &all {
                    let p = r * *v;
                    mn = mn.min(p);
                    mx = mx.max(p);
                }
                let e = mx - mn;
                let vol = e.x * e.y * e.z;
                k += 1;
                if vol < best_vol {
                    best_vol = vol;
                    best_ext = e;
                    best_yaw = yaw;
                    best_pitch = pitch;
                    best_roll = roll;
                }
            }
        }
    }
    println!("Grid-Rotationen getestet: {k}");
    println!(
        "GRID best: x={:.2} y={:.2} z={:.2} (Vol {:.0})",
        best_ext.x, best_ext.y, best_ext.z, best_vol as f64
    );
    // Bestes Grid weiter verfeinern (±10° in 2°-Schritten) — die Winkel des
    // besten Grid-Treffers werden direkt mitgenommen.
    let mut best_vol2 = best_vol;
    let mut best_ext2 = best_ext;
    let d2 = 2.0_f32.to_radians();
    for dy in -5..=5 {
        for dp in -5..=5 {
            for dr in -5..=5 {
                let r = rot_from(
                    best_yaw + dy as f32 * d2,
                    best_pitch + dp as f32 * d2,
                    best_roll + dr as f32 * d2,
                );
                let mut mn = Vec3::splat(f32::MAX);
                let mut mx = Vec3::splat(f32::MIN);
                for v in &all {
                    let p = r * *v;
                    mn = mn.min(p);
                    mx = mx.max(p);
                }
                let e = mx - mn;
                let vol = e.x * e.y * e.z;
                if vol < best_vol2 {
                    best_vol2 = vol;
                    best_ext2 = e;
                }
            }
        }
    }
    println!(
        "GRID refined: x={:.2} y={:.2} z={:.2} (Vol {:.0})",
        best_ext2.x, best_ext2.y, best_ext2.z, best_vol2 as f64
    );
    println!(
        "ALIGN vs GRID: {:.1}× mehr Volumen",
        best_vol2 / (aext.x * aext.y * aext.z)
    );

    // Schaft-Achse messen: PCA der Punkte im hinteren Viertel (Schaftspitze)
    // im AUSGERICHTETEN Raum → Winkel zum X-Achsen-Rahmen.
    println!("Schaft-Check …");
    let mut projected: Vec<(f32, Vec3)> = all
        .iter()
        .map(|v| {
            let p = align.rot * (*v - align.centroid);
            (p.x, p)
        })
        .collect();
    projected.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let tail: Vec<Vec3> = projected
        [projected.len() * 3 / 4..]
        .iter()
        .map(|&(_, p)| p)
        .collect();
    let tc = tail.iter().fold(Vec3::ZERO, |a, &v| a + v) / tail.len() as f32;
    let mut tcov = glam::Mat3::ZERO;
    for v in &tail {
        let p = *v - tc;
        tcov.x_axis += Vec3::new(p.x * p.x, p.x * p.y, p.x * p.z);
        tcov.y_axis += Vec3::new(p.y * p.x, p.y * p.y, p.y * p.z);
        tcov.z_axis += Vec3::new(p.z * p.x, p.z * p.y, p.z * p.z);
    }
    tcov /= tail.len() as f32;
    let tev = stock::eigen_symmetric(tcov);
    let shaft = tev[0].normalize();
    let ang_x = shaft.x.clamp(-1.0, 1.0).acos().to_degrees();
    println!("Schaft-Richtung (aligned): {shaft:?} · Winkel zur X-Achse: {ang_x:.1}°");

    // Kandidat „Schaft exakt auf X“: Schaft-Richtung zurück in den Welt-
    // Raum transformieren (p = rot · v ⇒ Welt = rotᵀ · ausgerichtet),
    // Rahmen aufspannen und per refine polieren.
    let shaft_raw = align.rot.transpose() * shaft;
    println!("Schaft im Roh-Modell: {shaft_raw:?}");

    // Verschiedene Kandidaten-Orientierungen: Volumen + Bild.
    // (a) Roh-Lage (Datei-Orientierung)
    // (b) Arm horizontal, Biegeebene vertikal: X=Arm, Z=Arm×Y... gebaut aus
    //     Arm-Richtung und Biege-Normale (≈ Y im Roh-Modell, Profil in XZ).
    // (c) aktuelles aligniertes Ergebnis (render über align.rot).
    let arm = shaft_raw.normalize();
    let bend_n = Vec3::Y - arm * arm.dot(Vec3::Y);
    let bend_n = bend_n.normalize_or_zero();
    let arm_frame = glam::Mat3::from_cols(arm, bend_n.cross(arm), bend_n);
    for (name, frame) in [
        ("raw", glam::Mat3::IDENTITY),
        ("arm", arm_frame),
        ("aligned", align.rot),
    ] {
        // Welt-Rahmen-Kandidaten: Spalten = Achsen ⇒ p = frameᵀ·v.
        // align.rot mappt Welt→ausgerichtet (Zeilen = Achsen) ⇒ p = frame·v.
        let is_align_rot = name == "aligned";
        let rt = if is_align_rot { frame } else { frame.transpose() };
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for v in &all {
            let p = rt * *v;
            mn = mn.min(p);
            mx = mx.max(p);
        }
        let e = mx - mn;
        println!(
            "Frame {name:8}: x={:6.2} y={:6.2} z={:6.2}  Vol {:.0}",
            e.x, e.y, e.z, (e.x * e.y * e.z) as f64
        );
        let vis = if is_align_rot { frame.transpose() } else { frame };
        render_frame_ppm(&part.bodies, vis, name);
    }

    let helper = if shaft_raw.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let sy = shaft_raw.cross(helper).normalize();
    let sz = shaft_raw.cross(sy);
    let frame = glam::Mat3::from_cols(shaft_raw, sy, sz);
    let (frame2, _vol2) = stock::refine_frame(&all, frame);

    // „Grundplatte unten" (= Roh-Z als Normalenrichtung) + bester Dreiwinkel
    // um die Plattennormale: Footprint × Höhe minimieren.
    println!("Grundplatte-unten-Jig …");
    let mut best_plate_vol = f32::MAX;
    let mut best_plate_frame = glam::Mat3::IDENTITY;
    for i in 0..90 {
        let yaw = i as f32;
        let r = glam::Mat3::from_rotation_z(yaw.to_radians());
        let rt = r.transpose();
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for v in &all {
            let p = rt * *v;
            mn = mn.min(p);
            mx = mx.max(p);
        }
        let e = mx - mn;
        let vol = e.x * e.y * e.z;
        if vol < best_plate_vol {
            best_plate_vol = vol;
            best_plate_frame = r;
            println!("  yaw={yaw:2}° → {}×{}×{} = {:.0}", e.x, e.y, e.z, vol as f64);
        }
    }
    render_frame_ppm(&part.bodies, best_plate_frame, "plate");
    println!(
        "Grundplatte-unten best: {:.0} mm³",
        best_plate_vol as f64
    );

    // Rotationsachse? Für jeden Kandidaten-Achsrichtung prüfen, wie stark
    // die quer dazu gemessenen Boxmaße schwanken, wenn das Teil um die
    // Achse gedreht wird (kleine Schwankung = Rotations-/Symmetrieachse).
    println!("Rotations-Check …");
    let axes: [(&str, Vec3); 6] = [
        ("Welt X", Vec3::X),
        ("Welt Y", Vec3::Y),
        ("Welt Z", Vec3::Z),
        ("Eigen 0", ev[0]),
        ("Eigen 1", ev[1]),
        ("Eigen 2", ev[2]),
    ];
    for (name, a) in axes {
        let a = a.normalize_or_zero();
        // Orthonormale Querpaare b, c.
        let helper = if a.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
        let b = a.cross(helper).normalize();
        let c = a.cross(b);
        let mut dmin_b = f32::MAX;
        let mut dmax_b = f32::MIN;
        let mut dmin_c = f32::MAX;
        let mut dmax_c = f32::MIN;
        let mut vol_min = f32::MAX;
        let mut vol_max = f32::MIN;
        let steps = 24;
        for s in 0..steps {
            let t = s as f32 / steps as f32 * std::f32::consts::TAU;
            let rb = b * t.cos() + c * t.sin();
            let rc = -b * t.sin() + c * t.cos();
            let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for v in &all {
                let p = Vec3::new(v.dot(a), v.dot(rb), v.dot(rc));
                mn = mn.min(p);
                mx = mx.max(p);
            }
            let e = mx - mn;
            dmin_b = dmin_b.min(e.y);
            dmax_b = dmax_b.max(e.y);
            dmin_c = dmin_c.min(e.z);
            dmax_c = dmax_c.max(e.z);
            let vol = e.x * e.y * e.z;
            vol_min = vol_min.min(vol);
            vol_max = vol_max.max(vol);
        }
        println!(
            "{name:8} Länge {:.2} · quer b {:.2}..{:.2} · quer c {:.2}..{:.2} · Vol {:.0}..{:.0}",
            {
                let (mut mn, mut mx) = (f32::MAX, f32::MIN);
                for v in &all {
                    mn = mn.min(v.dot(a));
                    mx = mx.max(v.dot(a));
                }
                mx - mn
            },
            dmin_b, dmax_b, dmin_c, dmax_c, vol_min, vol_max
        );
    }
}

/// Software-Renderer: Orthoprojektion, Tiefe via Z-Buffer,
/// Flach-Shading über die Normale. Schreibt 4 PPMs nach target/.
fn render_ppm(
    bodies: &[step::Body],
    all: &[Vec3],
    _rmin: Vec3,
    _rmax: Vec3,
) {
    let views: [(&str, Vec3, Vec3); 4] = [
        ("top", Vec3::Z, Vec3::Y),          // Blick −Z, oben +Y
        ("front", Vec3::Y, Vec3::Z),        // Blick +Y (front = +Y? siehe App)
        ("left", Vec3::X, Vec3::Z),
        ("iso", Vec3::new(1.0, -1.0, 1.0), Vec3::Z),
    ];
    let w = 480usize;
    let h = 360usize;
    for (name, view, up) in views {
        let view = view.normalize();
        let right = up.cross(view).normalize();
        let up2 = view.cross(right).normalize();
        // Alle Dreiecke einsammeln.
        let mut tris: Vec<[Vec3; 3]> = Vec::new();
        for b in bodies {
            for t in &b.mesh.triangles {
                tris.push([
                    b.mesh.vertices[t[0] as usize],
                    b.mesh.vertices[t[1] as usize],
                    b.mesh.vertices[t[2] as usize],
                ]);
            }
        }
        // Screen-Bounds.
        let project = |p: Vec3| -> (f32, f32, f32) {
            (p.dot(right), p.dot(up2), p.dot(view))
        };
        let mut smin = (f32::MAX, f32::MAX);
        let mut smax = (f32::MIN, f32::MIN);
        let mut proj: Vec<[(f32, f32, f32); 3]> = Vec::with_capacity(tris.len());
        for t in &tris {
            let pt = [project(t[0]), project(t[1]), project(t[2])];
            for &(a, b, _) in &pt {
                smin.0 = smin.0.min(a);
                smin.1 = smin.1.min(b);
                smax.0 = smax.0.max(a);
                smax.1 = smax.1.max(b);
            }
            proj.push(pt);
        }
        let scale = ((w as f32 - 20.0) / (smax.0 - smin.0))
            .min((h as f32 - 20.0) / (smax.1 - smin.1));
        let to_px = |a: f32, b: f32| -> (f32, f32) {
            (
                (a - smin.0) * scale + 10.0,
                h as f32 - ((b - smin.1) * scale + 10.0),
            )
        };
        let mut zbuf = vec![f32::INFINITY; w * h];
        let mut img = vec![0u8; w * h * 3];
        for (tri, pt) in tris.iter().zip(&proj) {
            let n = (tri[1] - tri[0]).cross(tri[2] - tri[0]);
            let Some(n) = n.try_normalize() else { continue };
            // Normale relativ zur Blickrichtung (view zeigt VOM Betrachter
            // ins Bild hinein): Helligkeit aus |dot|.
            let lam = n.dot(view).abs() * 0.7 + 0.3;
            let shade = (lam * 230.0) as u8;
            // Blick von +view-Seite: sichtbar, wenn Normale entgegen view zeigt.
            let front = n.dot(view) < 0.0;
            let px: Vec<(f32, f32)> = pt
                .iter()
                .map(|&(a, b, _)| to_px(a, b))
                .collect();
            let zs: [f32; 3] = [pt[0].2, pt[1].2, pt[2].2];
            let minx = px.iter().fold(f32::MAX, |m, p| m.min(p.0)).max(0.0) as usize;
            let maxx = (px.iter().fold(f32::MIN, |m, p| m.max(p.0)) as usize + 1).min(w);
            let miny = px.iter().fold(f32::MAX, |m, p| m.min(p.1)).max(0.0) as usize;
            let maxy = (px.iter().fold(f32::MIN, |m, p| m.max(p.1)) as usize + 1).min(h);
            for y in miny..maxy {
                for x in minx..maxx {
                    let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                    // Baryzentrischer Einschließungstest.
                    let mut bary = [0.0f32; 3];
                    let mut inside = true;
                    for k in 0..3 {
                        let a = px[k];
                        let b = px[(k + 1) % 3];
                        let c = px[(k + 2) % 3];
                        let d1 = (b.0 - a.0) * (fy - a.1) - (b.1 - a.1) * (fx - a.0);
                        let d2 = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
                        if d1 * d2 < 0.0 {
                            inside = false;
                            break;
                        }
                        bary[k] = d1;
                    }
                    if !inside || !front {
                        continue;
                    }
                    let sum = bary[0] + bary[1] + bary[2];
                    let z = if sum.abs() > 1e-9 {
                        (bary[0] * zs[0] + bary[1] * zs[1] + bary[2] * zs[2]) / sum
                    } else {
                        zs[0]
                    };
                    let idx = y * w + x;
                    if z < zbuf[idx] {
                        zbuf[idx] = z;
                        img[idx * 3] = shade;
                        img[idx * 3 + 1] = shade;
                        img[idx * 3 + 2] = shade;
                    }
                }
            }
        }
        // Hintergrund hell.
        for i in 0..w * h {
            if zbuf[i] == f32::INFINITY {
                img[i * 3] = 255;
                img[i * 3 + 1] = 255;
                img[i * 3 + 2] = 255;
            }
        }
        let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
        ppm.extend_from_slice(&img);
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("test4_{name}.ppm"));
        std::fs::write(&out, &ppm).unwrap();
        println!("PPM geschrieben: {}", out.display());
    }
    let _ = all;
}

/// Rendert das Teil in einem gegebenen Welt-Rahmen (Iso-Ansicht) als PNG.
fn render_frame_ppm(bodies: &[step::Body], frame: glam::Mat3, name: &str) {
    let view_dir = frame * Vec3::new(1.0, -1.0, 1.0).normalize();
    let up_dir = frame * Vec3::Z;
    let w = 480usize;
    let h = 360usize;
    let right = up_dir.cross(view_dir).normalize();
    let up2 = view_dir.cross(right).normalize();
    let mut tris: Vec<[Vec3; 3]> = Vec::new();
    for b in bodies {
        for t in &b.mesh.triangles {
            tris.push([
                b.mesh.vertices[t[0] as usize],
                b.mesh.vertices[t[1] as usize],
                b.mesh.vertices[t[2] as usize],
            ]);
        }
    }
    let project = |p: Vec3| -> (f32, f32, f32) {
        (p.dot(right), p.dot(up2), p.dot(view_dir))
    };
    let mut smin = (f32::MAX, f32::MAX);
    let mut smax = (f32::MIN, f32::MIN);
    let mut proj: Vec<[(f32, f32, f32); 3]> = Vec::with_capacity(tris.len());
    for t in &tris {
        let pt = [project(t[0]), project(t[1]), project(t[2])];
        for &(a, b, _) in &pt {
            smin.0 = smin.0.min(a);
            smin.1 = smin.1.min(b);
            smax.0 = smax.0.max(a);
            smax.1 = smax.1.max(b);
        }
        proj.push(pt);
    }
    let scale = ((w as f32 - 20.0) / (smax.0 - smin.0))
        .min((h as f32 - 20.0) / (smax.1 - smin.1));
    let to_px = |a: f32, b: f32| -> (f32, f32) {
        (
            (a - smin.0) * scale + 10.0,
            h as f32 - ((b - smin.1) * scale + 10.0),
        )
    };
    let mut zbuf = vec![f32::INFINITY; w * h];
    let mut img = vec![0u8; w * h * 3];
    for (tri, pt) in tris.iter().zip(&proj) {
        let n = (tri[1] - tri[0]).cross(tri[2] - tri[0]);
        let Some(n) = n.try_normalize() else { continue };
        let lam = n.dot(view_dir).abs() * 0.7 + 0.3;
        let shade = (lam * 230.0) as u8;
        let front = n.dot(view_dir) < 0.0;
        let px: Vec<(f32, f32)> = pt
            .iter()
            .map(|&(a, b, _)| to_px(a, b))
            .collect();
        let zs: [f32; 3] = [pt[0].2, pt[1].2, pt[2].2];
        let minx = (px.iter().fold(f32::MAX, |m, p| m.min(p.0)).max(0.0)) as usize;
        let maxx = ((px.iter().fold(f32::MIN, |m, p| m.max(p.0)) as usize + 1).min(w));
        let miny = (px.iter().fold(f32::MAX, |m, p| m.min(p.1)).max(0.0)) as usize;
        let maxy = ((px.iter().fold(f32::MIN, |m, p| m.max(p.1)) as usize + 1).min(h));
        for y in miny..maxy {
            for x in minx..maxx {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let mut bary = [0.0f32; 3];
                let mut inside = true;
                for k in 0..3 {
                    let a = px[k];
                    let b = px[(k + 1) % 3];
                    let c = px[(k + 2) % 3];
                    let d1 = (b.0 - a.0) * (fy - a.1) - (b.1 - a.1) * (fx - a.0);
                    let d2 = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
                    if d1 * d2 < 0.0 {
                        inside = false;
                        break;
                    }
                    bary[k] = d1;
                }
                if !inside || !front {
                    continue;
                }
                let sum = bary[0] + bary[1] + bary[2];
                let z = if sum.abs() > 1e-9 {
                    (bary[0] * zs[0] + bary[1] * zs[1] + bary[2] * zs[2]) / sum
                } else {
                    zs[0]
                };
                let idx = y * w + x;
                if z < zbuf[idx] {
                    zbuf[idx] = z;
                    img[idx * 3] = shade;
                    img[idx * 3 + 1] = shade;
                    img[idx * 3 + 2] = shade;
                }
            }
        }
    }
    for i in 0..w * h {
        if zbuf[i] == f32::INFINITY {
            img[i * 3] = 255;
            img[i * 3 + 1] = 255;
            img[i * 3 + 2] = 255;
        }
    }
    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    ppm.extend_from_slice(&img);
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("test4_frame_{name}.ppm"));
    std::fs::write(&out, &ppm).unwrap();
    println!("PPM geschrieben: {}", out.display());
}

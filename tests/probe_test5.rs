//! Probe: Ausrichtung von test5.step (ebene Platte). Zeigt Roh-Ausdehnung,
//! Ergebnis der verfeinerten Ausrichtung, 2D-Footprint-OBB und rendert
//! Orientierungen als PNG (target/).

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
fn probe_test5_alignment() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("test5.step");
    let doc = step::StepDocument::from_path(&path).unwrap();
    let part = step::tessellate(&doc, step::Quality::Medium).unwrap();
    println!("bodies: {}", part.bodies.len());

    let mut all: Vec<Vec3> = Vec::new();
    for b in &part.bodies {
        all.extend(b.mesh.vertices.iter().copied());
    }
    let mut rmin = Vec3::splat(f32::MAX);
    let mut rmax = Vec3::splat(f32::MIN);
    for v in &all {
        rmin = rmin.min(*v);
        rmax = rmax.max(*v);
    }
    println!("verts: {} · RAW extent: {:?}", all.len(), rmax - rmin);

    let Some((align, size)) = stock::alignment_for_vertices_refined(&all) else {
        println!("keine Ausrichtung");
        return;
    };
    println!("ALIGNED size: x={:.2} y={:.2} z={:.2}", size.x, size.y, size.z);

    // Ist die Platte im aligned-Frame flach (Z klein)? Extents gemessen.
    // Kandidat: 2D-Footprint-OBB über konvexe Hülle (Rotating Calipers:
    // Kante der Hülle flush mit Rechteckkante) — „gerade Kanten auf X/Y".
    println!("Footprint-OBB …");
    let rt = align.rot;
    let mut pts2: Vec<(f32, f32)> = Vec::new();
    let mut zmin = f32::MAX;
    let mut zmax = f32::MIN;
    for v in &all {
        let p = rt * (*v - align.centroid);
        pts2.push((p.x, p.y));
        zmin = zmin.min(p.z);
        zmax = zmax.max(p.z);
    }
    println!("  z-extent im aligned frame: {:.2}", zmax - zmin);
    let hull = convex_hull(&mut pts2);
    println!("  hull points: {}", hull.len());
    let (best_area, best_angle) = min_area_rectangle(&hull);
    println!("  min-area footprint: {:.1} mm² · Kantenwinkel zu X: {:.1}°",
        best_area, best_angle.to_degrees());

    // Und der aligned Frame selbst: Footprint-Fläche + Rechteckwinkel.
    let (a_cur, ang_cur) = footprint_area_and_angle(&hull);
    println!("  aktueller aligned Footprint: {:.1} mm² · Winkel {:.1}°", a_cur, ang_cur.to_degrees());

    render(&part.bodies, "raw", glam::Mat3::IDENTITY);
    let vis = rt.transpose();
    render(&part.bodies, "aligned", vis);
    // Ideal: um die Platten-Normale (Z im aligned Frame) um best_angle drehen.
    let yaw = glam::Mat3::from_rotation_z(best_angle);
    let ideal_world = rt.transpose() * yaw * rt; // Drehung um aligned-Z, zurück in Welt
    render(&part.bodies, "footprint", ideal_world);
    // Volumen der Ideal-Orientierung.
    let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for v in &all {
        let p = ideal_world.transpose() * *v;
        mn = mn.min(p);
        mx = mx.max(p);
    }
    let e = mx - mn;
    println!(
        "FOOTPRINT-OBB Frame: x={:.2} y={:.2} z={:.2} (Vol {:.0})",
        e.x, e.y, e.z, (e.x * e.y * e.z) as f64
    );
}

fn footprint_area_and_angle(hull: &[(f32, f32)]) -> (f32, f32) {
    let mut a = 0.0f32;
    for i in 0..hull.len() {
        let j = (i + 1) % hull.len();
        a += hull[i].0 * hull[j].1 - hull[j].0 * hull[i].1;
    }
    // Dominante Kantenrichtung: längste Hüllkante.
    let mut best_len = 0.0f32;
    let mut best_ang = 0.0f32;
    for i in 0..hull.len() {
        let j = (i + 1) % hull.len();
        let dx = hull[j].0 - hull[i].0;
        let dy = hull[j].1 - hull[i].1;
        let l = (dx * dx + dy * dy).sqrt();
        if l > best_len {
            best_len = l;
            best_ang = dy.atan2(dx);
        }
    }
    (a * 0.5, best_ang)
}

fn convex_hull(pts: &mut [(f32, f32)]) -> Vec<(f32, f32)> {
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.partial_cmp(&b.1).unwrap()));
    let mut h: Vec<(f32, f32)> = Vec::new();
    for &p in pts.iter() {
        while h.len() >= 2 && cross(&h[h.len() - 2], &h[h.len() - 1], &p) <= 0.0 {
            h.pop();
        }
        h.push(p);
    }
    let lower = h.len() + 1;
    for &p in pts.iter().rev() {
        while h.len() >= lower && cross(&h[h.len() - 2], &h[h.len() - 1], &p) <= 0.0 {
            h.pop();
        }
        h.push(p);
    }
    h.pop();
    h
}

fn cross(o: &(f32, f32), a: &(f32, f32), b: &(f32, f32)) -> f32 {
    (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0)
}

/// Min-Flächen-Rechteck über die Hülle: für jede Hüllkante das Rechteck
/// ausrichten (Rotating-Calipers-Prinzip) — Flächeninhalt + Winkel der
/// flush-Kante zur X-Achse.
fn min_area_rectangle(hull: &[(f32, f32)]) -> (f32, f32) {
    let mut best = (f32::MAX, 0.0f32);
    for i in 0..hull.len() {
        let j = (i + 1) % hull.len();
        let dx = hull[j].0 - hull[i].0;
        let dy = hull[j].1 - hull[i].1;
        let l = (dx * dx + dy * dy).sqrt();
        if l < 1e-6 {
            continue;
        }
        let (ux, uy) = (dx / l, dy / l);
        let (mut umin, mut umax, mut vmin, mut vmax) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for &(x, y) in hull {
            let u = x * ux + y * uy;
            let v = -x * uy + y * ux;
            umin = umin.min(u);
            umax = umax.max(u);
            vmin = vmin.min(v);
            vmax = vmax.max(v);
        }
        let area = (umax - umin) * (vmax - vmin);
        if area < best.0 {
            best = (area, dy.atan2(dx));
        }
    }
    best
}

/// Rendert das Teil in einem Welt-Rahmen (Iso + Draufsicht) als PNG.
fn render(bodies: &[step::Body], name: &str, frame: glam::Mat3) {
    for (suffix, view_dir, up_dir) in [
        ("iso", frame * Vec3::new(1.0, -1.0, 1.0).normalize(), frame * Vec3::Z),
        ("top", frame * Vec3::Z, frame * Vec3::Y),
    ] {
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
            let px: Vec<(f32, f32)> = pt.iter().map(|&(a, b, _)| to_px(a, b)).collect();
            let zs: [f32; 3] = [pt[0].2, pt[1].2, pt[2].2];
            let minx = (px.iter().fold(f32::MAX, |m, p| m.min(p.0)).max(0.0)) as usize;
            let maxx = (px.iter().fold(f32::MIN, |m, p| m.max(p.0)) as usize + 1).min(w);
            let miny = (px.iter().fold(f32::MAX, |m, p| m.min(p.1)).max(0.0)) as usize;
            let maxy = (px.iter().fold(f32::MIN, |m, p| m.max(p.1)) as usize + 1).min(h);
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
            .join(format!("test5_{name}_{suffix}.ppm"));
        std::fs::write(&out, &ppm).unwrap();
    }
    println!("PPMs für {name} geschrieben");
}

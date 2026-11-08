//! Flächen-Picking im 3D-Fenster für „Neu ausrichten“: Strahl durch den
//! Mauszeiger (Passpunkt des Orbit-Renders), nächstes Bauteil-Dreieck,
//! daraus die Normale der zusammenhängenden Ebene.

use crate::camera::OrbitCamera;
use crate::renderer::ScModel;
use eframe::egui::Rect;
use glam::Vec3;
use solidcore_record::Vec3 as RVec3;
use std::sync::Arc;

/// Mögliche Flächennormale (Anzeige-Koordinaten) unter dem Mauszeiger:
/// `None`, wenn kein Bauteil-Dreieck getroffen wird.
pub fn face_normal(model: &ScModel, camera: &OrbitCamera, rect: Rect, pos: [f32; 2]) -> Option<Vec3> {
    let (origin, dir) = pick_ray(camera, rect, pos);
    let hit = nearest_hit(model, origin, dir)?;
    // Normale der zusammenhängenden, ebenen Fläche (Flut-Füllung über
    // gemeinsame Kanten innerhalb eines kleinen Winkels) — so zählt die
    // ganze Flächenlage statt eines einzelnen Tessellierungs-Dreiecks.
    coplanar_normal(model, hit)
}

/// Treffer auf einer Bauteil-Fläche: Mesh, Dreiecksindex, B-rep-Face-ID und
/// Trefferpunkt (Welt-Koordinaten). Ignoriert Hilfsgeometrie (Rohteil-
/// Vorschau mit niedriger Deckkraft).
pub fn pick_part_face(
    model: &ScModel,
    camera: &OrbitCamera,
    rect: Rect,
    pos: [f32; 2],
) -> Option<(Arc<solidcraft_geom::Mesh>, u32, [f32; 3])> {
    let (origin, dir) = pick_ray(camera, rect, pos);
    let o = RVec3::new(origin.x as f64, origin.y as f64, origin.z as f64);
    let d = RVec3::new(dir.x as f64, dir.y as f64, dir.z as f64);
    let mut best: Option<(f64, Arc<solidcraft_geom::Mesh>, u32)> = None;
    for sm in &model.scene.meshes {
        // Nur Bauteil-Körper (keine Rohteil-/Hilfsdarstellung).
        if sm.opacity < 0.95 {
            continue;
        }
        if let Some((t, tri_idx)) = sm.mesh.raycast(o, d) {
            if t <= 1e-9 {
                continue;
            }
            let tri_face = sm.mesh.tri_face.get(tri_idx).copied().unwrap_or(u32::MAX);
            let better = best.as_ref().is_none_or(|(bt, _, _)| t < *bt);
            if better {
                best = Some((t, Arc::clone(&sm.mesh), tri_face));
            }
        }
    }
    best.map(|(t, mesh, tri_face)| {
        let hit = origin + dir * (t as f32);
        (mesh, tri_face, [hit.x, hit.y, hit.z])
    })
}

/// Strahl (Ursprung, Richtung) durch einen Bildpunkt — Spiegelbild der
/// Renderkamera: Perspektive über die sichtbare Höhe (`distance ·
/// tan(fov/2)`), Orthografie mit festem parallaktischem Kegel (0,4).
fn pick_ray(camera: &OrbitCamera, rect: Rect, pos: [f32; 2]) -> (Vec3, Vec3) {
    let eye = camera.eye();
    let forward = (camera.target - eye).normalize_or_zero();
    let up_hint = if camera.pitch.cos().abs() < 0.05 {
        Vec3::Y * camera.pitch.signum()
    } else {
        Vec3::Z
    };
    let right = forward.cross(up_hint).normalize_or_zero();
    let up = right.cross(forward).normalize_or_zero();

    let aspect = (rect.width() / rect.height().max(1.0)).max(0.01);
    // Bildschirm → normierte Koordinaten (y nach oben).
    let nx = (2.0 * (pos[0] - rect.left()) / rect.width().max(1.0) - 1.0).clamp(-1.0, 1.0);
    let ny = (1.0 - 2.0 * (pos[1] - rect.top()) / rect.height().max(1.0)).clamp(-1.0, 1.0);
    // Sichtbare Halb-Höhe am Zielabstand (wie `half_height`).
    let half_h = if camera.perspective {
        camera.distance * (camera.fov_y * 0.5).tan()
    } else {
        camera.distance * 0.4
    };
    let half_w = half_h * aspect;
    if camera.perspective {
        // Strahl=[Blick · dist + seitliche Offsets], skaliert auf Kegelradius.
        let dir = forward * camera.distance + right * (nx * half_w) + up * (ny * half_h);
        (eye, dir.normalize_or_zero())
    } else {
        let origin = eye + right * (nx * half_w) + up * (ny * half_h);
        (origin, forward)
    }
}

/// Nächstes getroffenes Dreieck (Möller–Trumbore in f64, beidseitig).
fn nearest_hit(model: &ScModel, origin: Vec3, dir: Vec3) -> Option<usize> {
    let positions = &model.part_positions;
    let o = [f64::from(origin.x), f64::from(origin.y), f64::from(origin.z)];
    let d = [f64::from(dir.x), f64::from(dir.y), f64::from(dir.z)];
    let mut best: Option<(f64, usize)> = None;
    for (i, t) in model.part_triangles.iter().enumerate() {
        let a = f64v(positions[t[0] as usize]);
        let b = f64v(positions[t[1] as usize]);
        let c = f64v(positions[t[2] as usize]);
        let e1 = sub(b, a);
        let e2 = sub(c, a);
        let pv = cross(d, e2);
        let det = dot(e1, pv);
        if det.abs() < 1e-15 {
            continue;
        }
        let inv = 1.0 / det;
        let tv = sub(o, a);
        let uu = dot(tv, pv) * inv;
        if !(-1e-9..=1.0 + 1e-9).contains(&uu) {
            continue;
        }
        let qv = cross(tv, e1);
        let vv = dot(d, qv) * inv;
        if !(-1e-9..=1.0 + 1e-9).contains(&vv) || uu + vv > 1.0 + 1e-9 {
            continue;
        }
        let tt = dot(e2, qv) * inv;
        if tt <= 1e-9 {
            continue;
        }
        if best.is_none_or(|(bt, _)| tt < bt) {
            best = Some((tt, i));
        }
    }
    best.map(|(_, i)| i)
}

/// Flächennormale der Ebene, zu der das Treffer-Dreieck gehört: Flut-Füllung
/// über Kanten-Nachbarn, deren Normale innerhalb ~2,5° übereinstimmt;
/// flächengewichtetes Mittel.
fn coplanar_normal(model: &ScModel, seed: usize) -> Option<Vec3> {
    let positions = &model.part_positions;
    let tris = &model.part_triangles;
    // Kanten-Nachbarschaft nur einmal je Klick aufbauen.
    let mut edges: std::collections::HashMap<(u32, u32), Vec<u32>> = std::collections::HashMap::new();
    for (i, t) in tris.iter().enumerate() {
        let (a, b, c) = (t[0], t[1], t[2]);
        for (p, q) in [(a, b), (b, c), (c, a)] {
            edges.entry((p.min(q), p.max(q))).or_default().push(i as u32);
        }
    }
    let unit_normal = |i: usize| -> Vec3 {
        let t = tris[i];
        let a = f32v(positions[t[0] as usize]);
        let b = f32v(positions[t[1] as usize]);
        let c = f32v(positions[t[2] as usize]);
        (b - a).cross(c - a).normalize_or_zero()
    };
    let n0 = unit_normal(seed);
    if n0.length_squared() < 0.5 {
        return None;
    }
    // Parallel-Schwelle (cos 2,5°): ebene Lagen zusammen, leichte Krümmung trennt.
    const PARALLEL: f32 = 0.999;
    let mut seen = vec![false; tris.len()];
    let mut stack = vec![seed];
    seen[seed] = true;
    let mut sum_area = 0.0_f32;
    let mut sum_normal = Vec3::ZERO;
    while let Some(i) = stack.pop() {
        let t = tris[i];
        let a = f32v(positions[t[0] as usize]);
        let b = f32v(positions[t[1] as usize]);
        let c = f32v(positions[t[2] as usize]);
        let area = (b - a).cross(c - a).length() * 0.5;
        let n = unit_normal(i);
        // Beidseitig parallel zählen (Winding kann je Körper wechseln).
        let aligned = if n.dot(n0) >= 0.0 { n } else { -n };
        sum_area += area;
        sum_normal += aligned * area;
        for (p, q) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let key = (p.min(q), p.max(q));
            if let Some(neigh) = edges.get(&key) {
                for &j in neigh {
                    let j = j as usize;
                    if !seen[j] && unit_normal(j).dot(n0).abs() > PARALLEL {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
    }
    let n = sum_normal / sum_area.max(1e-9);
    (n.length_squared() > 0.5).then_some(n.normalize_or_zero())
}

fn f64v(p: [f32; 3]) -> [f64; 3] {
    [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]
}
fn f32v(p: [f32; 3]) -> Vec3 {
    Vec3::new(p[0], p[1], p[2])
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[cfg(test)]
mod tests {
    use super::face_normal;
    use crate::camera::OrbitCamera;
    use crate::renderer::ScModel;
    use eframe::egui::{Rect, pos2, vec2};
    use glam::{Mat3, Vec3};
    use std::sync::Arc;

    /// Quader 20×10×5 um den Ursprung als Netz (12 Dreiecke).
    fn box_model() -> ScModel {
        let (x, y, z) = (10.0_f32, 5.0_f32, 2.5_f32);
        let corners = [
            [x, y, z],
            [x, y, -z],
            [x, -y, z],
            [x, -y, -z],
            [-x, y, z],
            [-x, y, -z],
            [-x, -y, z],
            [-x, -y, -z],
        ];
        // je Fläche zwei Dreiecke (Winding egal — Picking beidseitig).
        let faces: [[usize; 4]; 6] = [
            [0, 1, 3, 2], // +x
            [4, 6, 7, 5], // −x
            [0, 1, 5, 4], // +y
            [2, 3, 7, 6], // −y
            [0, 2, 6, 4], // +z
            [1, 3, 7, 5], // −z
        ];
        let mut triangles = Vec::new();
        for f in faces {
            triangles.push([f[0] as u32, f[1] as u32, f[2] as u32]);
            triangles.push([f[0] as u32, f[2] as u32, f[3] as u32]);
        }
        ScModel {
            scene: Arc::new(solidcraft_render::Scene::default()),
            center: Vec3::ZERO,
            radius: 12.0,
            part_size: Vec3::new(20.0, 10.0, 5.0),
            stock_size: Vec3::ZERO,
            clamp_mm: 0.0,
            top_allow_mm: 0.0,
            volume_mm3: 0.0,
            triangle_count: triangles.len(),
            edge_count: 0,
            bodies: Vec::new(),
            part_positions: corners.to_vec(),
            part_triangles: triangles,
            align_rot: Mat3::IDENTITY,
            round: false,
            round_axis: None,
        }
    }

    /// Draufsicht auf den Quader: Die Normale der geklickten Fläche liegt
    /// rein in Z (Flächen-Picking für „Neu ausrichten“).
    #[test]
    fn pick_top_face_from_top_view() {
        let model = box_model();
        let mut camera = OrbitCamera::default();
        camera.target = Vec3::ZERO;
        camera.pitch = 1.5533; // Draufsicht
        camera.distance = 10.0;
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 100.0));
        let center = [100.0, 50.0];
        let normal = face_normal(&model, &camera, rect, center).expect("getroffen");
        assert!(normal.z.abs() > 0.95, "Flächennormale: {normal:?}");
    }
}

//! Debug: tesselliert jede Fläche von test2.step einzeln und legt eine OBJ pro
//! verdächtiger Fläche ab. Hilft, die Flächen zu identifizieren, deren
//! Triangulation Artefakte erzeugt.

#[path = "../src/step.rs"]
mod step;
#[path = "../src/mesh.rs"]
mod mesh;
#[path = "../src/stock.rs"]
mod stock;
#[path = "../src/mesher.rs"]
mod mesher;

use glam::Vec3;
use std::path::Path;
use truck_meshalgo::tessellation::RobustMeshableShape;
use truck_stepio::r#in::alias::{
    re_exports::{InnerSpace, Invertible, Point3, SquareMatrix},
    ElementarySurface, Surface,
};

#[test]
fn debug_faces() {
    let doc = step::StepDocument::from_path(Path::new("test2.step")).unwrap();
    let table = truck_stepio::r#in::Table::from_step(&doc.source).unwrap();
    let out_dir = std::env::temp_dir().join("t2_faces");
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).unwrap();

    for (si, shell) in table.shell.values().enumerate() {
        let mut cshell = table.to_compressed_shell(shell).unwrap();
        println!("Shell {si}: {} Flächen, {} Vertices, {} Kanten",
            cshell.faces.len(), cshell.vertices.len(), cshell.edges.len());

        for fi in 0..cshell.faces.len() {
            let mut single = cshell.clone();
            single.faces.clear();
            single.faces.push(cshell.faces[fi].clone());

            //財 Flächentyp bestimmen
            let kind = match &cshell.faces[fi].surface {
                Surface::ElementarySurface(e) => match e.as_ref() {
                    ElementarySurface::Plane(_) => "plane",
                    ElementarySurface::CylindricalSurface(_) => "cylinder",
                    ElementarySurface::ConicalSurface(_) => "cone",
                    ElementarySurface::Sphere(_) => "sphere",
                    ElementarySurface::ToroidalSurface(_) => "torus",
                },
                _ => "other",
            };

            // Über den Randpuffer-Algorithmus informierten Kegel rand-anpassen.
            // (gleiche Logik wie in step.rs)
            let mut boundary_vertices = Vec::new();
            {
                let shell_vertices = &single.vertices;
                let shell_edges = &single.edges;
                let face = &single.faces[0];
                for boundary in &face.boundaries {
                    for edge_idx in boundary {
                        let edge = &shell_edges[edge_idx.index];
                        boundary_vertices.push(shell_vertices[edge.vertices.0]);
                        boundary_vertices.push(shell_vertices[edge.vertices.1]);
                    }
                }
            }

            let tessellated = single.robust_triangulation(step::Quality::High.tolerance());
            let mut tris: Vec<[usize; 3]> = Vec::new();
            let mut verts: Vec<Point3> = Vec::new();
            for face in &tessellated.faces {
                if let Some(poly) = &face.surface {
                    let base = verts.len();
                    for p in poly.positions() {
                        verts.push(*p);
                    }
                    for tri in poly.faces().tri_faces() {
                        tris.push([base + tri[0].pos, base + tri[1].pos, base + tri[2].pos]);
                    }
                    for quad in poly.faces().quad_faces() {
                        tris.push([base + quad[0].pos, base + quad[1].pos, base + quad[2].pos]);
                        tris.push([base + quad[0].pos, base + quad[2].pos, base + quad[3].pos]);
                    }
                    continue;
                }
                let Some(boundary) = face.boundaries.first() else { continue };
                let mut ring_points: Vec<Point3> = Vec::new();
                for edge_idx in boundary {
                    let edge = &tessellated.edges[edge_idx.index];
                    let mut points = edge.curve.0.clone();
                    if !edge_idx.orientation { points.reverse(); }
                    for p in points {
                        if ring_points.last().is_some_and(|l| (l - p).magnitude() < 1e-9) { continue; }
                        ring_points.push(p);
                    }
                }
                if ring_points.len() < 3 { continue; }
                let base = verts.len();
                verts.extend(ring_points.iter().copied());
                for i in 1..ring_points.len() - 1 {
                    tris.push([base, base + i, base + i + 1]);
                }
            }

            // Verdächtige: Dreiecke mit Flächeninhalt 0 oder sehr steile Kanten
            // oder Dreiecke fern der Randloop.
            if tris.is_empty() {
                println!("Fläche {} ({kind}): KEINE Dreiecke (übersprungen)", fi);
                continue;
            }
            // bbox der Randpingel vs. bbox der Dreiecke
            let mut bmin = Vec3::splat(f32::INFINITY);
            let mut bmax = Vec3::splat(f32::NEG_INFINITY);
            for v in &boundary_vertices {
                let v = Vec3::new(v.x as f32, v.y as f32, v.z as f32);
                bmin = bmin.min(v);
                bmax = bmax.max(v);
            }
            let mut tmin = Vec3::splat(f32::INFINITY);
            let mut tmax = Vec3::splat(f32::NEG_INFINITY);
            for v in &verts {
                let v = Vec3::new(v.x as f32, v.y as f32, v.z as f32);
                tmin = tmin.min(v);
                tmax = tmax.max(v);
            }
            let diag_b = (bmax - bmin).length();
            let diag_t = (tmax - tmin).length();
            // Ausdehnung außerhalb der Randloop-bbox
            let grow = (tmax - bmax).max(Vec3::splat(0.0)) + (bmin - tmin).max(Vec3::splat(0.0));
            let grow_mag = grow.length();
            if grow_mag > 0.5 || !diag_b.is_finite() {
                println!("Fläche {} ({kind}): Randdiag={diag_b:.3} Meshdiag={diag_t:.3} Überhang={grow_mag:.3} -> {}/face_{:03}.obj",
                    fi, out_dir.display(), fi);
                write_obj(&out_dir.join(format!("face_{fi:03}.obj")), &verts, &tris);
                // Randloop ebenfalls als Objekt für Vergleich
                write_loop_obj(&out_dir.join(format!("face_{fi:03}_loop.obj")), &boundary_vertices);
            }
        }
    }
}

fn write_obj(path: &Path, verts: &[Point3], tris: &[[usize; 3]]) {
    let mut s = String::new();
    for v in verts {
        s.push_str(&format!("v {} {} {}\n", v.x, v.y, v.z as f64));
    }
    for t in tris {
        s.push_str(&format!("f {} {} {}\n", t[0] + 1, t[1] + 1, t[2] + 1));
    }
    std::fs::write(path, s).unwrap();
}

fn write_loop_obj(path: &Path, points: &[Point3]) {
    let mut s = String::from("v\n");
    for v in points {
        s.push_str(&format!("v {} {} {}\n", v.x, v.y, v.z as f64));
    }
    std::fs::write(path, s).unwrap();
}

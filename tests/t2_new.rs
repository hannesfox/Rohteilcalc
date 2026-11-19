//! Debug: für jede Fläche von test2.step den analytischen Mesher aufrufen und
//! zählen, welche Flächen scheitern und welche Normals/Winding医生的 konträren
//! Indizien zeigen.

#[path = "../src/step.rs"]
mod step;
#[path = "../src/mesh.rs"]
mod mesh;
#[path = "../src/stock.rs"]
mod stock;
#[path = "../src/mesher.rs"]
mod mesher;

use std::path::Path;
use truck_stepio::r#in::alias::re_exports::{BoundedCurve, ParameterDivision1D};
use truck_stepio::r#in::alias::Surface;

#[test]
fn analytic_coverage() {
    let doc = step::StepDocument::from_path(Path::new("test2.step")).unwrap();
    let table = truck_stepio::r#in::Table::from_step(&doc.source).unwrap();

    let mut total = 0usize;
    let mut failed = Vec::new();
    for (si, shell) in table.shell.values().enumerate() {
        let cshell = table.to_compressed_shell(shell).unwrap();
        println!(
            "Shell {si}: {} Flächen, {} Kanten",
            cshell.faces.len(),
            cshell.edges.len()
        );
        for (fi, face) in cshell.faces.iter().enumerate() {
            total += 1;
            let kind = match &face.surface {
                Surface::ElementarySurface(e) => match e.as_ref() {
                    truck_stepio::r#in::alias::ElementarySurface::Plane(_) => "Plane",
                    truck_stepio::r#in::alias::ElementarySurface::Sphere(_) => "Sphere",
                    truck_stepio::r#in::alias::ElementarySurface::CylindricalSurface(_) => "Cyl",
                    truck_stepio::r#in::alias::ElementarySurface::ToroidalSurface(_) => "Torus",
                    truck_stepio::r#in::alias::ElementarySurface::ConicalSurface(_) => "Cone",
                },
                Surface::SweptCurve(_) => "Swept",
                Surface::BSplineSurface(_) => "BSpline",
                Surface::NurbsSurface(_) => "Nurbs",
            };
            let loops: usize = face.boundaries.len();
            match mesher::tessellate_face(face, &cshell.vertices, &cshell.edges, 0.05, &format!("face{fi} {kind}")) {
                Some(mesh) => {
                    // Boundary-BBOX aus den abgeflachten Randkurven.
                    let mut bmin = [f64::INFINITY; 3];
                    let mut bmax = [f64::NEG_INFINITY; 3];
                    for b in &face.boundaries {
                        for e in b {
                            let curve = &cshell.edges[e.index].curve;
                            let range = curve.range_tuple();
                            let (_, pts) = curve.parameter_division(range, 0.05);
                            for q in pts {
                                let arr = [q.x, q.y, q.z];
                                for d in 0..3 {
                                    bmin[d] = bmin[d].min(arr[d]);
                                    bmax[d] = bmax[d].max(arr[d]);
                                }
                            }
                        }
                    }
                    let mut over = 0f64;
                    for v in &mesh.vertices {
                        let arr = [v.x, v.y, v.z];
                        for d in 0..3 {
                            if arr[d] < bmin[d] { over = over.max(bmin[d] - arr[d]); }
                            if arr[d] > bmax[d] { over = over.max(arr[d] - bmax[d]); }
                        }
                    }
                    if over > 1e-6 {
                        println!("OVERHANG shell{si} face{fi} {kind} loops{loops} = {over:.4} mm");
                    } else {
                        println!(
                            "shell{si} face{fi} {kind} loops{loops} -> {} dreiecke {} vertices",
                            mesh.triangles.len(),
                            mesh.vertices.len()
                        );
                    }
                }
                None => failed.push(format!("shell{si} face{fi} {kind} loops{loops}")),
            }
        }
    }
    println!("TOTAL {total}, FAILED {}", failed.len());
    for f in &failed {
        println!("FAILED {f}");
    }
}

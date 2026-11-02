//! Automatisierte CNC-Rohteilberechnung.
//!
//! Architektur:
//! - [`step`]   STEP-Import (B-Rep bleibt als Dokument erhalten) + Tessellierung
//! - [`mesh`]   Anzeigenetz, Normalen, Feature-Kanten, Kanten-Glättung
//! - [`stock`]  Ausrichtung und Rohteilregeln
//! - [`camera`] Orbit-Kamera mit Standardansichten
//! - [`renderer`] wgpu-Renderer (MSAA, Tiefenpuffer, Blinn-Phong, Kanten)
//! - [`app`]    egui-Oberfläche und asynchrones Laden

mod app;
mod camera;
mod mesh;
mod renderer;
mod step;
mod stock;

use app::AppState;
use camera::OrbitCamera;
use eframe::egui;
use renderer::{CameraUniforms, SceneData, ViewMode};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Headless: `cargo run -- --inspect datei.step`
    if args.first().map(String::as_str) == Some("--inspect") {
        let Some(path) = args.get(1) else {
            eprintln!("Nutzung: rohteil_calc --inspect <datei.step>");
            std::process::exit(2);
        };
        match step::load_aligned(Path::new(path), step::Quality::Medium) {
            Ok((bodies, stock)) => {
                let tri: usize = bodies.iter().map(|b| b.mesh.triangles.len()).sum();
                println!("Körper:   {}", bodies.len());
                println!("Dreiecke: {tri}");
                println!(
                    "Bauteil:  {:.2} × {:.2} × {:.2} mm",
                    stock.part_size.x, stock.part_size.y, stock.part_size.z
                );
                println!(
                    "Rohteil:  {:.2} × {:.2} × {:.2} mm",
                    stock.stock_size.x, stock.stock_size.y, stock.stock_size.z
                );
            }
            Err(err) => {
                eprintln!("Fehler: {err}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // Headless-GPU-Render: `--render datei.step out.bmp [yaw pitch zoom] [mode]`
    if args.first().map(String::as_str) == Some("--render") {
        let (Some(path), Some(out)) = (args.get(1), args.get(2)) else {
            eprintln!(
                "Nutzung: rohteil_calc --render <datei.step> <out.bmp> [yaw pitch zoom] [shaded|edges|wire]"
            );
            std::process::exit(2);
        };
        let yaw = args
            .get(3)
            .and_then(|s| s.parse::<f32>().ok())
            .unwrap_or(-0.72);
        let pitch = args
            .get(4)
            .and_then(|s| s.parse::<f32>().ok())
            .unwrap_or(0.42);
        let zoom = args
            .get(5)
            .and_then(|s| s.parse::<f32>().ok())
            .unwrap_or(1.0);
        let mode = match args.get(6).map(String::as_str) {
            Some("shaded") => ViewMode::Shaded,
            Some("wire") | Some("wireframe") => ViewMode::Wireframe,
            _ => ViewMode::ShadedEdges,
        };
        if let Err(err) = render_cli(Path::new(path), Path::new(out), yaw, pitch, zoom, mode) {
            eprintln!("Fehler: {err}");
            std::process::exit(1);
        }
        return Ok(());
    }

    let initial = args.first().map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_title("Automatisierte CNC Rohteilberechnung"),
        renderer: eframe::Renderer::Wgpu,
        multisampling: 0, // MSAA passiert im Offscreen-Ziel des Viewports
        ..Default::default()
    };
    eframe::run_native(
        "Rohteilberechnung",
        options,
        Box::new(move |cc| Box::new(AppState::new(cc, initial))),
    )
}

fn render_cli(
    path: &Path,
    out: &Path,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    mode: ViewMode,
) -> Result<(), String> {
    let (mut bodies, stock) = step::load_aligned(path, step::Quality::High)?;
    for b in &mut bodies {
        b.visible = true;
    }
    let scene = Arc::new(SceneData::build(&bodies, &stock, 1));

    let mut camera = OrbitCamera::default();
    camera.fit(scene.center, scene.radius);
    camera.yaw = yaw;
    camera.pitch = pitch;
    camera.distance = camera.fit_distance() / zoom.max(0.01);

    let (w, h) = (1400u32, 1000u32);
    let aspect = w as f32 / h as f32;
    let uniforms = CameraUniforms {
        view_proj: camera.view_projection(aspect),
        eye: camera.eye_position(),
    };

    let pixels = renderer::render_offscreen(
        &scene,
        uniforms,
        w,
        h,
        mode,
        true,
    )?;
    write_bmp(out, w, h, &pixels)?;
    println!(
        "Render: {} Körper, {} Dreiecke, {} Feature-Kanten -> {} ({w}×{h})",
        bodies.len(),
        scene.triangle_count,
        scene.feature_edges.len() / 2,
        out.display()
    );
    Ok(())
}

/// Schreibt RGBA-Pixel als 24-Bit-BMP (BGR, bottom-up).
fn write_bmp(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    if rgba.len() < (w * h * 4) as usize {
        return Err("Bilddaten unvollständig".to_string());
    }
    let row = (w * 3 + 3) & !3;
    let size = 54 + row * h;
    let mut bytes = Vec::with_capacity(size as usize);
    bytes.extend_from_slice(b"BM");
    bytes.extend_from_slice(&size.to_le_bytes());
    bytes.extend_from_slice(&[0u8; 4]);
    bytes.extend_from_slice(&54u32.to_le_bytes());
    bytes.extend_from_slice(&40u32.to_le_bytes());
    bytes.extend_from_slice(&(w as i32).to_le_bytes());
    bytes.extend_from_slice(&(h as i32).to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&24u16.to_le_bytes());
    bytes.extend_from_slice(&[0u8; 24]);
    for y in (0..h).rev() {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            bytes.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i]]);
        }
        for _ in w * 3..row {
            bytes.push(0);
        }
    }
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

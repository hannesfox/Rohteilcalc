//! Automatisierte CNC-Rohteilberechnung.
//!
//! Architektur:
//! - [`renderer`] SolidCraft-Ansicht: STEP-Import (engine), Szene (render),
//!   Pipelines (gpu); Rohteil-Ausrichtung und Rohteil-Box liegen obendrauf
//! - [`step`]     Legacy-truck-Import (nur noch für tests/diagnose)
//! - [`mesh`]     Rohteil-Netztyp (Demo-Bauteil und Rohteil-Rechnung)
//! - [`stock`]    Ausrichtung und Rohteilregeln
//! - [`camera`]   Orbit-Kamera mit Standardansichten
//! - [`app`]      egui-Oberfläche und asynchrones Laden
mod app;
mod camera;
mod mesh;
#[allow(dead_code)]
mod mesher;
mod renderer;
mod space_mouse;
#[allow(dead_code)]
mod step;
mod stock;

use app::AppState;
use camera::OrbitCamera;
use eframe::egui;
use glam::Vec3;
use renderer::{ScSession, ViewMode};
use std::path::{Path, PathBuf};

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Achsen-Sonde der 3D-Maus: `--space-probe` (20 s) zeigt die sechs
    // Rohachsen live, so dass man sieht, welcher Kanal auf welche Richtung
    // des Deckels reagiert. Endet nach 20 s von selbst (der Driver-Client
    // meldet sich dann sauber ab).
    if args.first().map(String::as_str) == Some("--space-probe") {
        space_probe();
        return Ok(());
    }

    // Headless: `cargo run -- --inspect datei.step`
    if args.first().map(String::as_str) == Some("--inspect") {
        let Some(path) = args.get(1) else {
            eprintln!("Nutzung: rohteil_calc --inspect <datei.step>");
            std::process::exit(2);
        };
        match inspect_cli(Path::new(path)) {
            Ok(out) => {
                println!("Körper:   {}", out.bodies);
                println!("Dreiecke: {}", out.triangles);
                println!(
                    "Bauteil:  {:.2} × {:.2} × {:.2} mm",
                    out.part.x, out.part.y, out.part.z
                );
                println!(
                    "Rohteil:  {:.2} × {:.2} × {:.2} mm",
                    out.stock.x, out.stock.y, out.stock.z
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
            Some("wire") | Some("wireframe") => ViewMode::Wire,
            _ => ViewMode::ShadedEdges,
        };
        if let Err(err) = render_cli(
            Path::new(path),
            Path::new(out),
            yaw,
            pitch,
            zoom,
            mode,
        ) {
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
        multisampling: 1, // MSAA passiert im Offscreen-Ziel des Viewports
        ..Default::default()
    };
    eframe::run_native(
        "Rohteilberechnung",
        options,
        Box::new(move |cc| {
            if let Err(e) = renderer::install(cc) {
                eprintln!("Warnung: GPU-Pipelines nicht installiert: {e}");
            }
            Ok(Box::new(AppState::new(cc, initial)))
        }),
    )
}

// ---------------------------------------------------------
// Headless-Befehle (--inspect, --render) über die SolidCraft-Pipelines.
// ---------------------------------------------------------

/// Sonde in einem echten Fenster: gleiche egui/eframe-Umgebung wie die
/// App, weil der 3Dconnexion-Driver Zustandsmeldungen offenbar nur an Clients
/// mit GUI-Ereignisschleife versendet (ohne Fenster bekam die Sonde nichts).
/// Zeigt 20 s lang die rohen Achsen als Balken und schreibt jede Änderung
/// ins Terminal.
struct Probe {
    mouse: space_mouse::SpaceMouse,
    start: std::time::Instant,
    last: [i16; 6],
    /// Wert mit größtem Betrag je Kanal während der Sonde (am Ende ausgegeben).
    peak: [i16; 6],
    /// Ob überhaupt ein Achsen-Event angekommen ist (für die Anzeige).
    got_signal: bool,
    /// Letzter auf's Terminal geschriebener Sekundenwert (Statusraster).
    last_print: f64,
    /// Zuletzt gedruckte Zustandszeile, damit Änderungen sofort kommen.
    last_status: String,
    /// Dasselbe, für's Hauptprogramm nach dem Fenster schließt geteilt.
    peaks: std::sync::Arc<std::sync::Mutex<[i16; 6]>>,
}

impl Probe {
    /// Zustandszeile auf's Terminal — sofort bei Änderung, sonst alle 5 s.
    fn print_status(&mut self) {
        let elapsed = self.start.elapsed().as_secs_f64();
        let (registered, device) = space_mouse::driver_status();
        let status = format!("registriert={registered} gerät={device} empfang={}", self.got_signal);
        if status != self.last_status || elapsed - self.last_print >= 5.0 {
            println!( "[{:5.1}s] Driver {status}   Spitze Ty: {:+}", elapsed, self.peak[1]);
            self.last_status = status;
            self.last_print = elapsed;
        }
    }
}

impl eframe::App for Probe {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // tick hält den Wheel-Guard am Leben und fragt nach Repaints.
        let _ = self.mouse.tick(ui.ctx(), 200.0);
        self.print_status();
        let axes = self.mouse.raw_axes();
        for (peak, value) in self.peak.iter_mut().zip(axes) {
            if value.abs() > peak.abs() {
                *peak = value;
            }
        }
        if let Ok(mut peaks) = self.peaks.lock() {
            *peaks = self.peak;
        }
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("3D-Maus-Sonde");
            let (registered, device) = space_mouse::driver_status();
            ui.label(format!(
                "Driver-Client: {}   Gerät: {}",
                if registered { "registriert ✓" } else { "NICHT REGISTRIERT ✗" },
                if device { "gemeldet ✓" } else { "kein Gerät ✗" },
            ));
            ui.label(format!("Empfang: {}", if self.got_signal { "aktiv ✓" } else { "noch nichts angekommen" }));
            ui.label("1. Dieses Fenster anklicken (Vordergrund) — der Treiber\nliefert nur der aktiven App.\n2. Dann den Deckel schieben und kanten.\nSchluss nach 20 Sekunden; Spitzenwerte stehen am Ende im Terminal.");
            ui.add_space(6.0);
            for (name, value, peak) in [
                ("Tx seitlich", axes[0], self.peak[0]),
                ("Ty ZOOM-Kanal", axes[1], self.peak[1]),
                ("Tz auf/ab", axes[2], self.peak[2]),
                ("Rx neigen", axes[3], self.peak[3]),
                ("Ry seitl. kippen", axes[4], self.peak[4]),
                ("Rz verdrillen", axes[5], self.peak[5]),
            ] {
                let share = (f32::from(value).abs() / 350.0).clamp(0.0, 1.0);
                let bar = egui::ProgressBar::new(share).text(format!("{name}: {value:+5}  (Spitze {peak:+5})"));
                ui.add(bar);
            }
        });
        if axes != self.last {
            if axes == [0; 6] {
                println!("Ruhelage.");
            } else if self.last == [0; 6] {
                println!("Empfang aktiv — der Treiber liefert:");
            }
            self.got_signal |= axes != [0; 6];
            let fmt = |v: i16| format!("{v:+5}");
            println!(
                "{} {} {} | {} {} {}",
                fmt(axes[0]),
                fmt(axes[1]),
                fmt(axes[2]),
                fmt(axes[3]),
                fmt(axes[4]),
                fmt(axes[5]),
            );
            self.last = axes;
        }
        if self.start.elapsed() > std::time::Duration::from_secs(20) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn space_probe() {
    use std::time::Instant;
    let ctx = eframe::egui::Context::default();
    let Some(mouse) = space_mouse::start(&ctx) else {
        eprintln!("3D-Maus: kein Leser vorhanden (nicht macOS oder fehlender HID-Zugriff)");
        return;
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([460.0, 340.0])
            .with_title("3D-Maus-Sonde"),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    let peaks = std::sync::Arc::new(std::sync::Mutex::new([0_i16; 6]));
    let app = Probe { mouse, start: Instant::now(), last: [0; 6], peak: [0; 6], got_signal: false, last_print: 0.0, last_status: String::new(), peaks: std::sync::Arc::clone(&peaks) };
    let result = eframe::run_native(
        "3d-maus-sonde",
        options,
        Box::new(move |_cc| Ok(Box::new(app) as Box<dyn eframe::App>)),
    );
    if let Err(err) = result {
        eprintln!("Sonde: Fenster nicht zu starten: {err}");
    }
    let fmt = |v: i16| format!("{v:+5}");
    let space_probe_peak = if let Ok(peak) = peaks.lock() {
        [peak[0], peak[1], peak[2], peak[3], peak[4], peak[5]]
    } else {
        [0; 6]
    };
    println!(
        "Spitzenwerte je Kanal: Tx {} Ty {} Tz {} | Rx {} Ry {} Rz {}",
        fmt(space_probe_peak[0]),
        fmt(space_probe_peak[1]),
        fmt(space_probe_peak[2]),
        fmt(space_probe_peak[3]),
        fmt(space_probe_peak[4]),
        fmt(space_probe_peak[5]),
    );
    println!("Sonde vorbei (der Driver-Client meldet sich sauber ab).");
}

struct InspectOut {
    bodies: usize,
    triangles: usize,
    part: Vec3,
    stock: Vec3,
}

fn inspect_cli(path: &Path) -> Result<InspectOut, String> {
    let session = ScSession::open(path, crate::step::Quality::Medium)?;
    let model = session.model(ViewMode::ShadedEdges, false)?;
    Ok(InspectOut {
        bodies: model.bodies.len(),
        triangles: model.triangle_count,
        part: model.part_size,
        stock: model.stock_size,
    })
}

fn render_cli(
    path: &Path,
    out: &Path,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    mode: ViewMode,
) -> Result<(), String> {
    let session = ScSession::open(path, crate::step::Quality::High)?;
    let model = session.model(mode, false)?;
    let mut camera = OrbitCamera::default();
    camera.fit(model.center, model.radius);
    camera.yaw = yaw;
    camera.pitch = pitch;
    camera.distance = camera.fit_distance() / zoom.max(0.01);
    let cam = renderer::render_camera(&camera);
    let (w, h) = (1400u32, 1000u32);
    let pixels = renderer::render_offscreen(&model.scene, &cam, w, h)?;
    write_bmp(out, w, h, &pixels)?;
    println!(
        "Render: {} Körper, {} Dreiecke, {} Kantenpolylines -> {} ({w}×{h})",
        model.bodies.len(),
        model.triangle_count,
        model.edge_count,
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
        // Zeilenlänge muss Vielfaches von 4 sein (BMP-Polsterung).
        let pad = (row - w * 3) as usize;
        bytes.extend(std::iter::repeat_n(0_u8, pad));
    }
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

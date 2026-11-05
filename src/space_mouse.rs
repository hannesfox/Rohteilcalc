//! 3D-Maus (SpaceMouse) für Rohteilcalc — auf die Art von SolidCraft
//! (apps/solidcraft/src/space_mouse.rs, MIT OR Apache-2.0):
//!
//! Ein Leser-Thread öffnet das Gerät (HID über hidapi) und meldet die sechs
//! Achsen über einen Mutex an den egui-Thread; der egui-Thread legt sie pro
//! Frame an die Kamera (Orbit, Pan, Zoom). Der Thread rührt nie die Szene
//! oder das UI an, er fragt nur nach Repaints. Die Achsen werden hier
//! normiert (`tick`), die Kamera liegt bei `crate::camera`.
//!
//! Umgesetzt ist der macOS-Pfad: hidapi (Generic-Desktop-Usage 0x08
//! „Multi-Axis Controller“) plus Client des 3Dconnexion-Drivers, der das
//! Gerät auf macOS exklusiv hält. Auf anderen Systemen tut `start` nichts
//! (Linux/spacenavd und Windows lassen sich aus SolidCraft nachportieren).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Häufigster Repaint für die Reports des Geräts (es sendet schneller, als
/// jemand sieht).
pub const REPAINT_EVERY: Duration = Duration::from_millis(16);
/// Ohne frischen Report gilt eine Achse nach so langer Zeit als in Ruhe:
/// HID-Geräte wiederholen ihren Report nur, solange der Deckel gehalten
/// wird — Schweigen heißt losgelassen (oder das Gerät ist fort).
pub const STALE_AFTER: Duration = Duration::from_millis(150);

/// Die sechs Rohachsen: X, Y, Z, Rx, Ry, Rz, ±350 bei voller Auslenkung.
pub type Axes = [i16; 6];

/// Was der Leser-Thread zuletzt vom Gerät gehört hat; der egui-Thread liest.
#[derive(Clone)]
struct Reading {
    axes: Axes,
    /// Wann Achse i zuletzt frisch war (je für sich: Translation und Rotation
    /// kommen oft in getrennten Reports).
    at: [Instant; 6],
}

impl Reading {
    fn new(now: Instant) -> Self {
        Self { axes: [0; 6], at: [now; 6] }
    }

    /// Die Achsen bei `now`: jede, deren letzter Wert zu alt ist, liest 0.
    fn current(&self, now: Instant) -> Axes {
        let mut axes = self.axes;
        for (value, at) in axes.iter_mut().zip(self.at) {
            if now.duration_since(at) >= STALE_AFTER {
                *value = 0;
            }
        }
        axes
    }
}

/// Tote Trefferzone, damit ein Deckel, der knapp außerhalb seiner Null ruht,
/// die Karte nicht schleichend bewegt; volle Auslenkung zählt 1.
fn normalized(value: i16) -> f64 {
    let v = f64::from(value);
    if v.abs() <= 20.0 {
        0.0
    } else {
        v.signum() * ((v.abs() - 20.0) / 330.0).min(1.0)
    }
}

/// Sekunden nach der letzten Deckelbewegung, in denen Rad-Ereignisse noch
/// dem Treiber zugerechnet werden (Scrolls können kurz vor dem Loslassen
/// angestaut sein und erst den folgenden Frame erreichen).
pub const WHEEL_QUIET_S: f64 = 0.15;

/// Ein Frame 3D-Maus-Bewegung, wie SolidCrafts `navigation::device_camera`
/// sie in Kamera-Schritte umrechnet (Drehtisch-Orbit mit Roll):
/// - `pan_x`/`pan_y`: Bildschirm-Deltas in Punkten für die Kamera,
/// - `zoom`: Schub·Framezeit (positiv = fort vom Bauteil); die Anwendung
///   dreht daraus den Faktor `exp(−1.5·zoom)` — wie `zoom_at` in SolidCraft,
/// - `orbit_x`/`orbit_y`: Drehtisch-Deltas (durch die Roll-Kante zurück
///   aufgelöst, `OrbitCamera::orbit_on_screen`),
/// - `roll`: Roll-Radians, die der Frame zusätzlich kantet,
/// - `seconds`: Frame-Zeit, auf 50 ms begrenzt.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeviceMotion {
    pub pan_x: f32,
    pub pan_y: f32,
    pub zoom: f32,
    pub orbit_x: f32,
    pub orbit_y: f32,
    pub roll: f32,
    pub seconds: f32,
}


/// Welche körperliche Achse welche logische Dreidmaus-Richtung steuert: die
/// Achsenzuordnung des 3DxWare-Treibers ohne Profil — sein Objekt-Modus (wie
/// in SolidCraft `DriverMapping::object_mode`). X, Y, Z und Rx (Neigen vor/
/// zurück) gehen gerade durch; körperliches Ry (seitlich kippen) rollt den
/// Blick, körperliches Rz (Verdrillen) dreht ihn seitwärts — beides
/// gespiegelt, wie der Treiber es tut.
#[rustfmt::skip]
fn object_mode(axes: Axes) -> ([f64; 3], [f64; 3]) {
    let v = |i: usize| normalized(axes[i]);
    (
        [v(0), v(1), v(2)],
        [v(3), -v(5), -v(4)],
    )
}

pub struct SpaceMouse {
    shared: Arc<Mutex<Reading>>,
    last_frame: Instant,
    /// Wann sich der Deckel zuletzt bewegt hat (für den WheelGuard).
    moved_at: Option<Instant>,
    /// Ob die Rad-Ereignisse dieses Frames vom Treiber stammen (Deckel bewegt
    /// oder eben erst stehen geblieben). Der Treiber imitiert aus der
    /// Deckelbewegung Mausrad-Scrolls (#1380); die dazu gehörten Wegwerfen.
    wheel_is_device: bool,
}

impl SpaceMouse {
    /// Die aktuellen rohen Achsen (einzeln alternd), für die Kanalprobe.
    pub fn raw_axes(&self) -> Axes {
        self.shared
            .lock()
            .map(|reading| reading.current(Instant::now()))
            .unwrap_or([0; 6])
    }

    /// Wessen Rad ist das? Während der Deckel bewegt wird — und eine kurze
    /// Stille danach — sind Rad-Ereignisse des Treibers (3DxWare imitiert
    /// Mausrad-Drehungen aus der Deckelbewegung, #1380); danach des
    /// Anwenders. Rohteilcalc kann das Rohinput nicht filtern (eframe), so
    /// dass der Viewport seinen Zoom nur auflegt, wenn hier `true` kommt.
    pub fn wheel_is_user(&self) -> bool {
        !self.wheel_is_device
    }
    /// Ein Frame: seconds seither, die frischen Achsen (je altert einzeln)
    /// normiert und auf den Viewport umgerechnet. `None`, wenn in Ruhe.
    /// Während der Deckel bewegt, fragt hier nach dem nächsten Bild — so
    /// bleibt die Bewegung ruckfrei.
    pub fn tick(&mut self, ctx: &egui::Context, viewport_height: f32) -> Option<DeviceMotion> {
        let now = Instant::now();
        let seconds = now.duration_since(self.last_frame).as_secs_f64();
        self.last_frame = now;
        let axes = self.shared.lock().ok()?.current(now);
        // Objekt-Modus (3DxWare ohne Profil, wie in SolidCraft): logische
        // [x, y, z] und [rx, ry, rz] aus den körperlichen Achsen.
        let [tx, ty, tz] = object_mode(axes).0;
        let [rx, ry, rz] = object_mode(axes).1;
        let (pan_x, zoom, pan_y) = (tx, ty, tz);
        let (orbit_x, orbit_y) = (ry, rx);
        // Eine lange Pause (das erste Bild nach einer Pause) bewegt nicht
        // weiter als ein 50-ms-Frameschritt, wie device_camera.
        let step = seconds.min(0.05);
        if step <= 0.0 {
            return None;
        }
        let moving = (pan_x, pan_y, zoom, orbit_x, orbit_y, rz)
            != (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        // WheelGuard (wie SolidCraft): während der Deckel sich bewegt und
        // [`WHEEL_QUIET_S`] danach sind Rad-Ereignisse des Treibers — der
        // sendet sie ab etwa einem Drittel Auslenkung, nie in Ruhe, so dass
        // ein Rad-Dreh bei stillstehendem Deckel des Anwenders bleibt.
        if moving {
            self.moved_at = Some(now);
        }
        let since = self.moved_at.map(|at| now.duration_since(at).as_secs_f64());
        self.wheel_is_device = moving || since.is_some_and(|s| (0.0..=WHEEL_QUIET_S).contains(&s));
        if !moving {
            return None;
        }
        let height = f64::from(viewport_height.max(1.0));
        // Volle Auslenkung: 0.6 Viewporthöhen pro Sekunde Pan, Faktor
        // exp(1.5) pro Sekunde Zoom, 250 px (≈ 2 rad bei 0.008 rad/px)
        // pro Sekunde Orbit, 2 rad/s Roll — unabhängig von der Bildrate.
        let motion = DeviceMotion {
            pan_x: (pan_x * height * 0.6 * step) as f32,
            pan_y: (pan_y * height * 0.6 * step) as f32,
            zoom: (zoom * step) as f32,
            orbit_x: (orbit_x * 250.0 * step) as f32,
            orbit_y: (orbit_y * 250.0 * step) as f32,
            roll: (rz * 2.0 * step) as f32,
            seconds: step as f32,
        };
        ctx.request_repaint_after(REPAINT_EVERY);
        Some(motion)
    }
}

impl Drop for SpaceMouse {
    fn drop(&mut self) {
        // Der Driver behält einen Client eines vergriffenen Prozesses
        // registriert; ein sauberer Abflug hält ihn frei.
        #[cfg(target_os = "macos")]
        solidcraft_3dx_client::stop();
    }
}

/// Startet den Leser. `None`, wenn nichts gelesen werden kann (anderes
/// System, HID ohne Zugriff) — die Anwendung läuft einfach ohne 3D-Maus.
#[cfg(not(target_os = "macos"))]
pub fn start(_ctx: &egui::Context) -> Option<SpaceMouse> {
    None
}

/// Zustand der Treiberanbindung für die Sonde: (Client registriert?,
/// Gerät vom Driver gemeldet?). Ohne macOS: immer nein.
#[cfg(not(target_os = "macos"))]
pub fn driver_status() -> (bool, bool) {
    (false, false)
}

/// Zustand der Treiberanbindung für die Sonde: (Client registriert?,
/// Gerät vom Driver gemeldet?).
#[cfg(target_os = "macos")]
pub fn driver_status() -> (bool, bool) {
    (
        mod_reader::DRIVER_REGISTERED.load(std::sync::atomic::Ordering::Relaxed),
        mod_reader::DRIVER_CONNECTED.load(std::sync::atomic::Ordering::Relaxed),
    )
}

#[cfg(target_os = "macos")]
pub fn start(ctx: &egui::Context) -> Option<SpaceMouse> {
    let now = Instant::now();
    let shared = Arc::new(Mutex::new(Reading::new(now)));
    let reader_state = Arc::clone(&shared);
    let reader_ctx = ctx.clone();
    let spawned = std::thread::Builder::new()
        .name("rohteilcalc-3d-mouse".into())
        .spawn(move || mod_reader::reader(reader_ctx, reader_state));
    if let Err(error) = spawned {
        eprintln!("3D-Maus: Leser konnte nicht starten: {error}");
    }
    // macOS: der 3Dconnexion-Driver gibt das Gerät nur seinen eigenen
    // Clients; die Anwendung meldet sich zusätzlich als einer. Läuft der
    // Helper nicht, liest der HID-Leser oben selbst.
    mod_reader::start_driver_client(ctx, Arc::clone(&shared));
    Some(SpaceMouse { shared, last_frame: now, moved_at: None, wheel_is_device: false })
}

#[cfg(target_os = "macos")]
mod mod_reader {
    use std::ffi::CString;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use egui::Context;
    use hidapi::{HidApi, HidDevice};
    use solidcraft_space_mouse::{Descriptor, Layout};

    use super::{Reading, REPAINT_EVERY, STALE_AFTER};

    /// Hat der 3Dconnexion-Driver das Gerät, meldet er den Deckel selbst —
    /// direkte HID-Reports würden doppelt zählen (und scheitern).
    pub(super) static DRIVER_CONNECTED: AtomicBool = AtomicBool::new(false);

    /// ist der Client einmal beim Driver eingetragen, so hält der Driver das
    /// Gerät — ein Fehlschlag beim direkten Öffnen ist dann keine Meldung.
    pub(super) static DRIVER_REGISTERED: AtomicBool = AtomicBool::new(false);

    /// Der Generic-Desktop-Usage „Multi-Axis Controller“ benennt die
    /// Bewegungs-Schnittstelle einer 3D-Maus — egal von wem — und schließt
    /// gewöhnliche Mäuse und die Tastatur-Interfaces der 3D-Mäuse aus.
    fn is_3d_mouse(info: &hidapi::DeviceInfo) -> bool {
        info.usage_page() == 0x01 && info.usage() == 0x08
    }

    /// Ein offenes 3D-Maus-Interface: das Gerät und der Ort der Achsen in
    /// seinen Reports.
    struct Candidate {
        device: HidDevice,
        layout: Layout,
    }

    /// Zeigt ein Report ein Gerät in Benutzung: eine Achse abseits von null.
    /// Ein Deckel in Ruhe sendet vor allem Nullen; ein Empfänger (Universal
    /// Receiver) ohne Paarung sendet gar nichts.
    fn shows_use(layout: &Layout, report: &[u8]) -> bool {
        layout
            .decode(report)
            .is_some_and(|decoded| decoded.axes.iter().flatten().any(|axis| *axis != 0))
    }

    /// Nimmt die Achsen aus einem Input-Report, wie der Descriptor ihn legt,
    /// und lässt die übrigen stehen: Translation und Rotation können in
    /// getrennten Reports kommen (kabellierte 3Dconnexion-Geräte) oder in
    /// einem (kabellose Geräte, die meisten selbstgebauten).
    fn decode_report(report: &[u8], layout: Layout, dest: &mut Reading, now: Instant) {
        let Some(decoded) = layout.decode(report) else {
            return;
        };
        for ((value, at), new) in dest.axes.iter_mut().zip(dest.at.iter_mut()).zip(decoded.axes) {
            if let Some(new) = new {
                *value = new;
                *at = now;
            }
        }
    }

    /// Meldet einen Input-Report an den egui-Thread. Ein Report, der ebenso
    /// liest wie der, den er ersetzt, verlangt kein Bild (Geräte wiederholen
    /// ihren Report in Ruhe); gleichzeitig frischt er die Timestamps der
    /// gesendeten Achsen auf, so dass sie frisch bleibt.
    fn publish(ctx: &Context, shared: &Mutex<Reading>, layout: Layout, report: &[u8]) {
        if DRIVER_CONNECTED.load(Ordering::Relaxed) {
            return;
        }
        if let Ok(mut reading) = shared.lock() {
            let before = reading.axes;
            let now = Instant::now();
            decode_report(report, layout, &mut reading, now);
            if reading.axes != before {
                ctx.request_repaint_after(REPAINT_EVERY);
            }
        }
    }

    /// Das Gerät ist fort: aufhören, seine letzte Position zu melden.
    fn reset(ctx: &Context, shared: &Mutex<Reading>) {
        if let Ok(mut reading) = shared.lock() {
            *reading = Reading::new(Instant::now());
        }
        ctx.request_repaint();
    }

    /// Wie das Gerät seine Achsen packt, aus seinem Report-Descriptor. Ohne
    /// Descriptor (nicht lesbar oder leer) bekommt ein 3Dconnexion-Gerät
    /// (Logitechs ID 0x046d für die alte SpaceNavigator, 3Dconnexions 0x256f
    /// für den Rest) 3Dconnexions Standard-Layout; alles andere bleibt
    /// unausgesprochen statt geraten.
    fn device_layout(device: &HidDevice, vendor_id: u16) -> Option<Layout> {
        let mut descriptor = [0_u8; hidapi::MAX_REPORT_DESCRIPTOR_SIZE];
        let read = device.get_report_descriptor(&mut descriptor);
        if let Ok(size @ 1..) = read {
            return Descriptor::parse(descriptor.get(..size).unwrap_or_default()).layout();
        }
        let why = match read {
            Err(error) => error.to_string(),
            Ok(_) => "leer".to_string(),
        };
        if matches!(vendor_id, 0x046d | 0x256f) {
            eprintln!("3D-Maus: kein Report-Descriptor ({why}), Standardlayout angenommen");
            Some(Layout::default())
        } else {
            eprintln!("3D-Maus: Multiaxis-Controller ohne Report-Descriptor ausgelassen ({why})");
            None
        }
    }

    /// Die Pfade der Interfaces, die 3D-Mäuse sein können, aus hidapis
    /// Enummerierung (IOKits Registry).
    fn candidates(api: &mut HidApi) -> Vec<CString> {
        if let Err(error) = api.refresh_devices() {
            eprintln!("3D-Maus: Geräteliste fehlgeschlagen: {error}");
            return Vec::new();
        }
        api.device_list()
            .filter(|info| is_3d_mouse(info))
            .map(|info| info.path().to_owned())
            .collect()
    }

    /// Platz für einen Input-Report (4096 Bytes Descriptor-Platz plus die
    /// Report-ID); ein kürzerer Puffer stutzt einen längeren Report ab, und
    /// ein gestutzter Report fällt ganz.
    const REPORT_BUFFER_BYTES: usize = 4096 + 1;
    /// Eine Runde Pause, bevor wieder enummeriert wird.
    const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(2);

    /// Liest alle Kandidaten und folgt dem gerade benutzten. Der erste wird
    /// von Anfang an gelesen; ein anderer übernimmt, sobald er sich bewegt,
    /// während der gelesene sich seit `STALE_AFTER` nicht mehr bewegt hat
    /// (zwei Geräte, die zugleich bewegt werden, reichen das Lesen nicht hin
    /// und her — z. B. ein Empfänger, der sich öffnet und nie scheitert, darf
    /// eine SpaceMouse daneben nie verdrängen). Reports des gelesenen Geräts
    /// — auch seine Nullen in Ruhe — melden sich, so lange das Lesen läuft;
    /// daraus altert jedes Achse. Ein Kandidat mit Fehler fällt weg; war es
    /// der gelesene, endet das Lesen. Läuft leer, wenn kein Kandidat mehr
    /// übrig ist.
    fn follow(ctx: &Context, shared: &Mutex<Reading>, mut candidates: Vec<Candidate>) {
        let mut followed = 0usize;
        // Wann sich das gelesene Gerät zuletzt bewegt hat.
        let mut used_at: Option<Instant> = None;
        let mut report = [0_u8; REPORT_BUFFER_BYTES];
        while !candidates.is_empty() {
            // Ein Gerät wird durchgehend gewartet; mehrere wechseln sich kurz,
            // sodass keiner hinter einem untätigen wartet.
            let timeout_ms = if candidates.len() == 1 { 100 } else { 10 };
            let mut index = 0;
            while index < candidates.len() {
                match candidates[index].device.read_timeout(&mut report, timeout_ms) {
                    Ok(0) => {}
                    Ok(size) => {
                        let bytes = report.get(..size).unwrap_or_default();
                        let now = Instant::now();
                        if shows_use(&candidates[index].layout, bytes) {
                            let stale = used_at.is_none_or(|used| now.duration_since(used) >= STALE_AFTER);
                            if index != followed && stale {
                                followed = index;
                                used_at = Some(now);
                            }
                            if index == followed {
                                used_at = Some(now);
                            }
                        }
                        if index == followed {
                            publish(ctx, shared, candidates[index].layout, bytes);
                        }
                    }
                    Err(_) => {
                        // Gerät fort (Unpluggen &c.).
                        candidates.remove(index);
                        if index < followed {
                            followed -= 1;
                            continue;
                        }
                        if index == followed {
                            followed = 0;
                            used_at = None;
                            reset(ctx, shared);
                        }
                        continue;
                    }
                }
                index += 1;
            }
        }
        reset(ctx, shared);
    }

    /// Der Leser: enummeriert, öffnet, liest, folgt dem benutzten Gerät;
    /// wenn keins mehr übrig ist, erst nach einer Pause wieder.
    pub(super) fn reader(ctx: Context, shared: Arc<Mutex<Reading>>) {
        let Ok(mut api) = HidApi::new() else {
            eprintln!("3D-Maus: HID-Initialisierung fehlgeschlagen");
            return;
        };
        loop {
            let open: Vec<Candidate> = candidates(&mut api)
                .into_iter()
                .filter_map(|path| {
                    let device = api.open_path(&path).inspect_err(|error| {
                        // macOS: fehlende Erlaubnis (Input Monitoring), oder
                        // der 3DxWare-Helfer hält das Gerät — mit dem Driver
                        // ist das kein Problem, sondern sein Zustand.
                        if !DRIVER_CONNECTED.load(Ordering::Relaxed)
                            && !DRIVER_REGISTERED.load(Ordering::Relaxed)
                        {
                            eprintln!("3D-Maus: Gerät nicht zu öffnen: {error}");
                        }
                    })
                    .ok()?;
                    let info = device.get_device_info().ok()?;
                    let layout = device_layout(&device, info.vendor_id())?;
                    Some(Candidate { device, layout })
                })
                .collect();
            follow(&ctx, &shared, open);
            std::thread::sleep(RETRY_AFTER);
        }
    }

    /// macOS: der 3Dconnexion-Driver meldet den Deckel an angemeldete
    /// Clients — Achsen nach den Einstellungen des Benutzers im 3DxWare.
    pub(super) fn start_driver_client(ctx: &Context, shared: Arc<Mutex<Reading>>) {
        use solidcraft_3dx_client::Event;
        // In den Callbacks, die der Client-Thread ruft.
        let (ctx, shared) = (ctx.clone(), shared);
        let started = solidcraft_3dx_client::start(
            "Rohteilcalc",
            move |event| match event {
                Event::DeviceAdded => {
                    DRIVER_CONNECTED.store(true, Ordering::Relaxed);
                }
                Event::DeviceRemoved => {
                    DRIVER_CONNECTED.store(false, Ordering::Relaxed);
                    reset(&ctx, &shared);
                }
                Event::Axes { translation, rotation } => {
                    if let Ok(mut reading) = shared.lock() {
                        reading.axes = [
                            translation[0],
                            translation[1],
                            translation[2],
                            rotation[0],
                            rotation[1],
                            rotation[2],
                        ];
                        reading.at = [Instant::now(); 6];
                        ctx.request_repaint_after(REPAINT_EVERY);
                    }
                }
                Event::Buttons(_) => {}
            },
            |connected| {
                match connected {
                    Ok(()) => DRIVER_REGISTERED.store(true, Ordering::Relaxed),
                    Err(error) => eprintln!("3D-Maus: kein Driver-Client: {error}"),
                };
            },
        );
        if let Err(error) = started {
            eprintln!("3D-Maus: kein Driver-Client: {error}");
        }
    }
}

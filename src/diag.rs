//! Startup-Diagnose: crash-sichere Logdatei (`%TEMP%\Rohteilcalc.log`),
//! Phase-Marker, Adapter-Survey für `--diag`, GPU-Fehler-Log (Fix 2/3/4).
//!
//! Std-only: jeder Schreibzugriff auf die Logdatei hält sich an
//! append+flush, sodass auch aborts/harte Abstürze das Log überleben.
//! Logfehler stürzen den Start nie ab.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Logdatei: `std::env::temp_dir().join("Rohteilcalc.log")`
/// (auf Windows: `%TEMP%\Rohteilcalc.log`).
pub fn log_path() -> PathBuf {
    std::env::temp_dir().join("Rohteilcalc.log")
}

/// Das vorherige Log (Rotation): `Rohteilcalc.prev.log` im selben Verzeichnis.
pub fn prev_log_path() -> PathBuf {
    std::env::temp_dir().join("Rohteilcalc.prev.log")
}

/// Epoch-Millisekunden-Zeitstempel.
pub fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Eine Zeile append + flush an die Logdatei (Best-Effort).
pub fn log_line(msg: &str) {
    let mut file = match open_log() {
        Some(file) => file,
        None => return,
    };
    let _ = writeln!(file, "{msg}");
    let _ = file.flush();
}

fn open_log() -> Option<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
        .ok()
}

/// Vorhandenes Log rotieren: `Rohteilcalc.log` → `Rohteilcalc.prev.log`.
/// Best-effort (Fehler werden ignoriert).
pub fn rotate_log() {
    let _ = std::fs::rename(log_path(), prev_log_path());
}

/// Start-Log aufbauen: vorhandenes Log rotieren, dann die erste Zeile
/// schreiben — Version, Exe, Args, OS/Arch, relevante Env-Variablen,
/// DXC-DLL-Präsenz und die gewählte `dx12_compiler()`-Wahl (Fix 2).
pub fn log_startup() {
    rotate_log();
    let exe = current_exe_display();
    log_line(&format!(
        "[{}] Startup: version={} exe={} args={:?} os_arch={}/{} env={:?} dxc_dlls={:?} dx12_compiler={:?}",
        now_millis(),
        env!("CARGO_PKG_VERSION"),
        exe,
        std::env::args().skip(1).collect::<Vec<_>>(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        env_lines(),
        dxc_dll_lines(Path::new(&exe)),
        solidcraft_gpu::dx12_compiler(),
    ));
}

/// Exe-Pfad als String (best-effort).
fn current_exe_display() -> String {
    std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "(exepfad unbekannt)".to_string())
}

/// Relevante Umgebungsvariablen: `WGPU_BACKEND`, `WGPU_DX12_COMPILER`,
/// `SOLIDCRAFT_GPU` — `key=value` oder `key=(nicht gesetzt)`.
pub fn env_lines() -> Vec<String> {
    ["WGPU_BACKEND", "WGPU_DX12_COMPILER", "SOLIDCRAFT_GPU"]
        .into_iter()
        .map(|key| match std::env::var(key) {
            Ok(value) => format!("{key}={value}"),
            Err(_) => format!("{key}=(nicht gesetzt)"),
        })
        .collect()
}

/// Zeilen zur Präsenz von `dxcompiler.dll`/`dxil.dll` beside `exe`.
pub fn dxc_dll_lines(exe: &Path) -> Vec<String> {
    ["dxcompiler.dll", "dxil.dll"]
        .into_iter()
        .map(|name| {
            let path = exe
                .parent()
                .map(|dir| dir.join(name))
                .unwrap_or_else(|| PathBuf::from(name));
            format!("{name}: {} ({})", bool_str(path.is_file()), path.display())
        })
        .collect()
}

fn bool_str(b: bool) -> &'static str {
    if b {
        "ja"
    } else {
        "nein"
    }
}

// ---------------------------------------------------------
// --diag (Fix 4): Adapter-Survey + Geräteprobe
// ---------------------------------------------------------

/// Adapter-Aufzählung: Instanz aus `solidcraft_gpu::instance_descriptor_from_env()`
/// mit ALLEN kompilierten Backends (`WGPU_BACKEND` schließt sonst Adapter aus).
pub fn enumerate_adapters() -> Vec<wgpu::Adapter> {
    let mut desc = solidcraft_gpu::instance_descriptor_from_env();
    desc.backends = wgpu::Backends::all();
    let instance = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wgpu::Instance::new(desc)
    })) {
        Ok(instance) => instance,
        Err(p) => {
            let msg = format!("Instance::new panicked: {p:?}");
            log_line(&msg);
            return Vec::new();
        }
    };
    pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()))
}

/// Ein Adapter-Eintrag für den `--diag`-Bericht: Info-Zeile, Limits, Probe.
#[derive(Clone, Debug)]
pub struct AdapterEntry {
    pub info_line: String,
    pub limits_line: String,
    pub probe: String,
}

/// Alle I/O für den `--diag`-Bericht: Version, Exe, OS/Arch, DXC-DLLs,
/// DX12-Compiler-Wahl, Env, Adapter-Liste (Info+Limits+Probe).
#[derive(Clone, Debug)]
pub struct DiagData {
    pub version: String,
    pub exe: String,
    pub os_arch: String,
    pub dlls: Vec<String>,
    pub dx12_compiler: String,
    pub env: Vec<String>,
    pub adapters: Vec<AdapterEntry>,
    pub warning: Option<String>,
}

/// Human-readable `--diag`-Bericht (reine Formatierung — unit-testbar).
pub fn format_diag_report(data: &DiagData) -> String {
    let mut out = String::new();
    out.push_str("RohteilCalc Diagnose-Bericht\n");
    out.push_str(&format!("Version:              {}\n", data.version));
    out.push_str(&format!("Exe:                  {}\n", data.exe));
    out.push_str(&format!("Betriebssystem/Arch:  {}\n", data.os_arch));
    for dll in &data.dlls {
        out.push_str(&format!("DXC-DLL:              {dll}\n"));
    }
    out.push_str(&format!("DX12-Shader-Compiler: {}\n", data.dx12_compiler));
    for line in &data.env {
        out.push_str(&format!("Umgebung:             {line}\n"));
    }
    if data.adapters.is_empty() {
        out.push_str("Adapter:              keine gefunden\n");
        if let Some(warning) = &data.warning {
            out.push_str(&format!("Warnung:              {warning}\n"));
        }
    } else {
        for (i, adapter) in data.adapters.iter().enumerate() {
            out.push_str(&format!(
                "Adapter {i}:                 {}\n",
                adapter.info_line
            ));
            out.push_str(&format!(
                "  Limits:               {}\n",
                adapter.limits_line
            ));
            out.push_str(&format!("  Geräte-Probe:         {}\n", adapter.probe));
        }
    }
    out
}

/// Info-Zeile eines Adapters: name, vendor/device-IDs, typ, backend,
/// Treiber — aus `AdapterInfo` (`name`, `vendor`, `device`, `device_type`,
/// `backend`, `driver_info`).
fn adapter_entry(adapter: &wgpu::Adapter) -> AdapterEntry {
    let info = adapter.get_info();
    let driver = [info.driver.as_str(), info.driver_info.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let info_line = format!(
        "{} (vendor: 0x{:04X}, device: 0x{:04X}, typ: {:?}, backend: {:?}, treiber: {})",
        info.name, info.vendor, info.device, info.device_type, info.backend, driver
    );
    let limits = adapter.limits();
    let limits_line = format!(
        "max_texture_dimension_2d: {}, max_compute_workgroup_size_x: {}, max_compute_workgroups_per_dimension: {}",
        limits.max_texture_dimension_2d,
        limits.max_compute_workgroup_size_x,
        limits.max_compute_workgroups_per_dimension,
    );
    let probe = match probe_device(adapter) {
        Ok((device, _queue)) => format!("Ok (features: {:?})", device.features()),
        Err(err) => format!("Err({err})"),
    };
    AdapterEntry {
        info_line,
        limits_line,
        probe,
    }
}

/// Geräteprobe, gespiegelt auf `solidcraft_gpu::open_device`:
/// `pollster::block_on(adapter.request_device(&desc))` mit den Downlevel-
/// Limits + depth-Features. Dies ist der "kann dieser PC den Renderer
/// starten"-Test.
fn probe_device(
    adapter: &wgpu::Adapter,
) -> Result<(wgpu::Device, wgpu::Queue), wgpu::RequestDeviceError> {
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rohteilcalc-diag"),
        required_features: solidcraft_gpu::depth_features(adapter),
        required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
        ..Default::default()
    }))
}

/// Alle I/O für `--diag` sammeln: Adapter-Aufzählung und Proben, jede
/// Adapter-Behandlung einzeln abgesichert (Panic → Eintrag mit der Panic-
/// Botschaft, weitere Adapter weiter bearbeiten).
pub fn gather_diag() -> DiagData {
    let exe = current_exe_display();
    let mut adapters = Vec::new();
    for adapter in enumerate_adapters() {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| adapter_entry(&adapter))) {
            Ok(entry) => adapters.push(entry),
            Err(p) => {
                let msg = format!("Adapter-Panic: {p:?}");
                log_line(&msg);
                adapters.push(AdapterEntry {
                    info_line: msg.clone(),
                    limits_line: "-".into(),
                    probe: "Panic".into(),
                });
            }
        }
    }
    DiagData {
        version: env!("CARGO_PKG_VERSION").to_string(),
        exe: exe.clone(),
        os_arch: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        dlls: dxc_dll_lines(Path::new(&exe)),
        dx12_compiler: format!("{:?}", solidcraft_gpu::dx12_compiler()),
        env: env_lines(),
        adapters,
        warning: None,
    }
}

/// Vollständiger `--diag`-Bericht. Exit-Code-Regel: `Ok` — der Bericht
/// wurde produziert (auch bei 0 Adaptern, dann Warnung); `Err` —
/// interner Fehler (Enumeration abgestürzt).
pub fn diag_report() -> Result<String, String> {
    let mut data = std::panic::catch_unwind(gather_diag)
        .map_err(|_| "Adapter-Enumeration abgestürzt (interner Fehler)".to_string())?;
    if data.adapters.is_empty() {
        data.warning = Some(format!(
            "Keine Adapter gefunden (Backends: {:?}) – der Renderer braucht mindestens einen Adapter.",
            wgpu::Backends::all()
        ));
    }
    Ok(format_diag_report(&data))
}

/// `--diag` ausführen: Bericht auf stdout UND ins Log (Fix 4).
pub fn run_diag() -> bool {
    match diag_report() {
        Ok(report) => {
            println!("{report}");
            println!("Logdatei: {}", log_path().display());
            log_line(&format!("Diagnose-Bericht:\n{report}"));
            true
        }
        Err(err) => {
            eprintln!("--diag fehlgeschlagen: {err}");
            log_line(&format!("--diag fehlgeschlagen: {err}"));
            false
        }
    }
}

/// Adapter + Target-Format im Creation-Callback loggen (Fix 2, Phase:
/// "creating eframe window") — der echte Adapter aus
/// `cc.wgpu_render_state` (`adapter.get_info()`).
pub fn log_creation_callback(cc: &eframe::CreationContext<'_>) {
    let Some(rs) = cc.wgpu_render_state.as_ref() else {
        return;
    };
    let info = rs.adapter.get_info();
    log_line(&format!(
        "Phase: eframe CreationCallback — Adapter name={} vendor={} device={} typ={:?} backend={:?} treiber={}",
        info.name, info.vendor, info.device, info.device_type, info.backend, info.driver_info
    ));
    log_line(&format!(
        "Phase: eframe CreationCallback — Target format: {:?}",
        rs.target_format
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_rotation_naming() {
        assert_eq!(
            log_path().file_name(),
            Some(std::ffi::OsStr::new("Rohteilcalc.log"))
        );
        assert_eq!(
            prev_log_path().file_name(),
            Some(std::ffi::OsStr::new("Rohteilcalc.prev.log"))
        );
        // Rotation liegt im selben Verzeichnis.
        assert_eq!(log_path().parent(), prev_log_path().parent());
    }

    #[test]
    fn dxc_dll_zeilen_format() {
        let lines = dxc_dll_lines(Path::new("/app/Rohteilcalc.exe"));
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("dxcompiler.dll: "));
        assert!(lines[1].starts_with("dxil.dll: "));
        assert!(lines[0].contains("dxcompiler.dll"));
        assert!(lines[1].contains("dxil.dll"));
    }

    #[test]
    fn diag_bericht_formatierung() {
        let data = DiagData {
            version: "0.1.0".into(),
            exe: "/dist/Rohteilcalc.exe".into(),
            os_arch: "windows/x86_64".into(),
            dlls: vec!["dxcompiler.dll: ja".into(), "dxil.dll: nein".into()],
            dx12_compiler: "DynamicDxc { dxc_path: \"/dist/dxcompiler.dll\" }".into(),
            env: vec!["WGPU_BACKEND=(nicht gesetzt)".into()],
            adapters: vec![AdapterEntry {
                info_line: "NVIDIA RTX 4090 (vendor: 0x10DE, device: 0x2684, typ: DiscreteGpu, backend: Vulkan, treiber: NVIDIA)".into(),
                limits_line: "max_texture_dimension_2d: 16384".into(),
                probe: "Ok".into(),
            }],
            warning: None,
        };
        let report = format_diag_report(&data);
        assert!(report.contains("Version:"));
        assert!(report.contains("0.1.0"));
        assert!(report.contains("Exe:"));
        assert!(report.contains("/dist/Rohteilcalc.exe"));
        assert!(report.contains("Betriebssystem/Arch:"));
        assert!(report.contains("windows/x86_64"));
        assert!(report.contains("dxcompiler.dll: ja"));
        assert!(report.contains("dxil.dll: nein"));
        assert!(report.contains("DX12-Shader-Compiler: DynamicDxc"));
        assert!(report.contains("WGPU_BACKEND=(nicht gesetzt)"));
        assert!(report.contains("Adapter 0:"));
        assert!(report.contains("NVIDIA RTX 4090"));
        assert!(report.contains("Geräte-Probe:"));
        assert!(report.contains("Ok"));

        // Keine-Adapter-Fall: Warnung drucken.
        let empty = DiagData {
            adapters: Vec::new(),
            warning: Some("Keine Adapter gefunden (Backends: all)".into()),
            ..data.clone()
        };
        let report_empty = format_diag_report(&empty);
        assert!(report_empty.contains("Adapter:"));
        assert!(report_empty.contains("keine gefunden"));
        assert!(report_empty.contains("Keine Adapter gefunden"));
    }
}

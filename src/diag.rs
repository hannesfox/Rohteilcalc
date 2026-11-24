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
    log_startup_line();
}

/// Dieselbe Start-Zeile **ohne** Rotation. Rückfall-Kindprozesse und
/// `--diag`-Kinder dürfen die Meldungen der vorherigen Versuche nicht
/// wegrotieren — sonst fehlt im Fehlerfall genau die erste Ursache.
pub fn log_startup_line() {
    let exe = current_exe_display();
    log_line(&format!(
        "[{}] Startup: version={} exe={} args={:?} os={} os_arch={}/{} env={:?} dxc_dlls={:?} dx12_compiler={:?} compiled_backends={:?}",
        now_millis(),
        env!("CARGO_PKG_VERSION"),
        exe,
        std::env::args().skip(1).collect::<Vec<_>>(),
        os_version(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        env_lines(),
        dxc_dll_lines(Path::new(&exe)),
        crate::gpu::dx12_compiler(),
        crate::gpu::compiled_backends(),
    ));
}

/// Exe-Pfad als String (best-effort).
fn current_exe_display() -> String {
    std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "(exepfad unbekannt)".to_string())
}

/// Relevante Umgebungsvariablen — `key=value` oder `key=(nicht gesetzt)`.
pub fn env_lines() -> Vec<String> {
    [
        "WGPU_BACKEND",
        "WGPU_DX12_COMPILER",
        "SOLIDCRAFT_GPU",
        crate::gpu::RETRY_ENV,
        crate::gpu::DIAG_ENV,
    ]
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

/// Betriebssystem inklusive Version — Windows aus der Registry (damit auch
/// Windows 11 nicht als "Windows 10" durch `GetVersionEx` gemeldet wird),
/// macOS über `sw_vers`, Linux über `/etc/os-release`. Best-Effort.
pub fn os_version() -> String {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_LOCAL_MACHINE, KEY_READ,
        };
        let key_name: Vec<u16> = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\0"
            .encode_utf16()
            .collect();
        let mut key: windows_sys::Win32::System::Registry::HKEY = std::ptr::null_mut();
        unsafe {
            if RegOpenKeyExW(HKEY_LOCAL_MACHINE, key_name.as_ptr(), 0, KEY_READ, &mut key) != 0 {
                return "Windows (Version unbekannt)".to_string();
            }
            let mut parts = Vec::new();
            for value in ["ProductName", "DisplayVersion", "CurrentBuild"] {
                let name: Vec<u16> = format!("{value}\0").encode_utf16().collect();
                let mut len = 0_u32;
                if RegQueryValueExW(
                    key,
                    name.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut len,
                ) != 0
                {
                    continue;
                }
                let mut buf = vec![0_u8; len as usize + 2];
                let mut kind = 0_u32;
                if RegQueryValueExW(
                    key,
                    name.as_ptr(),
                    std::ptr::null(),
                    &mut kind,
                    buf.as_mut_ptr(),
                    &mut len,
                ) == 0
                {
                    let words = std::slice::from_raw_parts(buf.as_ptr() as *const u16, len as usize / 2);
                    let text = String::from_utf16_lossy(words)
                        .trim_end_matches('\0')
                        .trim()
                        .to_string();
                    if !text.is_empty() {
                        parts.push(text);
                    }
                }
            }
            RegCloseKey(key);
            if parts.is_empty() {
                "Windows (Version unbekannt)".to_string()
            } else {
                parts.join(" ")
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        match std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
        {
            Ok(out) if out.status.success() => {
                format!("macOS {}", String::from_utf8_lossy(&out.stdout).trim())
            }
            _ => "macOS (Version unbekannt)".to_string(),
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let text = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        let pretty = text
            .lines()
            .find_map(|line| line.strip_prefix("PRETTY_NAME="))
            .map(|v| v.trim_matches('"').to_string())
            .unwrap_or_default();
        if pretty.is_empty() {
            "Linux (Version unbekannt)".to_string()
        } else {
            pretty
        }
    }
}

/// DLL-Importe einer PE-Datei (die eigenen `.exe`) — ohne externe Tools.
/// Damit ist im `--diag`-Bericht nachvollziehbar, welche DLLs das Paket
/// wirklich braucht. Delay-Imports zählen mit. Bei Fehlern: leere Liste.
pub fn pe_imports(path: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let u16at = |off: usize| -> Option<u16> {
        let slice = bytes.get(off..off.checked_add(2)?)?;
        Some(u16::from_le_bytes([slice[0], slice[1]]))
    };
    let u32at = |off: usize| -> Option<u32> {
        let slice = bytes.get(off..off.checked_add(4)?)?;
        Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
    };
    let pe = match u32at(0x3c) {
        Some(v) => v as usize,
        None => return Vec::new(),
    };
    let Some(sig_end) = pe.checked_add(4) else {
        return Vec::new();
    };
    if bytes.get(pe..sig_end) != Some(b"PE\0\0".as_slice()) {
        return Vec::new();
    }
    let coff = pe + 4;
    let (Some(nsec), Some(opt_size)) = (u16at(coff + 2), u16at(coff + 16)) else {
        return Vec::new();
    };
    let opt = coff + 20;
    let Some(magic) = u16at(opt) else {
        return Vec::new();
    };
    let dirs = opt + if magic == 0x20b { 112 } else { 96 };
    let mut sections = Vec::new();
    for i in 0..nsec as usize {
        let off = opt + opt_size as usize + 40 * i;
        let (Some(vsize), Some(vaddr), Some(raw_size), Some(raw_ptr)) = (
            u32at(off + 8),
            u32at(off + 12),
            u32at(off + 16),
            u32at(off + 20),
        ) else {
            return Vec::new();
        };
        sections.push((vaddr, vsize, raw_ptr, raw_size));
    }
    let rva_to_off = |rva: u32| -> Option<usize> {
        sections.iter().find_map(|&(vaddr, vsize, raw_ptr, raw_size)| {
            let span = vsize.max(raw_size);
            (rva >= vaddr && (rva - vaddr) < span).then(|| raw_ptr as usize + (rva - vaddr) as usize)
        })
    };
    let mut names = Vec::new();
    // 1 = Import Directory, 13 = Delay Import Directory.
    for (dir_index, name_offset) in [(1_usize, 12_usize), (13, 4)] {
        let Some(dir_rva) = u32at(dirs + 8 * dir_index) else {
            continue;
        };
        let Some(mut off) = rva_to_off(dir_rva) else {
            continue;
        };
        for _ in 0..1024 {
            let Some(first_thunk) = u32at(off) else {
                break;
            };
            let Some(name_rva) = u32at(off + name_offset) else {
                break;
            };
            if first_thunk == 0 && name_rva == 0 {
                break;
            }
            let Some(name_off) = rva_to_off(name_rva) else {
                break;
            };
            let Some(end) = bytes[name_off..].iter().position(|&b| b == 0) else {
                break;
            };
            let name = String::from_utf8_lossy(&bytes[name_off..name_off + end]).to_string();
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
            off += if dir_index == 13 { 32 } else { 20 };
        }
    }
    names.sort();
    names
}

/// Zeilen zu den tatsächlichen DLL-Abhängigkeiten der EXE.
pub fn dll_dependency_lines(path: &Path) -> Vec<String> {
    pe_imports(path)
        .into_iter()
        .map(|name| format!("DLL-Abhängigkeit: {name}"))
        .collect()
}

// ---------------------------------------------------------
// --diag (Fix 4): Adapter-Survey + Geräteprobe
// ---------------------------------------------------------

/// Adapter-Aufzählung für genau ein Backend. Wichtig: nie mehrere Windows-
/// APIs gleichzeitig initialisieren (Intels Vulkan-Treiber stürzte dabei ab,
/// siehe `src/gpu.rs`). `catch_unwind` hält einen Treiber-Panic von der
/// Diagnose fern; ein harter Absturz wird durch getrennte `--diag`-Prozesse
/// pro Backend abgefangen (siehe `run_diag`).
pub fn enumerate_adapters(backends: wgpu::Backends) -> Vec<wgpu::Adapter> {
    let mut desc = solidcraft_gpu::instance_descriptor_from_env();
    desc.backends = backends;
    desc.backend_options.dx12.shader_compiler = crate::gpu::dx12_compiler();
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
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pollster::block_on(instance.enumerate_adapters(backends))
    })) {
        Ok(adapters) => adapters,
        Err(p) => {
            log_line(&format!("Adapter-Enumeration panicked: {p:?}"));
            Vec::new()
        }
    }
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
    pub os: String,
    pub os_arch: String,
    pub compiled_backends: String,
    pub selected_backend: String,
    pub dlls: Vec<String>,
    pub dll_dependencies: Vec<String>,
    pub dx12_compiler: String,
    pub env: Vec<String>,
    pub adapters: Vec<AdapterEntry>,
    pub pipeline_error: Option<String>,
    pub errors: Vec<String>,
    pub warning: Option<String>,
}

/// Human-readable `--diag`-Bericht (reine Formatierung — unit-testbar).
pub fn format_diag_report(data: &DiagData) -> String {
    let mut out = String::new();
    out.push_str("RohteilCalc Diagnose-Bericht\n");
    out.push_str(&format!("Version:              {}\n", data.version));
    out.push_str(&format!("Exe:                  {}\n", data.exe));
    out.push_str(&format!("Betriebssystem:       {}\n", data.os));
    out.push_str(&format!("Betriebssystem/Arch:  {}\n", data.os_arch));
    out.push_str(&format!(
        "Kompilierte Backends: {}\n",
        data.compiled_backends
    ));
    out.push_str(&format!("Ausgewähltes Backend: {}\n", data.selected_backend));
    for dll in &data.dlls {
        out.push_str(&format!("DXC-DLL:              {dll}\n"));
    }
    for dll in &data.dll_dependencies {
        out.push_str(&format!("{dll}\n"));
    }
    out.push_str(&format!("DX12-Shader-Compiler: {}\n", data.dx12_compiler));
    for line in &data.env {
        out.push_str(&format!("Umgebung:             {line}\n"));
    }
    for error in &data.errors {
        out.push_str(&format!("Fehler:               {error}\n"));
    }
    if let Some(error) = &data.pipeline_error {
        out.push_str(&format!("Pipeline-Fehler:      {error}\n"));
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

/// Pipeline-Fehler aus dem Creation-Callback für den `--diag`-Bericht merken.
static PIPELINE_ERROR: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Pipeline-Fehler aus einem vorherigen Lauf, der das Log geschrieben hat —
/// auch dann sichtbar, wenn `--diag` in einem neuen Prozess läuft.
fn pipeline_error_from_log() -> Option<String> {
    for path in [log_path(), prev_log_path()] {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if let Some(line) = text
            .lines()
            .rev()
            .find_map(|line| line.strip_prefix("Pipeline-Fehler: "))
        {
            return Some(line.to_string());
        }
    }
    None
}

/// Ein Fehler beim Erstellen der SolidCraft-Render-Pipelines (nie still
/// verschlucken).
pub fn log_pipeline_error(message: &str) {
    let _ = PIPELINE_ERROR.set(message.to_string());
    log_line(&format!("Pipeline-Fehler: {message}"));
}

/// Alle I/O für `--diag` sammeln: genau ein Backend pro Prozess, Adapter und
/// Geräteproben einzeln abgesichert (Panic → Eintrag mit Panic-Meldung).
pub fn gather_diag() -> DiagData {
    let exe = current_exe_display();
    let backends = crate::gpu::diag_backend().unwrap_or_else(|| crate::gpu::startup().0);
    let mut adapters = Vec::new();
    let mut errors = Vec::new();
    for adapter in enumerate_adapters(backends) {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| adapter_entry(&adapter))) {
            Ok(entry) => adapters.push(entry),
            Err(p) => {
                let msg = format!("Adapter-Panic: {p:?}");
                log_line(&msg);
                errors.push(msg.clone());
                adapters.push(AdapterEntry {
                    info_line: msg,
                    limits_line: "-".into(),
                    probe: "Panic".into(),
                });
            }
        }
    }
    DiagData {
        version: env!("CARGO_PKG_VERSION").to_string(),
        exe: exe.clone(),
        os: os_version(),
        os_arch: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        compiled_backends: format!("{:?}", crate::gpu::compiled_backends()),
        selected_backend: format!("{} ({backends:?})", crate::gpu::name(backends)),
        dlls: dxc_dll_lines(Path::new(&exe)),
        dll_dependencies: dll_dependency_lines(Path::new(&exe)),
        dx12_compiler: format!("{:?}", crate::gpu::dx12_compiler()),
        env: env_lines(),
        adapters,
        pipeline_error: PIPELINE_ERROR.get().cloned().or_else(pipeline_error_from_log),
        errors,
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
            "Keine Adapter für {} gefunden – der Renderer braucht mindestens einen Adapter.",
            data.selected_backend
        ));
    }
    Ok(format_diag_report(&data))
}

/// Text auf stdout ausgeben – best effort: Auf Windows-Release ohne
/// Konsolen-Subsystem ist stdout eine leere Pipe ("The pipe is being
/// closed", os error 232), und `println!` paniziert. Das Log ist die
/// zuverlässige Ausgabe (Fix 2/4).
pub fn print_safe(text: &str) {
    use std::io::Write;
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
}

/// `--diag` ausführen: Bericht auf stdout UND ins Log. Auf Windows werden
/// die weiteren kompilierten APIs in je einem **eigenen Prozess** abgefragt:
/// ein Absturz im Vulkan-Treiber darf die eigene Diagnose nicht mitnehmen.
pub fn run_diag() -> bool {
    let ok = match diag_report() {
        Ok(report) => {
            print_safe(&format!("{report}\nLogdatei: {}\n", log_path().display()));
            log_line(&format!("Diagnose-Bericht:\n{report}"));
            true
        }
        Err(err) => {
            print_safe(&format!("--diag fehlgeschlagen: {err}\n"));
            log_line(&format!("--diag fehlgeschlagen: {err}"));
            false
        }
    };
    if crate::gpu::diag_backend().is_some() || !cfg!(target_os = "windows") {
        return ok;
    }
    let primary = crate::gpu::name(crate::gpu::startup().0);
    for (label, _) in crate::gpu::survey_plan() {
        if label == primary {
            continue;
        }
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(err) => {
                log_line(&format!("Backend-Survey {label}: Exe unbekannt: {err}"));
                print_safe(&format!("Warnung: Backend-Survey {label} nicht möglich: {err}\n"));
                continue;
            }
        };
        match std::process::Command::new(exe)
            .arg("--diag")
            .env(crate::gpu::DIAG_ENV, label)
            .output()
        {
            Ok(out) => {
                print_safe(&format!(
                    "\n--- Backend-Survey: {label} ---\n{}\n",
                    String::from_utf8_lossy(&out.stdout)
                ));
                if !out.status.success() {
                    // Ein Treiberabsturz in einem *anderen* API-Zweig darf
                    // die eigene Diagnose nicht als gescheitert markieren:
                    // er wird ausdrücklich gemeldet und protokolliert.
                    let msg = format!(
                        "Backend-Survey {label}: Exit-Code {} (Treiber abgestürzt?)",
                        out.status
                    );
                    log_line(&msg);
                    print_safe(&format!("Warnung: {msg}\n"));
                }
            }
            Err(err) => {
                let msg = format!("Backend-Survey {label} nicht möglich: {err}");
                log_line(&msg);
                print_safe(&format!("Warnung: {msg}\n"));
            }
        }
    }
    ok
}

/// Adapter, Hersteller/Gerät, ausgewähltes Backend und Target-Format im
/// Creation-Callback loggen — der echte Adapter aus
/// `cc.wgpu_render_state` (`adapter.get_info()`).
pub fn log_creation_callback(cc: &eframe::CreationContext<'_>, backend: &str) {
    log_line(&format!("Ausgewähltes Backend: {backend}"));
    let Some(rs) = cc.wgpu_render_state.as_ref() else {
        log_line("Fehler: kein wgpu-RenderState im Creation-Callback");
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

    /// Wenn die CI `PE_TEST_EXE` auf die gebaute EXE setzt, wird der
    /// Import-Parser gegen eine echte PE-Datei geprüft.
    #[test]
    fn pe_imports_der_exe() {
        if let Ok(path) = std::env::var("PE_TEST_EXE") {
            let names = pe_imports(Path::new(&path));
            println!("PE imports: {names:?}");
            assert!(names.contains(&"kernel32.dll".to_string()));
            assert!(!names.iter().any(|n| n.starts_with("vcruntime")));
        }
    }

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
            os: "Windows 11 Pro 24H2 26100".into(),
            os_arch: "windows/x86_64".into(),
            compiled_backends: "DX12 | VULKAN | GL".into(),
            selected_backend: "dx12 (DX12)".into(),
            dlls: vec!["dxcompiler.dll: nein".into(), "dxil.dll: nein".into()],
            dll_dependencies: vec!["DLL-Abhängigkeit: dxgi.dll".into()],
            dx12_compiler: "Fxc".into(),
            env: vec!["WGPU_BACKEND=(nicht gesetzt)".into()],
            adapters: vec![AdapterEntry {
                info_line: "NVIDIA RTX 4090 (vendor: 0x10DE, device: 0x2684, typ: DiscreteGpu, backend: Dx12, treiber: NVIDIA)".into(),
                limits_line: "max_texture_dimension_2d: 16384".into(),
                probe: "Ok (features: 0)".into(),
            }],
            pipeline_error: None,
            errors: Vec::new(),
            warning: None,
        };
        let report = format_diag_report(&data);
        assert!(report.contains("Version:"));
        assert!(report.contains("0.1.0"));
        assert!(report.contains("Exe:"));
        assert!(report.contains("/dist/Rohteilcalc.exe"));
        assert!(report.contains("Betriebssystem:"));
        assert!(report.contains("Windows 11 Pro 24H2 26100"));
        assert!(report.contains("Betriebssystem/Arch:"));
        assert!(report.contains("windows/x86_64"));
        assert!(report.contains("Kompilierte Backends:"));
        assert!(report.contains("Ausgewähltes Backend:"));
        assert!(report.contains("dx12 (DX12)"));
        assert!(report.contains("dxcompiler.dll: nein"));
        assert!(report.contains("dxil.dll: nein"));
        assert!(report.contains("DLL-Abhängigkeit: dxgi.dll"));
        assert!(report.contains("DX12-Shader-Compiler: Fxc"));
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

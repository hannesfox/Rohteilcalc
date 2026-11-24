//! Grafik-Backend-Richtlinie — bewusst an SolidCrafts Startup-Strategie
//! angelehnt (Tag `v0.40.0-rc.3`, `apps/solidcraft/src/startup.rs`):
//!
//! * Windows startet **nur DX12**, wenn es dafür einen Adapter gibt (auch
//!   WARP zählt). Vulkan und GLES werden nicht zusammen mit DX12
//!   initialisiert. Der Grund ist dokumentiert in SolidCrafts
//!   `docs/book/src/guide/troubleshooting.md`: wgpu startete früher
//!   Vulkan + DX12 + GLES gleichzeitig und lud dabei *jeden* installierten
//!   Vulkan-Treiber; Intels `igvk64.dll` stürzte dabei vor jeder Fehlermeldung
//!   ab. Genau dieses Muster hatte die Standardkonfiguration von eframe 0.36
//!   (`Backends::PRIMARY | Backends::GL`) auch in Rohteilcalc.
//! * Ein Wechsel des Backends passiert in einem **neuen Prozess** (wie
//!   SolidCrafts `SOLIDCRAFT_STARTUP_BACKEND`), nie durch erneutes Aufrufen
//!   von `eframe::run_native` im selben Prozess.
//! * `WGPU_BACKEND` bleibt die ausdrückliche Nutzerwahl und hat immer Vorrang.

use wgpu::Backends;

/// Interner Rückfall-Schalter, nur für den von uns neu gestarteten Prozess.
pub const RETRY_ENV: &str = "ROHTEILCALC_STARTUP_BACKEND";
/// `--diag` soll einen einzelnen Backend-Zweig abfragen dürfen, ohne dass ein
/// Treiberabsturz die gesamte Sammlung mitnimmt.
pub const DIAG_ENV: &str = "ROHTEILCALC_DIAG_BACKEND";

/// Auf dieser Plattform tatsächlich kompilierte Backends (Cargo.toml).
pub fn compiled_backends() -> Backends {
    if cfg!(target_os = "windows") {
        Backends::DX12 | Backends::VULKAN | Backends::GL
    } else if cfg!(target_os = "macos") {
        Backends::METAL
    } else {
        Backends::VULKAN | Backends::GL
    }
}

/// Lesbarer Name für ein einzelnes Backend.
pub fn name(backends: Backends) -> &'static str {
    if backends.contains(Backends::DX12) {
        "dx12"
    } else if backends.contains(Backends::VULKAN) {
        "vulkan"
    } else if backends.contains(Backends::GL) {
        "gl"
    } else if backends.contains(Backends::METAL) {
        "metal"
    } else {
        "unbekannt"
    }
}

/// Alle Backends, die `--diag` einzeln abfragt (Reihenfolge wie beim Start).
pub fn survey_plan() -> Vec<(&'static str, Backends)> {
    if cfg!(target_os = "windows") {
        vec![
            ("dx12", Backends::DX12),
            ("vulkan", Backends::VULKAN),
            ("gl", Backends::GL),
        ]
    } else {
        vec![(name(compiled_backends()), compiled_backends())]
    }
}

/// Explizite Wahl durch `WGPU_BACKEND` (final, kein automatischer Rückfall).
pub fn explicit() -> bool {
    std::env::var("WGPU_BACKEND").is_ok_and(|value| !value.trim().is_empty())
}

fn env_backends(key: &str) -> Option<Backends> {
    std::env::var(key).ok().map(|v| Backends::from_comma_list(&v)).filter(|b| !b.is_empty())
}

/// `WGPU_BACKEND`, auf die kompilierten Backends begrenzt.
pub fn from_env() -> Option<Backends> {
    env_backends("WGPU_BACKEND").map(|b| b & compiled_backends()).filter(|b| !b.is_empty())
}

/// Für `--diag`: ein einzelnes Backends aus `ROHTEILCALC_DIAG_BACKEND`.
pub fn diag_backend() -> Option<Backends> {
    env_backends(DIAG_ENV).map(|b| b & compiled_backends()).filter(|b| !b.is_empty())
}

/// Ob dieser Prozess ein gezielter Rückfallversuch ist.
pub fn is_retry() -> bool {
    std::env::var(RETRY_ENV).is_ok_and(|value| !value.trim().is_empty())
}

/// Das Backend, mit dem die Anwendung normal startet, plus der Wunsch nach
/// Software-Rendering (WARP). Ohne `WGPU_BACKEND` wird auf Windows ausschließlich
/// DX12 initialisiert -- nie Vulkan, nie GLES, nie Metal.
pub fn startup() -> (Backends, bool) {
    if let Some(backends) = from_env() {
        return (backends, false);
    }
    let software = match std::env::var(RETRY_ENV).as_deref() {
        Ok("dx12-warp") if cfg!(target_os = "windows") => true,
        Ok("vulkan") if cfg!(target_os = "windows") => {
            return (Backends::VULKAN, false);
        }
        Ok("gl") if cfg!(target_os = "windows") => {
            return (Backends::GL, false);
        }
        _ => false,
    };
    let default = if cfg!(target_os = "windows") {
        Backends::DX12
    } else if cfg!(target_os = "macos") {
        Backends::METAL
    } else {
        Backends::VULKAN | Backends::GL
    };
    (default & compiled_backends(), software)
}

/// Der nächste Rückfallversuch nach einem fehlgeschlagenen Start.
/// Windows: DX12 → Vulkan → GLES → DX12 mit WARP-Softwareadapter.
pub fn fallback_after(current: Backends) -> Option<(&'static str, Backends, bool)> {
    if explicit() || is_retry() || !cfg!(target_os = "windows") {
        return None;
    }
    if current.contains(Backends::DX12) {
        Some(("vulkan", Backends::VULKAN, false))
    } else if current.contains(Backends::VULKAN) {
        Some(("gl", Backends::GL, false))
    } else if current.contains(Backends::GL) {
        Some(("dx12-warp", Backends::DX12, true))
    } else {
        None
    }
}

/// Backends für Headless-Rendering (`--render`). `solidcraft_gpu` liest
/// `WGPU_BACKEND` beim Erstellen der Instanz; ohne explizite Wahl setzen wir
/// auf Windows DX12, damit auch dort kein Vulkan-Treiber geladen wird.
pub fn headless_backends() -> Backends {
    if cfg!(target_os = "windows") {
        Backends::DX12
    } else if cfg!(target_os = "macos") {
        Backends::METAL
    } else {
        Backends::VULKAN | Backends::GL
    }
}

/// Solange der Nutzer nichts anderes wählt: Headless auf demselben Backend
/// wie die GUI starten und FXC statt eines zufälligen `dxcompiler.dll` von
/// PATH verwenden (SolidCraft #6756).
pub fn set_headless_defaults() {
    if !explicit() {
        // WGPU_BACKEND ist die von wgpu und solidcraft-gpu gelesene Variable.
        std::env::set_var("WGPU_BACKEND", name(headless_backends()));
    }
    if std::env::var("WGPU_DX12_COMPILER").is_err() {
        std::env::set_var("WGPU_DX12_COMPILER", "fxc");
    }
}

/// Der D3D12-Shadercompiler. Standard ist FXC: er ist in Windows/wgpu
/// enthalten und benötigt keine `dxcompiler.dll`. Ein Nutzer kann mit
/// `WGPU_DX12_COMPILER` jederzeit DXC (auch dynamisch) verlangen; mit dem
/// optionalen Cargo-Feature `static-dxc` wird DXC fest in die EXE gebunden.
pub fn dx12_compiler() -> wgpu::Dx12Compiler {
    if let Ok(setting) = std::env::var("WGPU_DX12_COMPILER") {
        if let Ok(chosen) = setting.trim().parse::<wgpu::Dx12Compiler>() {
            return chosen;
        }
    }
    #[cfg(feature = "static-dxc")]
    {
        wgpu::Dx12Compiler::StaticDxc
    }
    #[cfg(not(feature = "static-dxc"))]
    {
        wgpu::Dx12Compiler::Fxc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_backends_sind_plattformspezifisch() {
        let all = compiled_backends();
        assert!(!all.is_empty());
        assert_eq!(all & !compiled_backends(), Backends::empty());
        if cfg!(target_os = "windows") {
            assert!(all.contains(Backends::DX12));
            assert!(all.contains(Backends::VULKAN));
            assert!(all.contains(Backends::GL));
        }
    }

    #[test]
    fn namen_der_backends() {
        assert_eq!(name(Backends::DX12), "dx12");
        assert_eq!(name(Backends::VULKAN), "vulkan");
        assert_eq!(name(Backends::GL), "gl");
        assert_eq!(name(Backends::METAL), "metal");
    }

    #[test]
    fn startauswahl_und_rueckfall() {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LOCK.lock().unwrap();
        let old_backend = std::env::var("WGPU_BACKEND").ok();
        let old_retry = std::env::var(RETRY_ENV).ok();
        std::env::remove_var("WGPU_BACKEND");
        std::env::remove_var(RETRY_ENV);

        let (default, software) = startup();
        assert_eq!(default, compiled_backends() & default);
        assert!(!software);
        if cfg!(target_os = "windows") {
            assert_eq!(default, Backends::DX12);
            let Some((retry, next, _)) = fallback_after(default) else {
                panic!("Windows braucht einen Rückfall");
            };
            assert_eq!(retry, "vulkan");
            assert_eq!(next, Backends::VULKAN);
        }

        std::env::set_var(RETRY_ENV, "dx12-warp");
        let (warp, software) = startup();
        assert_eq!(software, cfg!(target_os = "windows"));
        if cfg!(target_os = "windows") {
            assert_eq!(warp, Backends::DX12);
        }
        assert!(fallback_after(warp).is_none(), "Retry darf nicht erneut wechseln");

        std::env::remove_var(RETRY_ENV);
        std::env::set_var("WGPU_BACKEND", "vulkan");
        assert!(explicit());
        assert!(fallback_after(Backends::DX12).is_none(), "WGPU_BACKEND ist final");

        match old_backend {
            Some(value) => std::env::set_var("WGPU_BACKEND", value),
            None => std::env::remove_var("WGPU_BACKEND"),
        }
        match old_retry {
            Some(value) => std::env::set_var(RETRY_ENV, value),
            None => std::env::remove_var(RETRY_ENV),
        }
    }

    #[test]
    fn umfrage_der_backends_ist_endlich() {
        let plan = survey_plan();
        assert!(!plan.is_empty());
        for (label, backends) in plan {
            assert!(!label.is_empty());
            assert!(!backends.is_empty());
        }
    }
}

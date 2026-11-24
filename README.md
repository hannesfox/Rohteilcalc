<p align="center">
  <img src="assets/logo.png" width="160" alt="Rohteilcalc Logo">
</p>

<h1 align="center">Rohteilcalc</h1>

<p align="center">
  CNC-Rohteilrechner für STEP-Dateien – Bauteil laden, Rohteilmaße berechnen, fertig.
</p>

<p align="center">
  <a href="https://github.com/hannesfox/Rohteilcalc/actions/workflows/main.yml"><img src="https://github.com/hannesfox/Rohteilcalc/actions/workflows/main.yml/badge.svg" alt="Build"></a>
  <img src="https://img.shields.io/badge/Platform-Windows%20%7C%20macOS-blue" alt="Platform">
  <img src="https://img.shields.io/badge/Lizenz-Apache%202.0-green" alt="Lizenz">
</p>

## Was macht das Programm?

Rohteilcalc liest eine STEP-Datei (`*.step`, `*.stp`) ein, richtet das Bauteil
automatisch an seiner längsten Achse aus und berechnet daraus das passende
Standard-Rohteil. Die 3D-Ansicht zeigt Bauteil und Rohteil-Box direkt
gegenüber.

## Funktionen

- **STEP-Import** inkl. B-rep-Triangulierung (`truck-stepio` / `truck-meshalgo`)
- **Automatische Ausrichtung**: längste Kante → X, mittlere → Y, kürzeste → Z
- **Rohteilberechnung** nach festen Fertigungsregeln
- **GPU-Viewport (wgpu)**: MSAA 4×, Tiefenpuffer, Blinn-Phong-Beleuchtung und
  dicke CAD-Kanten – weiche Rundungen, scharfe Bauteilkanten
- **Darstellungsmodi**: Schattiert, Schattiert + Kanten, Drahtgitter
- **Kamera**: Orbit, Pan, Zoom, Einpassen sowie Vorne/Hinten/Links/Rechts/
  Oben/Unten/Iso
- **Qualitätsstufen**: Niedrig (0,40 mm), Mittel (0,05 mm) und Hoch (0,015 mm)
  Tessellierungs-Toleranz – umschaltbar ohne erneutes Laden der Datei
- **Asynchrones Laden**: STEP-Parsing und Tessellierung laufen im Hintergrund,
  die Oberfläche bleibt bedienbar
- **Bedienung**: STEP über Button laden, per **Drag & Drop** ins Fenster ziehen,
  Kamera mit linker Maustaste drehen, mit dem Mausrad zoomen
- **Fertige Builds**: Windows `.exe` und macOS `.app` (Universal: Apple Silicon + Intel)

## Bedienung

| Aktion | Steuerung |
|--------|-----------|
| Drehen | Linke Maustaste ziehen |
| Verschieben | Rechte oder mittlere Maustaste bzw. Shift + ziehen |
| Zoomen | Mausrad |
| Einpassen | Doppelklick oder Taste `F` |
| Ansichten | Vorne `1`, Oben `2`, Rechts `3`, Iso `4` |
| STEP laden | Button „STEP-Datei öffnen…“ oder Datei ins Fenster ziehen |

## Kommandozeile

```bash
# Geometrie prüfen (Körper, Dreiecke, Bauteil- und Rohteilmaße)
cargo run -- --inspect bauteil.step

# Headless rendern: <datei> <ausgabe.bmp> [yaw pitch zoom] [shaded|edges|wire]
cargo run -- --render bauteil.step ansicht.bmp -0.72 0.42 1.0 edges
```

**Startup-Diagnose (`--diag`, Windows/macOS):** ohne Fenster prüft das
Programm Version, Adapter und einen echten Geräteprobe-Versuch – Bericht auf
stdout und in `%TEMP%\Rohteilcalc.log`. Exit-Code 0 (auch bei 0 Adaptern).
Vom entpackten Ordner in PowerShell:

```powershell
.\Rohteilcalc.exe --diag
```

## Rohteilregeln

| Achse | Regel |
|-------|-------|
| **X** | Bauteillänge + 5 mm Sägeaufmaß |
| **Y** | Halbzeug-Staffel: bis 25 mm in 5-mm-Schritten, dann 10-mm-Schritte |
| **Z** | Standard-Plattenstärke in 10-mm-Stufen (10, 20, 30, 40, …) |

Die Z-Dicke enthält immer mindestens 4 mm Spannrand unten und 1 mm
Planfräs-Aufmaß oben:

```text
Z_Rohteil = ceil((Z_Bauteil + 5 mm) / 10 mm) * 10 mm
```

## Download

Die aktuellen Builds liegen unter
[**Actions → Build**](https://github.com/hannesfox/Rohteilcalc/actions/workflows/main.yml)
als Artefakt:

- `Rohteilcalc-windows-x64.zip` → enthält `Rohteilcalc.exe`,
  `dxcompiler.dll`, `dxil.dll` (Microsoft DirectXShaderCompiler v1.9.2609)
  und `DXC-LICENSE.txt` – die beiden DLLs müssen **direkt neben dem
  `.exe`** liegen, so dass DX12 deterministisch über DXC (`DynamicDxc`)
  läuft.
- `Rohteilcalc-macos-universal.zip` → enthält `Rohteilcalc.app`

Bei einem Tag (`v*`) werden die Pakete zusätzlich als
[Release](https://github.com/hannesfox/Rohteilcalc/releases) veröffentlicht.

### Troubleshooting: Starts mit Problemen

Ein stiller Start auf Windows ist jetzt diagnosierbar:

1. **Log ansehen:** Das Programm schreibt ein Crash-Log nach
   `%TEMP%\Rohteilcalc.log`; am Start des Vorläufers steht das vorherige
   Log in `Rohteilcalc.prev.log`. Falls das Log fehlt, ging es nach dem
   Neustart noch gar nicht so weit.
2. **Diagnose laufen lassen:** In einer PowerShell im entpackten Ordner:
   ```powershell
   .\Rohteilcalc.exe --diag
   ```
   Der Bericht (Version, Adapter, Limits, echte Geräteprobe) kommt auf
   stdout und ins Log.
3. **Vom Terminal starten:** Mit Doppelklick ist stdout ohne Konsole
   unsichtbar; ein Start aus einer PowerShell/cmd zeigt dann die
   Meldungen (`RohteilCalc konnte nicht starten: …`) – der Dialog nennt
   das Log.
4. **Alternativen (weiterführend):** Falls DX12 (oder der mitgelieferte
   DXC) problematisch ist, Backend umstellen:
   ```powershell
   $env:WGPU_BACKEND = "gl"        # oder "vulkan"
   $env:WGPU_DX12_COMPILER = "fxc"
   .\Rohteilcalc.exe
   ```
5. **Beim Melden eines Problems:** Nimm den `--diag`-Bericht und den Inhalt
   von `%TEMP%\Rohteilcalc.log` als Text mit (keine STEP-Datei-Inhalte).

### Hinweise zu den Downloads

- **Windows:** Die `.exe` ist nicht signiert. Windows SmartScreen meldet sich
  beim ersten Start – über *Weitere Informationen → Trotzdem ausführen* starten.
- **macOS:** Die App ist nur ad-hoc signiert und nicht notarisiert. Beim ersten
  Start *Rechtsklick → Öffnen* wählen oder das Quarantäne-Attribut entfernen:

  ```bash
  xattr -dr com.apple.quarantine /Applications/Rohteilcalc.app
  ```

## Selbst bauen

Voraussetzung: [Rust](https://rustup.rs) (stable).

```bash
git clone https://github.com/hannesfox/Rohteilcalc.git
cd Rohteilcalc
cargo run --release
```

Release-Binary erzeugen:

```bash
cargo build --release
# macOS:   target/release/rohteil_calc
# Windows: target\release\rohteil_calc.exe
```

### Projektstruktur

```text
.
├── .github/workflows/main.yml   # CI: Windows- und macOS-Build
├── assets/
│   ├── logo.png                 # Logo (Quelle für die App-Icons)
│   ├── icon.ico                 # Windows-Icon (in der .exe eingebettet)
│   └── icon.icns                # macOS-App-Icon
├── build.rs                     # bettet das Icon unter Windows ein
├── src/
│   ├── main.rs                  # Einstieg: GUI, CLI, BMP-Export
│   ├── diag.rs                  # Start-Log, --diag-Berichte, Adapter-Proben, GPU-Fehler-Log
│   ├── gpu.rs                   # Backend-Reihenfolge, Rückfall-Prozesse, Shader-Compiler
│   ├── step.rs                  # STEP-Import (B-Rep bleibt erhalten) + Tessellierung
│   ├── mesh.rs                  # Anzeigenetz, Normalen, Feature-Kanten
│   ├── stock.rs                 # Ausrichtung und Rohteilregeln
│   ├── camera.rs                # Orbit-Kamera und Standardansichten
│   ├── renderer.rs              # wgpu-Renderer (Pipelines, MSAA, Kanten, Rohteil)
│   └── app.rs                   # egui-Oberfläche, asynchrones Laden
├── examples/probe.rs            # Debug: roher ruststep-Parsefehler
├── tests/windows_smoke.ps1      # lokaler Windows-11-GUI/CLI-Smoke-Test
└── Cargo.toml
```

## Technischer Hintergrund

- **STEP-Parsing:** `truck-stepio` / `truck-meshalgo`. Die B-Rep-Daten bleiben
  als Dokument erhalten; das Dreiecksnetz ist reine Anzeigegeometrie und wird
  bei Qualitätswechsel neu erzeugt.
- **Rendering:** wgpu über einen `egui_wgpu`-Paint-Callback. Die Szene wird mit
  4× MSAA und Tiefenpuffer in ein Offscreen-Ziel gerendert und anschließend in
  den egui-Pass kopiert (egui selbst besitzt keinen Tiefenpuffer).
- **Schattierung:** Blinn-Phong mit Himmelslicht, Key-Light, Specular und
  Silhouettenabdunklung. Flächennormalen werden nur über Kanten < 40° geglättet,
  dadurch weiche Zylinder-/Kegelflächen bei scharfen Bauteilkanten.
- **Kanten:** Feature-Kanten ab 40° Flächenwinkel, als instanzierte
  Screen-Space-Quads gezeichnet (wgpu unterstützt nur 1-px-Linien).
- **Robustheit:** Latin-1-Fallback für ältere Exporte, Reparatur leerer
  Parameterlisten, Einheiten-Umrechnung nach mm; nicht triangulierbare Flächen
  werden als Warnung gemeldet statt still ignoriert.

## Windows: Start und Grafik-Backends

Die Windows-EXE ist eigenständig: sie benötigt weder Rust/Cargo noch ein
Visual-C++-Redistributable (statische CRT) und keine zusätzlich ausgelieferten
DLLs. Das ZIP enthält nur `Rohteilcalc.exe`, `logo.png` und `README.md`; die
EXE kann aus jedem Verzeichnis gestartet werden.

**Backends:** Wie SolidCraft startet Rohteilcalc unter Windows zuerst und
standardmäßig **nur Direct3D 12** (auch WARP-Softwareadapter zählt). Das ist
bewusst: wgpu/eframe würden sonst DX12, Vulkan und OpenGL gleichzeitig
initialisieren; Intels Vulkan-Treiber (`igvk64.dll`) stürzte dadurch beim
Start ab, bevor irgendeine Fehlermeldung möglich war. Schlägt der Start fehl,
wird in einem **neuen Prozess** der Reihe nach Vulkan, GLES und DX12-WARP
versucht. macOS nutzt Metal, Linux Vulkan/GLES. Alle diese APIs sind
kompiliert (`Cargo.toml`), aber nur die gewählte wird initialisiert.

**Shader-Compiler:** Standard ist FXC — in Windows/wgpu enthalten, ohne
`dxcompiler.dll`. `wgpu` kann DXC auch statisch einbinden (Cargo-Feature
`static-dxc`, dann ohne DLL); ein Nutzer kann mit `WGPU_DX12_COMPILER=dxc`
jederzeit ein dynamisches DXC verlangen. `WGPU_BACKEND=dx12|vulkan|gl` wählt
ein Backend manuell und schaltet den automatischen Rückfall ab.

**Diagnose:** Bei jedem Start wird `%TEMP%\Rohteilcalc.log` geschrieben
(altes Log → `Rohteilcalc.prev.log`). `Rohteilcalc.exe --diag` meldet OS-Version,
EXE, Adapter mit Hersteller/Gerät, gewähltes und kompilierte Backends,
Shader-Compiler, Geräteproben, Pipeline-Fehler und die echten DLL-Importe der
EXE. Auf Windows fragt `--diag` jedes Backend in einem eigenen Prozess ab,
damit ein Treiberabsturz die Diagnose nicht mitnimmt. `--render` rendert
headless ein BMP, `--inspect` analysiert eine STEP-Datei.

**Lokaler Smoke-Test:** `powershell -File tests/windows_smoke.ps1 -Exe
<pfad>\Rohteilcalc.exe` prüft `--diag`, `--render`, den laufenden GUI-Prozess
und das Start-Log. Der GitHub-Runner hat eine interaktive Sitzung, aber nur
einen WARP-Softwareadapter; echte Hardware (NVIDIA/AMD/Intel) kann dort nicht
getestet werden — dafür ist der lokale Smoke-Test auf dem Zielrechner
maßgeblich.

## Lizenz

Dieses Projekt steht unter der [Apache-2.0-Lizenz](LICENSE).

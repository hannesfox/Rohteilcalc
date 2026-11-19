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

- `Rohteilcalc-windows-x64.zip` → enthält `Rohteilcalc.exe`
- `Rohteilcalc-macos-universal.zip` → enthält `Rohteilcalc.app`

Bei einem Tag (`v*`) werden die Pakete zusätzlich als
[Release](https://github.com/hannesfox/Rohteilcalc/releases) veröffentlicht.

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
│   ├── step.rs                  # STEP-Import (B-Rep bleibt erhalten) + Tessellierung
│   ├── mesh.rs                  # Anzeigenetz, Normalen, Feature-Kanten
│   ├── stock.rs                 # Ausrichtung und Rohteilregeln
│   ├── camera.rs                # Orbit-Kamera und Standardansichten
│   ├── renderer.rs              # wgpu-Renderer (Pipelines, MSAA, Kanten, Rohteil)
│   └── app.rs                   # egui-Oberfläche, asynchrones Laden
├── examples/probe.rs            # Debug: roher ruststep-Parsefehler
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

## Lizenz

Dieses Projekt steht unter der [Apache-2.0-Lizenz](LICENSE).

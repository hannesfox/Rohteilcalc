//! Orbit-Kamera mit Pan, Zoom und Standardansichten.

use glam::{Mat4, Vec3};

/// Standard-Kamerapositionen (Z ist die Bauteilhöhe).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandardView {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    Iso,
}

impl StandardView {
    pub fn label(self) -> &'static str {
        match self {
            StandardView::Front => "Vorne",
            StandardView::Back => "Hinten",
            StandardView::Left => "Links",
            StandardView::Right => "Rechts",
            StandardView::Top => "Oben",
            StandardView::Bottom => "Unten",
            StandardView::Iso => "Iso",
        }
    }

    pub const ALL: [StandardView; 7] = [
        StandardView::Front,
        StandardView::Back,
        StandardView::Left,
        StandardView::Right,
        StandardView::Top,
        StandardView::Bottom,
        StandardView::Iso,
    ];

    /// (yaw, pitch) in Radiant.
    pub fn angles(self) -> (f32, f32) {
        let iso = std::f32::consts::FRAC_PI_4;
        match self {
            StandardView::Front => (0.0, 0.0),
            StandardView::Back => (std::f32::consts::PI, 0.0),
            StandardView::Left => (-std::f32::consts::FRAC_PI_2, 0.0),
            StandardView::Right => (std::f32::consts::FRAC_PI_2, 0.0),
            StandardView::Top => (0.0, std::f32::consts::FRAC_PI_2),
            StandardView::Bottom => (0.0, -std::f32::consts::FRAC_PI_2),
            StandardView::Iso => (-iso, 0.6155),
        }
    }
}

#[derive(Clone, Debug)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    /// Kantung des Bildes (Roll um die Blickachse), wie in SolidCraft;
    /// Standardansichten und fit setzen sie auf 0.
    pub roll: f32,
    pub distance: f32,
    pub fov_y: f32,
    /// `false` = orthografische Projektion (SolidCraft: `fov = 0`,
    /// sichtbare Höhe = 0,4 · distance).
    pub perspective: bool,
    pub model_center: Vec3,
    pub model_radius: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: -0.72,
            pitch: 0.42,
            roll: 0.0,
            distance: 10.0,
            fov_y: 35f32.to_radians(),
            perspective: false, // orthografische Ansicht als Standard
            model_center: Vec3::ZERO,
            model_radius: 1.0,
        }
    }
}

impl OrbitCamera {
    pub fn fit(&mut self, center: Vec3, radius: f32) {
        self.model_center = center;
        self.model_radius = radius.max(1.0);
        self.target = center;
        self.distance = self.fit_distance();
    }

    /// Sichtbarer Vertikalmaßstab wie in SolidCraft: bei Perspektive
    /// `distance · tan(fov/2)`, orthografisch fest `0,4 · distance`.
    pub fn half_height(&self) -> f32 {
        if self.perspective {
            self.distance * (self.fov_y * 0.5).tan()
        } else {
            self.distance * 0.4
        }
    }

    /// FOV für die Renderkamera (0 = orthografisch, wie SolidCraft).
    pub fn render_fov(&self) -> f32 {
        if self.perspective {
            self.fov_y
        } else {
            0.0
        }
    }

    /// Nachführen, wenn `perspective` extern (UI) umgeschaltet wurde:
    /// die sichtbare Modellhöhe bleibt gleich (wie SolidCrafts Ortho-Umschalter).
    /// `was` = Modus vor dem Umschalten.
    pub fn projection_adjust_distance(&mut self, was_perspective: bool) {
        let h = if was_perspective {
            self.distance * (self.fov_y * 0.5).tan()
        } else {
            self.distance * 0.4
        };
        self.distance = if self.perspective {
            h / (self.fov_y * 0.5).tan()
        } else {
            h / 0.4
        };
    }

    pub fn fit_distance(&self) -> f32 {
        // Fit wie SolidCraft (`Camera::fit`): Perspektive über den
        // Sichtkegel, Ortho konstant 0,4 · r · 1,15.
        if self.perspective {
            (self.model_radius / (self.fov_y * 0.5).sin()).max(self.model_radius * 0.25) * 1.10
        } else {
            self.model_radius / 0.4 * 1.15
        }
    }

    pub fn fit_view(&mut self) {
        self.target = self.model_center;
        self.distance = self.fit_distance();
    }

    pub fn eye(&self) -> Vec3 {
        let cp = self.pitch.cos();
        let dir = Vec3::new(
            cp * self.yaw.sin(),
            -cp * self.yaw.cos(),
            self.pitch.sin(),
        );
        self.target + dir * self.distance
    }

    fn up(&self) -> Vec3 {
        if self.pitch.cos().abs() < 0.05 {
            Vec3::Y * if self.pitch >= 0.0 { 1.0 } else { -1.0 }
        } else {
            Vec3::Z
        }
    }

    // Legacy-Matrizen (ehemaliger wgpu-0.19-Renderer); die Ansicht rechnet
    // inzwischen über die SolidCraft-Kamera.
    #[allow(dead_code)]
    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.target, self.up())
    }

    #[allow(dead_code)]
    pub fn projection(&self, aspect: f32) -> Mat4 {
        let near = (self.distance * 0.01).max(0.01);
        let far = self.distance + self.model_radius * 20.0;
        Mat4::perspective_rh(self.fov_y, aspect.max(0.01), near, far)
    }

    #[allow(dead_code)]
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        self.projection(aspect) * self.view()
    }

    #[allow(dead_code)]
    pub fn eye_position(&self) -> Vec3 {
        self.eye()
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        let max_pitch = std::f32::consts::FRAC_PI_2 - 0.001;
        self.yaw -= dx * 0.008;
        self.pitch = (self.pitch + dy * 0.008).clamp(-max_pitch, max_pitch);
    }

    /// Kantung des Bilds um `radians` (Roll um die Blickachse), wie in
    /// SolidCraft `Camera::roll_by` — mod 2π gewickelt.
    pub fn roll_by(&mut self, radians: f32) {
        self.roll = (self.roll + radians) % std::f32::consts::TAU;
    }

    /// Ein Drehen des Geräts bei gekantetem Blick, wie in SolidCraft
    /// `navigation::orbit_on_screen`: die Bildschirm-Deltas gehen zurück
    /// durch die Kante, so dass der Drehtisch trotzdem in Bildschirmrichtung
    /// lenkt (liegt der Horizont schief oder auf dem Kopf, schiebt die
    /// Bewegung trotzdem da hin, wo der Finger am Bildschirm ist). Ohne Roll
    /// genau `orbit`.
    pub fn orbit_on_screen(&mut self, dx: f32, dy: f32) {
        let (sin, cos) = self.roll.sin_cos();
        self.orbit(dx * cos + dy * sin, dy * cos - dx * sin);
    }

    pub fn pan(&mut self, dx: f32, dy: f32, viewport_height: f32) {
        let forward = (self.target - self.eye()).normalize_or_zero();
        // Die Bildachsen in der gekanteten Ansicht (wie in SolidCraft
        // `Camera::basis`): Grundbasis aus Blick und Welt-Zenit, dann Roll.
        let right = forward.cross(self.up()).normalize_or_zero();
        let up = right.cross(forward).normalize_or_zero();
        let (s, c) = self.roll.sin_cos();
        let screen_right = right * c + up * s;
        let screen_up = up * c - right * s;
        let world_per_pixel = 2.0 * self.half_height() / viewport_height.max(1.0);
        self.target += (-screen_right * dx + screen_up * dy) * world_per_pixel;
    }

    /// Zoom um einen Faktor (kleiner als 1 = heran), begrenzt auf den
    /// Modellkranz — die 3D-Maus kommt mit `exp(-1.5·t·Bewegung)` daher.
    pub fn zoom_factor(&mut self, factor: f32) {
        self.distance = (self.distance * factor)
            .clamp(self.model_radius * 0.15, self.model_radius * 25.0);
    }

    pub fn zoom(&mut self, scroll: f32) {
        self.zoom_factor((-scroll * 0.0015).exp());
    }

    pub fn set_standard_view(&mut self, view: StandardView) {
        let (yaw, pitch) = view.angles();
        self.yaw = yaw;
        self.pitch = pitch;
        self.roll = 0.0;
        self.target = self.model_center;
    }
}

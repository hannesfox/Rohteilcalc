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
            StandardView::Top => (0.0, 1.5533),
            StandardView::Bottom => (0.0, -1.5533),
            StandardView::Iso => (-iso, 0.6155),
        }
    }
}

#[derive(Clone, Debug)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov_y: f32,
    pub model_center: Vec3,
    pub model_radius: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            yaw: -0.72,
            pitch: 0.42,
            distance: 10.0,
            fov_y: 35f32.to_radians(),
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

    pub fn fit_distance(&self) -> f32 {
        (self.model_radius / (self.fov_y * 0.5).sin()).max(self.model_radius * 0.25) * 1.10
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

    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.target, self.up())
    }

    pub fn projection(&self, aspect: f32) -> Mat4 {
        let near = (self.distance * 0.01).max(0.01);
        let far = self.distance + self.model_radius * 20.0;
        Mat4::perspective_rh(self.fov_y, aspect.max(0.01), near, far)
    }

    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        self.projection(aspect) * self.view()
    }

    pub fn eye_position(&self) -> Vec3 {
        self.eye()
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.008;
        self.pitch = (self.pitch + dy * 0.008).clamp(-1.5533, 1.5533);
    }

    pub fn pan(&mut self, dx: f32, dy: f32, viewport_height: f32) {
        let forward = (self.target - self.eye()).normalize_or_zero();
        let right = forward.cross(self.up()).normalize_or_zero();
        let up = right.cross(forward).normalize_or_zero();
        let world_per_pixel =
            2.0 * self.distance * (self.fov_y * 0.5).tan() / viewport_height.max(1.0);
        self.target += (-right * dx + up * dy) * world_per_pixel;
    }

    pub fn zoom(&mut self, scroll: f32) {
        let factor = (-scroll * 0.0015).exp();
        self.distance = (self.distance * factor)
            .clamp(self.model_radius * 0.15, self.model_radius * 25.0);
    }

    pub fn set_standard_view(&mut self, view: StandardView) {
        let (yaw, pitch) = view.angles();
        self.yaw = yaw;
        self.pitch = pitch;
        self.target = self.model_center;
    }
}

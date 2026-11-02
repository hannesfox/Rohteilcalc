//! GPU-Renderer für den STEP-Viewport (wgpu).
//!
//! Aufbau:
//! - [`SceneData`] ist die plattformunabhängige, triangulierte Szene
//!   (Dreiecke mit Normalen, Feature-Kanten, Wireframe-Kanten, Rohteil-Box).
//! - [`SceneRenderer`] hält Pipelines, Uniforms und GPU-Puffer. Die
//!   Geometrie wird nur bei Revisionswechsel hochgeladen (Mesh-Cache).
//! - [`ViewportCallback`] bindet den Renderer in egui ein: `prepare` zeichnet
//!   in ein MSAA-Offscreen-Ziel mit Tiefenpuffer, `paint` legt das fertige
//!   Bild in den egui-Renderpass.
//!
//! Das Offscreen-Ziel ist nötig, weil egui selbst keinen Tiefenpuffer
//! bereitstellt. Es erlaubt zugleich echtes MSAA und einen sauberen
//! Tiefentest für Flächen und Kanten.

use crate::step::Body;
use crate::stock::StockResult;
use bytemuck::{Pod, Zeroable};
use eframe::egui;
use egui_wgpu::{CallbackResources, CallbackTrait, ScreenDescriptor};
use glam::{Mat4, Vec3};
use std::borrow::Cow;
use std::sync::Arc;
use wgpu::util::DeviceExt;

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SAMPLE_COUNT: u32 = 4;

// ---------------------------------------------------------
// Szenendaten (CPU-Seite)
// ---------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct StockVertex {
    pub pos: [f32; 3],
}

/// Ein Liniensegment. Die Verdickung passiert im Vertex-Shader
/// (Bildschirmraum), weil wgpu nur 1-px-Linien unterstützt.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct LineSegment {
    pub p0: [f32; 3],
    pub p1: [f32; 3],
    pub color: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Shaded,
    ShadedEdges,
    Wireframe,
}

impl ViewMode {
    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Shaded => "Schattiert",
            ViewMode::ShadedEdges => "Schattiert + Kanten",
            ViewMode::Wireframe => "Drahtgitter",
        }
    }

    pub const ALL: [ViewMode; 3] = [
        ViewMode::Shaded,
        ViewMode::ShadedEdges,
        ViewMode::Wireframe,
    ];
}

/// Fertig aufbereitete Anzeigeszene. `revision` steuert den GPU-Upload.
#[derive(Clone, Debug)]
pub struct SceneData {
    pub revision: u64,
    pub vertices: Vec<GpuVertex>,
    pub indices: Vec<u32>,
    pub feature_edges: Vec<LineSegment>,
    /// Transluzente Rohteil-Box (12 Dreiecke).
    pub stock_faces: Vec<StockVertex>,
    /// Nur obere Boxkontur + Spannlinie.
    pub stock_edges: Vec<LineSegment>,
    pub center: Vec3,
    pub radius: f32,
    pub triangle_count: usize,
}

fn srgb_to_linear(c: [f32; 3]) -> [f32; 3] {
    let f = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    [f(c[0]), f(c[1]), f(c[2])]
}

impl SceneData {
    /// Baut die Szene aus den sichtbaren Körpern und der Rohteil-Box.
    pub fn build(bodies: &[Body], stock: &StockResult, revision: u64) -> Self {
        let feature_color = srgb_to_linear([0.16, 0.18, 0.22]);

        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut feature_edges = Vec::new();

        for body in bodies.iter().filter(|b| b.visible) {
            let mesh = &body.mesh;

            for (ti, tri) in mesh.triangles.iter().enumerate() {
                let (a, b, c) = (
                    mesh.vertices[tri[0]],
                    mesh.vertices[tri[1]],
                    mesh.vertices[tri[2]],
                );
                let face_n = (b - a).cross(c - a).normalize_or_zero();
                let corner = mesh
                    .normals
                    .get(ti)
                    .copied()
                    .unwrap_or([face_n, face_n, face_n]);

                let base = vertices.len() as u32;
                for (k, &vi) in tri.iter().enumerate() {
                    let p = mesh.vertices[vi];
                    let mut n = corner[k];
                    if !n.is_finite() || n.length_squared() < 1e-12 {
                        n = face_n;
                    }
                    vertices.push(GpuVertex {
                        pos: [p.x, p.y, p.z],
                        normal: [n.x, n.y, n.z],
                    });
                }
                indices.extend_from_slice(&[base, base + 1, base + 2]);
            }

            for e in &mesh.feature_edges {
                let (a, b) = (mesh.vertices[e[0]], mesh.vertices[e[1]]);
                feature_edges.push(LineSegment {
                    p0: a.to_array(),
                    p1: b.to_array(),
                    color: feature_color,
                });
            }

        }

        // Rohteil-Box
        let part = stock.part_size;
        let size = stock.stock_size;
        let bmin = Vec3::new(
            (part.x - size.x) * 0.5,
            (part.y - size.y) * 0.5,
            -stock.clamp_depth_z,
        );
        let bmax = bmin + size;
        let center = (bmin + bmax) * 0.5;
        let radius = ((bmax - bmin).length() * 0.5).max(1.0);

        let box_color = srgb_to_linear([1.0, 0.78, 0.27]);
        let clamp_color = srgb_to_linear([0.90, 0.35, 0.35]);
        let corners = [
            Vec3::new(bmin.x, bmin.y, bmin.z),
            Vec3::new(bmax.x, bmin.y, bmin.z),
            Vec3::new(bmax.x, bmax.y, bmin.z),
            Vec3::new(bmin.x, bmax.y, bmin.z),
            Vec3::new(bmin.x, bmin.y, bmax.z),
            Vec3::new(bmax.x, bmin.y, bmax.z),
            Vec3::new(bmax.x, bmax.y, bmax.z),
            Vec3::new(bmin.x, bmax.y, bmax.z),
        ];

        // Rohteil als transluzenter Körper (statt Drahtgitter, das sonst
        // sichtbar durch das Werkstück scheint).
        let quads = [
            [0, 2, 1],
            [0, 3, 2], // unten
            [4, 5, 6],
            [4, 6, 7], // oben
            [0, 1, 5],
            [0, 5, 4], // vorne
            [2, 3, 7],
            [2, 7, 6], // hinten
            [0, 4, 7],
            [0, 7, 3], // links
            [1, 2, 6],
            [1, 6, 5], // rechts
        ];
        let mut stock_faces = Vec::with_capacity(quads.len() * 3);
        for tri in quads {
            for idx in tri {
                stock_faces.push(StockVertex {
                    pos: corners[idx].to_array(),
                });
            }
        }

        // Nur die obere Kontur und die Spannlinie sind als Linien sinnvoll.
        let mut stock_edges = Vec::with_capacity(8);
        for i in 0..4 {
            stock_edges.push(LineSegment {
                p0: corners[4 + i].to_array(),
                p1: corners[4 + (i + 1) % 4].to_array(),
                color: box_color,
            });
        }
        let clamp = [
            Vec3::new(bmin.x, bmin.y, 0.0),
            Vec3::new(bmax.x, bmin.y, 0.0),
            Vec3::new(bmax.x, bmax.y, 0.0),
            Vec3::new(bmin.x, bmax.y, 0.0),
        ];
        for i in 0..4 {
            stock_edges.push(LineSegment {
                p0: clamp[i].to_array(),
                p1: clamp[(i + 1) % 4].to_array(),
                color: clamp_color,
            });
        }

        let triangle_count = indices.len() / 3;
        Self {
            revision,
            vertices,
            indices,
            feature_edges,
            stock_faces,
            stock_edges,
            center,
            radius,
            triangle_count,
        }
    }
}

// ---------------------------------------------------------
// Shader & Uniforms
// ---------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    light_dir: [f32; 4],
    screen_size: [f32; 4],
    bg_top: [f32; 4],
    bg_bottom: [f32; 4],
    line_params: [f32; 4],
    stock_color: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
pub struct CameraUniforms {
    pub view_proj: Mat4,
    pub eye: Vec3,
}

const SHADER: &str = r#"
struct Globals {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    light_dir: vec4<f32>,
    screen_size: vec4<f32>,
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
    line_params: vec4<f32>,
    stock_color: vec4<f32>,
};
@group(0) @binding(0) var<uniform> globals: Globals;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = globals.view_proj * vec4<f32>(in.pos, 1.0);
    out.world = in.pos;
    out.normal = in.normal;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let view_dir = normalize(globals.eye.xyz - in.world);
    var n = normalize(in.normal);
    // truck liefert teils uneinheitliche Windungen: Normale immer zum
    // Betrachter drehen (zweiseitige Beleuchtung).
    if (dot(n, view_dir) < 0.0) {
        n = -n;
    }

    let key = normalize(globals.light_dir.xyz);
    let diff = max(dot(n, key), 0.0);
    let fill = max(dot(n, view_dir), 0.0);
    let half_vec = normalize(key + view_dir);
    let spec = pow(max(dot(n, half_vec), 0.0), 72.0) * 0.22;

    // Himmelslicht statt konstantem Ambient: oben heller, unten dunkler.
    // Dadurch heben sich Deck-, Seiten- und Bodenflächen klar voneinander ab.
    let hemi = mix(vec3<f32>(0.080, 0.090, 0.110),
                   vec3<f32>(0.265, 0.300, 0.365),
                   0.5 + 0.5 * clamp(n.z, -1.0, 1.0));

    // Stahlblau (linear), Key-Licht von links-vorne-oben.
    let base = vec3<f32>(0.455, 0.515, 0.625);
    var col = base * (hemi + 0.62 * diff + 0.12 * fill) + vec3<f32>(spec);

    // Silhouetten abdunkeln -> plastischer CAD-Look.
    let ndv = abs(dot(n, view_dir));
    if (ndv < 0.16) {
        let a = 1.0 - ndv / 0.16;
        col = mix(col, vec3<f32>(0.020, 0.024, 0.030), a);
    }
    return vec4<f32>(clamp(col, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}

@vertex
fn vs_bg(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(p[i], 0.0, 1.0);
}

@fragment
fn fs_bg(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let t = clamp(pos.y / globals.screen_size.y, 0.0, 1.0);
    let c = mix(globals.bg_top.xyz, globals.bg_bottom.xyz, t);
    return vec4<f32>(c, 1.0);
}

struct LineIn {
    @location(0) p0: vec3<f32>,
    @location(1) p1: vec3<f32>,
    @location(2) color: vec3<f32>,
};
struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
};

// Jedes Segment wird als instanziertes Rechteck (2 Dreiecke) gerendert.
// Die Verbreiterung erfolgt im Bildschirmraum, damit die Kanten unabhängig
// von der Perspektive gleich dick bleiben.
@vertex
fn vs_line(in: LineIn, @builtin(vertex_index) vi: u32) -> LineOut {
    let ca = globals.view_proj * vec4<f32>(in.p0, 1.0);
    let cb = globals.view_proj * vec4<f32>(in.p1, 1.0);

    let res = globals.screen_size.xy;
    let sa = ca.xy / ca.w * res * 0.5;
    let sb = cb.xy / cb.w * res * 0.5;
    let d = normalize(sb - sa);
    let nrm = vec2<f32>(-d.y, d.x);
    let half_px = globals.line_params.x;

    var at_b = false;
    var side = -1.0;
    switch vi {
        case 0u: { }
        case 1u: { side = 1.0; }
        case 2u: { at_b = true; }
        case 3u: { side = 1.0; }
        case 4u: { at_b = true; side = 1.0; }
        default: { at_b = true; }
    }

    var clip = ca;
    if (at_b) { clip = cb; }
    let offset = nrm * side * (half_px * 2.0 / res);
    clip = vec4<f32>(clip.xy + offset * clip.w, clip.z, clip.w);

    var out: LineOut;
    out.clip = clip;
    out.color = in.color;
    return out;
}

@fragment
fn fs_line(in: LineOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}

struct SolidIn {
    @location(0) pos: vec3<f32>,
};
struct SolidOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
};

@vertex
fn vs_solid(in: SolidIn) -> SolidOut {
    var out: SolidOut;
    out.clip = globals.view_proj * vec4<f32>(in.pos, 1.0);
    out.world = in.pos;
    return out;
}

@fragment
fn fs_stock(in: SolidOut) -> @location(0) vec4<f32> {
    // Leichter Kanten-Touch an den Flächen -> Volumen bleibt lesbar.
    let n = normalize(cross(dpdx(in.world), dpdy(in.world)));
    let facing = clamp(abs(dot(n, normalize(globals.eye.xyz - in.world))), 0.0, 1.0);
    let alpha = mix(0.035, 0.105, facing);
    return vec4<f32>(globals.stock_color.rgb, alpha);
}

// Abschluss: Offscreen-Textur in den egui-Renderpass kopieren.
struct QuadOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_quad(@builtin(vertex_index) i: u32) -> QuadOut {
    var p = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var out: QuadOut;
    out.clip = vec4<f32>(p[i], 0.0, 1.0);
    // uv: (-1,-1) -> (0,1), (3,-1) -> (2,1), (-1,3) -> (0,-1)
    out.uv = p[i] * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    return out;
}

@group(0) @binding(0) var screen_tex: texture_2d<f32>;
@group(0) @binding(1) var screen_sampler: sampler;

@fragment
fn fs_quad(in: QuadOut) -> @location(0) vec4<f32> {
    return textureSample(screen_tex, screen_sampler, in.uv);
}
"#;

// ---------------------------------------------------------
// GPU-Ressourcen
// ---------------------------------------------------------

struct Targets {
    size: [u32; 2],
    msaa: wgpu::TextureView,
    resolved_texture: wgpu::Texture,
    resolved: wgpu::TextureView,
    depth: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

struct GpuScene {
    revision: u64,
    tri_vb: wgpu::Buffer,
    tri_ib: wgpu::Buffer,
    tri_index_count: u32,
    feature_vb: wgpu::Buffer,
    feature_count: u32,
    stock_face_vb: wgpu::Buffer,
    stock_face_count: u32,
    stock_vb: wgpu::Buffer,
    stock_count: u32,
}

/// GPU-Zustand des Viewports. Lebt in den `callback_resources` von egui-wgpu,
/// damit Puffer und Pipelines über Frames hinweg erhalten bleiben.
pub struct SceneRenderer {
    globals: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    triangle_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    wire_pipeline: wgpu::RenderPipeline,
    stock_pipeline: wgpu::RenderPipeline,
    background_pipeline: wgpu::RenderPipeline,
    quad_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    quad_layout: wgpu::BindGroupLayout,
    targets: Option<Targets>,
    scene: Option<GpuScene>,
}

impl SceneRenderer {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viewport_shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER)),
        });

        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport_globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport_globals_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport_globals_bind_group"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });

        let scene_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viewport_scene_layout"),
            bind_group_layouts: &[&globals_layout],
            push_constant_ranges: &[],
        });

        let triangle_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];
        let line_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3];

        let triangle_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport_triangle_pipeline"),
            layout: Some(&scene_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &triangle_attrs,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: SAMPLE_COUNT,
                ..Default::default()
            },
            multiview: None,
        });

        let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport_line_pipeline"),
            layout: Some(&scene_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_line",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<LineSegment>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &line_attrs,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_line",
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                // Leichte Neigung gegen Z-Fighting; Kanten liegen exakt auf
                // der Fläche.
                bias: wgpu::DepthBiasState {
                    constant: -2,
                    slope_scale: -1.0,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState {
                count: SAMPLE_COUNT,
                ..Default::default()
            },
            multiview: None,
        });

        // Drahtgitter: dieselben CAD-Kanten, aber ohne Tiefentest, damit
        // auch die verdeckten Kanten sichtbar sind (klassischer Wireframe).
        let wire_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport_wire_pipeline"),
            layout: Some(&scene_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_line",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<LineSegment>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &line_attrs,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_line",
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: SAMPLE_COUNT,
                ..Default::default()
            },
            multiview: None,
        });

        let stock_attrs = wgpu::vertex_attr_array![0 => Float32x3];
        let stock_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport_stock_pipeline"),
            layout: Some(&scene_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_solid",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<StockVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &stock_attrs,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_stock",
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: SAMPLE_COUNT,
                ..Default::default()
            },
            multiview: None,
        });

        let background_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport_background_pipeline"),
            layout: Some(&scene_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_bg",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_bg",
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: SAMPLE_COUNT,
                ..Default::default()
            },
            multiview: None,
        });

        let quad_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport_quad_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let quad_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viewport_quad_pipeline_layout"),
            bind_group_layouts: &[&quad_layout],
            push_constant_ranges: &[],
        });

        let quad_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewport_quad_pipeline"),
            layout: Some(&quad_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_quad",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_quad",
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("viewport_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self {
            globals,
            bind_group,
            triangle_pipeline,
            line_pipeline,
            wire_pipeline,
            stock_pipeline,
            background_pipeline,
            quad_pipeline,
            sampler,
            quad_layout,
            targets: None,
            scene: None,
        }
    }

    fn ensure_targets(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        let size = [size[0].max(1), size[1].max(1)];
        if self.targets.as_ref().map(|t| t.size) == Some(size) {
            return;
        }

        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };

        let msaa = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("viewport_msaa"),
                size: extent,
                mip_level_count: 1,
                sample_count: SAMPLE_COUNT,
                dimension: wgpu::TextureDimension::D2,
                format: COLOR_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());

        let resolved_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("viewport_resolved"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let resolved = resolved_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("viewport_depth"),
                size: extent,
                mip_level_count: 1,
                sample_count: SAMPLE_COUNT,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport_resolved_bind_group"),
            layout: &self.quad_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&resolved),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        self.targets = Some(Targets {
            size,
            msaa,
            resolved_texture,
            resolved,
            depth,
            bind_group,
        });
    }

    fn upload_scene(&mut self, device: &wgpu::Device, scene: &Arc<SceneData>) {
        if self.scene.as_ref().map(|s| s.revision) == Some(scene.revision) {
            return;
        }

        let vb = |label: &str, data: &[u8]| {
            if data.is_empty() {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: 16,
                    usage: wgpu::BufferUsages::VERTEX,
                    mapped_at_creation: false,
                })
            } else {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: data,
                    usage: wgpu::BufferUsages::VERTEX,
                })
            }
        };

        let tri_vb = vb(
            "scene_triangles",
            bytemuck::cast_slice(&scene.vertices),
        );
        let tri_ib = if scene.indices.is_empty() {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("scene_indices"),
                size: 16,
                usage: wgpu::BufferUsages::INDEX,
                mapped_at_creation: false,
            })
        } else {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("scene_indices"),
                contents: bytemuck::cast_slice(&scene.indices),
                usage: wgpu::BufferUsages::INDEX,
            })
        };

        self.scene = Some(GpuScene {
            revision: scene.revision,
            tri_vb,
            tri_ib,
            tri_index_count: scene.indices.len() as u32,
            feature_vb: vb("scene_feature_edges", bytemuck::cast_slice(&scene.feature_edges)),
            feature_count: scene.feature_edges.len() as u32,
            stock_face_vb: vb("scene_stock_faces", bytemuck::cast_slice(&scene.stock_faces)),
            stock_face_count: scene.stock_faces.len() as u32,
            stock_vb: vb("scene_stock_edges", bytemuck::cast_slice(&scene.stock_edges)),
            stock_count: scene.stock_edges.len() as u32,
        });
    }

    /// Zeichnet die komplette Szene in das Offscreen-Ziel.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        scene: &Arc<SceneData>,
        camera: CameraUniforms,
        size: [u32; 2],
        mode: ViewMode,
        show_stock: bool,
        line_width: f32,
    ) {
        self.ensure_targets(device, size);
        self.upload_scene(device, scene);

        let globals = Globals {
            view_proj: camera.view_proj.to_cols_array_2d(),
            eye: [camera.eye.x, camera.eye.y, camera.eye.z, 1.0],
            light_dir: [-0.30, -0.65, 0.70, 0.0],
            screen_size: [size[0] as f32, size[1] as f32, 0.0, 0.0],
            bg_top: [0.855, 0.879, 0.913, 1.0],
            bg_bottom: [0.600, 0.643, 0.713, 1.0],
            line_params: [line_width.max(0.5), 0.0, 0.0, 0.0],
            stock_color: [0.95, 0.72, 0.25, 1.0],
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));

        let targets = self.targets.as_ref().unwrap();
        let Some(gpu_scene) = self.scene.as_ref() else {
            return;
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("viewport_offscreen_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.msaa,
                resolve_target: Some(&targets.resolved),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.9,
                        g: 0.9,
                        b: 0.9,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &targets.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            occlusion_query_set: None,
            timestamp_writes: None,
        });

        // Hintergrund
        pass.set_pipeline(&self.background_pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);

        if mode != ViewMode::Wireframe && gpu_scene.tri_index_count > 0 {
            pass.set_pipeline(&self.triangle_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, gpu_scene.tri_vb.slice(..));
            pass.set_index_buffer(gpu_scene.tri_ib.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..gpu_scene.tri_index_count, 0, 0..1);
        }

        // Transluzente Rohteil-Box (nach dem Bauteil, ohne Tiefenschreiben)
        if show_stock && mode != ViewMode::Wireframe && gpu_scene.stock_face_count > 0 {
            pass.set_pipeline(&self.stock_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, gpu_scene.stock_face_vb.slice(..));
            pass.draw(0..gpu_scene.stock_face_count, 0..1);
        }

        let pipeline = if mode == ViewMode::Wireframe {
            &self.wire_pipeline
        } else {
            &self.line_pipeline
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);

        // Modellkanten: im Drahtgitter alle CAD-Kanten ohne Tiefentest,
        // sonst die sichtbaren Kanten mit Tiefentest. Die Triangle-Diagonalen
        // des Netzes werden bewusst nicht gezeichnet – sie wirken wie
        // Kratzer über den Flächen.
        if mode != ViewMode::Shaded && gpu_scene.feature_count > 0 {
            pass.set_vertex_buffer(0, gpu_scene.feature_vb.slice(..));
            pass.draw(0..6, 0..gpu_scene.feature_count);
        }

        // Obere Rohteilkontur + Spannlinie. Mit Tiefentest, damit die Linien
        // hinter dem Bauteil verschwinden statt quer durch das Werkstück zu
        // laufen.
        if show_stock && gpu_scene.stock_count > 0 {
            pass.set_vertex_buffer(0, gpu_scene.stock_vb.slice(..));
            pass.draw(0..6, 0..gpu_scene.stock_count);
        }
    }

    /// Kopiert das fertige Offscreen-Bild in den aktuellen egui-Renderpass.
    pub fn paint<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        let Some(targets) = self.targets.as_ref() else {
            return;
        };
        render_pass.set_pipeline(&self.quad_pipeline);
        render_pass.set_bind_group(0, &targets.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }

    /// Nur intern für den Headless-Renderer.
    fn resolved_texture(&self) -> Option<&wgpu::Texture> {
        self.targets.as_ref().map(|t| &t.resolved_texture)
    }
}

// ---------------------------------------------------------
// egui-Integration
// ---------------------------------------------------------

/// Ein egui-Paint-Callback rendert die Szene mit der aktuellen Kamera.
pub struct ViewportCallback {
    pub scene: Arc<SceneData>,
    pub camera: CameraUniforms,
    pub size: [u32; 2],
    pub mode: ViewMode,
    pub show_stock: bool,
    pub line_width: f32,
}

impl CallbackTrait for ViewportCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &ScreenDescriptor,
        egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(renderer) = callback_resources.get_mut::<SceneRenderer>() {
            renderer.draw(
                device,
                queue,
                egui_encoder,
                &self.scene,
                self.camera,
                self.size,
                self.mode,
                self.show_stock,
                self.line_width,
            );
        }
        Vec::new()
    }

    fn paint<'a>(
        &'a self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'a>,
        callback_resources: &'a CallbackResources,
    ) {
        if let Some(renderer) = callback_resources.get::<SceneRenderer>() {
            renderer.paint(render_pass);
        }
    }
}

impl ViewportCallback {
    pub fn new(
        scene: Arc<SceneData>,
        camera: CameraUniforms,
        size_points: egui::Vec2,
        pixels_per_point: f32,
        mode: ViewMode,
        show_stock: bool,
    ) -> Self {
        let size = [
            (size_points.x * pixels_per_point).round().max(1.0) as u32,
            (size_points.y * pixels_per_point).round().max(1.0) as u32,
        ];
        Self {
            scene,
            camera,
            size,
            mode,
            show_stock,
            line_width: (1.15 * pixels_per_point).clamp(1.0, 4.0),
        }
    }
}

// ---------------------------------------------------------
// Headless-Rendering (CLI / Tests)
// ---------------------------------------------------------

/// Rendert eine Szene ohne Fenster und liefert RGBA-Pixel zurück.
#[allow(clippy::too_many_arguments)]
pub fn render_offscreen(
    scene: &Arc<SceneData>,
    camera: CameraUniforms,
    width: u32,
    height: u32,
    mode: ViewMode,
    show_stock: bool,
) -> Result<Vec<u8>, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .ok_or_else(|| "Kein GPU-Adapter verfügbar".to_string())?;

    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("rohteil_headless"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
        },
        None,
    ))
    .map_err(|e| format!("GPU-Gerät konnte nicht erstellt werden: {e}"))?;

    let mut renderer = SceneRenderer::new(&device, COLOR_FORMAT);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("headless_encoder"),
    });
    renderer.draw(
        &device,
        &queue,
        &mut encoder,
        scene,
        camera,
        [width, height],
        mode,
        show_stock,
        1.35,
    );

    let unpadded = (width * 4) as usize;
    let padded = unpadded.div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("headless_readback"),
        size: (padded * height as usize) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let resolved_texture = renderer
        .resolved_texture()
        .ok_or_else(|| "Renderziel fehlt".to_string())?;
    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture: resolved_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::ImageCopyBuffer {
            buffer: &readback,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(padded as u32),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));
    device.poll(wgpu::Maintain::Wait);

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::Maintain::Wait);
    rx.recv()
        .map_err(|e| format!("GPU-Lesetransfer fehlgeschlagen: {e}"))?
        .map_err(|e| format!("GPU-Lesetransfer fehlgeschlagen: {e}"))?;

    let data = slice.get_mapped_range();
    let mut out = Vec::with_capacity(unpadded * height as usize);
    for row in 0..height as usize {
        let start = row * padded;
        out.extend_from_slice(&data[start..start + unpadded]);
    }
    drop(data);
    readback.unmap();
    Ok(out)
}

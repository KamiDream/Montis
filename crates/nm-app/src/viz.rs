//! VisualSpectrum 的 wgpu 渲染管线，作为 iced 的 `shader` widget 嵌入正在播放页。
//!
//! 与参考项目保持同一套实现：
//!
//! 1. **场景**：频谱条 + 顶端连线 + 粒子 + 中心同心环/X 形油光画到离屏纹理；
//! 2. **余晖**：上一帧乘以衰减系数铺底，形成帧级拖尾；
//! 3. **bloom**：1/4 分辨率两趟可分离高斯；
//! 4. **CRT**：桶形畸变 + RGB 色散 + bloom 合成，最后写进 iced 的渲染目标。
//!
//! 唯一与参考不同的地方是“窗口所有权”：这里不创建 surface，而是使用 iced
//! 提供的 `wgpu::TextureView`，并且用 `clip_bounds` 限定只画在 widget 区域里。

use std::f32::consts::{PI, TAU};

use iced::widget::shader::{wgpu, Primitive, Program, Shader, Storage, Viewport};
use iced::Rectangle;

use crate::app::Message;
use crate::ring::RingScene;
use crate::visualizer::{VisualizerConfig, AFTERGLOW_RANGE, HEIGHT_RANGE, SIZE_RANGE};

const MAX_BARS: usize = 512;
const MAX_PARTICLES: usize = 700;
const CENTER_STOPS: usize = 8;
const CENTER_RINGS: usize = 10;
/// 中心圆随低频能量的最大缩放比例（3%），封面用同一个系数律动。
const CENTER_BASS_PULSE: f32 = 0.03;
const SCENE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// 桶形 / 分色拉满时的强度（与参考项目一致）。
const BARREL_MAX: f32 = 0.35;
const ABERRATION_MAX: f32 = 0.0035;
const BLOOM_STRENGTH: f32 = 0.9;

const BAR_ATTRIBUTES: [wgpu::VertexAttribute; 4] = [
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x2,
        offset: 0,
        shader_location: 0,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x2,
        offset: 8,
        shader_location: 1,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32,
        offset: 16,
        shader_location: 2,
    },
    // `BarInstance` 在 width 后有一个 4 字节的 `_pad`，颜色从 24 开始。
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x4,
        offset: 24,
        shader_location: 3,
    },
];
const PARTICLE_ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
    0 => Float32x2,
    1 => Float32,
    2 => Float32,
    3 => Float32x4
];

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct BarInstance {
    p0: [f32; 2],
    p1: [f32; 2],
    width: f32,
    _pad: f32,
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct ViewportUniform {
    size: [f32; 2],
    /// 余辉每帧的衰减系数（`exp(-dt/tau)`），0 表示关闭。
    decay: f32,
    _pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct PostUniforms {
    /// 背景色（预乘 alpha）；页面上要透出封面，所以固定透明。
    bg: [f32; 4],
    /// (桶形强度, RGB 色散强度, bloom 强度, 预留)
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct ParticleInstance {
    center: [f32; 2],
    radius: f32,
    alpha: f32,
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct CenterUniforms {
    center: [f32; 2],
    radius: f32,
    label_radius: f32,
    spin: f32,
    line_width: f32,
    ring_inner: f32,
    ring_outer: f32,
    bass: f32,
    glow: f32,
    stop_count: u32,
    /// 补齐到 48 字节：WGSL 的 `array<vec4f>` 要求 16 字节对齐。
    _pad: [u32; 1],
    stops: [[f32; 4]; CENTER_STOPS],
    ring_colors: [[f32; 4]; CENTER_RINGS],
}

/// 每帧上传给 GPU 的场景快照。
#[derive(Debug)]
pub struct VizPrimitive {
    viewport: ViewportUniform,
    center: CenterUniforms,
    post: PostUniforms,
    bars: Vec<BarInstance>,
    links: Vec<BarInstance>,
    particles: Vec<ParticleInstance>,
    center_on: bool,
    bloom_on: bool,
}

/// iced shader widget 的 Program。
pub struct VizProgram<'a> {
    pub scene: &'a RingScene,
    pub config: &'a VisualizerConfig,
    /// 环形几何的参考边长。画布可以比它大（铺满窗口），环仍按这个尺寸绘制。
    pub stage: f32,
    /// 圆心相对画布中心的偏移（像素）。
    pub offset: [f32; 2],
}

/// 环的几何（像素）：外径由 `size` 决定，条从内往外长；`height` 可以超过 1.0，
/// 此时底座收到圆心、条顺着往外溢出，不再被舞台边界静默截断。
///
/// 返回 `(radius_outer, radius_base, max_len)`。
fn geometry(short: f32, config: &VisualizerConfig) -> (f32, f32, f32) {
    let radius_outer = short * 0.5 * config.size.clamp(SIZE_RANGE.0, SIZE_RANGE.1);
    let height = config.height.clamp(HEIGHT_RANGE.0, HEIGHT_RANGE.1);
    let max_len = radius_outer * height;
    let radius_base = (radius_outer - max_len).max(0.0);
    (radius_outer, radius_base, max_len)
}

/// 环的外径（像素）。粒子裁剪需要知道半径与画布的关系。
pub(crate) fn outer_radius(short: f32, config: &VisualizerConfig) -> f32 {
    geometry(short, config).0
}

/// 中心圆 / 封面随低频能量的缩放系数：1 = 静止，最大 1.03。
///
/// 播放页的圆形封面用同一个系数缩放，圆边才能始终贴在第 6 个同心环上。
pub(crate) fn center_scale(bass: f32) -> f32 {
    1.0 + bass.clamp(0.0, 1.0) * CENTER_BASS_PULSE
}

/// 中心同心环的半径（像素）。
///
/// `ring` 是**从外往里**数的序号，`1` 表示最外圈。几何关系与
/// `assets/viz/center.wgsl` 里的 `mix(ring_inner, ring_outer, t)` 一致。
/// 正在播放页用它把封面裁成圆，让圆边正好落在指定的环上。
pub fn center_ring_radius(short: f32, config: &VisualizerConfig, ring: usize) -> f32 {
    let (_, radius_base, _) = geometry(short, config);
    let center_radius = radius_base * 0.92;

    let label_radius = center_radius * 0.36;
    let ring_inner = label_radius * 1.12;
    let ring_outer = center_radius * 0.99;

    let ring = ring.clamp(1, CENTER_RINGS);
    let t = (CENTER_RINGS - ring) as f32 / (CENTER_RINGS - 1) as f32;
    ring_inner + (ring_outer - ring_inner) * t
}

impl<'a> Program<Message> for VizProgram<'a> {
    type State = ();
    type Primitive = VizPrimitive;

    fn draw(
        &self,
        _state: &Self::State,
        _cursor: iced::mouse::Cursor,
        bounds: Rectangle,
    ) -> Self::Primitive {
        let width = bounds.width.max(1.0);
        let height = bounds.height.max(1.0);
        let cx = width / 2.0 + self.offset[0];
        let cy = height / 2.0 + self.offset[1];
        // 环的大小按舞台参考边长计算：画布可以铺满窗口，环不会跟着无限放大。
        let short = self.stage.max(1.0);

        let config = self.config;
        let (radius_outer, radius_base, max_len) = geometry(short, config);

        let count = self.scene.shown.len().min(MAX_BARS);
        let sector = TAU / count.max(1) as f32;
        let bar_width = (radius_base * sector * 0.42).max(0.9);
        let rotation = self.scene.rotation;
        let palette = config.palette;

        // ---- 条 ----
        let mut bars = Vec::with_capacity(count);
        let mut tips = Vec::with_capacity(count);
        for index in 0..count {
            let value = self.scene.shown[index].clamp(0.0, 1.0) as f32;
            let angle = index as f32 / count.max(1) as f32 * TAU - PI / 2.0 + rotation;
            let (sin, cos) = angle.sin_cos();
            let len = max_len * (0.06 + 0.94 * value);
            let p0 = [cx + cos * radius_base, cy + sin * radius_base];
            let p1 = [
                cx + cos * (radius_base + len),
                cy + sin * (radius_base + len),
            ];
            let color = palette.color_at(index as f32 / (count.max(2) - 1) as f32);
            bars.push(BarInstance {
                p0,
                p1,
                width: bar_width,
                _pad: 0.0,
                color: [color[0], color[1], color[2], 1.0],
            });
            tips.push(p1);
        }

        // ---- 顶端连线：每段两层（外晕 + 芯） ----
        let mut links = Vec::with_capacity(count * 2);
        if config.links && count > 2 {
            let mid = palette.color_at(0.5);
            let color = [
                mid[0] + (1.0 - mid[0]) * 0.15,
                mid[1] + (1.0 - mid[1]) * 0.15,
                mid[2] + (1.0 - mid[2]) * 0.15,
            ];
            for (index, &tip) in tips.iter().enumerate() {
                let next = tips[(index + 1) % count];
                for (width, alpha) in [(3.2, 0.16), (1.2, 0.5)] {
                    links.push(BarInstance {
                        p0: tip,
                        p1: next,
                        width,
                        _pad: 0.0,
                        color: [color[0], color[1], color[2], alpha],
                    });
                }
            }
        }

        // ---- 粒子 ----
        let mut particles = Vec::with_capacity(self.scene.particles.len().min(MAX_PARTICLES));
        for particle in self.scene.particles.iter().take(MAX_PARTICLES) {
            let (sin, cos) = particle.angle.sin_cos();
            let radius = particle.radius * radius_outer;
            let color = palette.color_at(particle.color_t);
            particles.push(ParticleInstance {
                center: [cx + cos * radius, cy + sin * radius],
                radius: particle.size,
                // 按自身寿命淡出：每颗粒子的寿命是随机的，所以淡出位置不会形成
                // 固定的圆。参数面板里的「粒子寿命 / 爆发寿命」就是控制它。
                alpha: (1.0 - particle.progress()).powf(1.5),
                color: [color[0], color[1], color[2], 1.0],
            });
        }

        // ---- 中心组件 ----
        // 低频能量取最外 4 根条的平均值；封面也用同一个值缩放（`center_bass`）。
        let bass = self.scene.center_bass();
        let center_radius = radius_base * 0.92 * center_scale(bass);
        let label_radius = center_radius * 0.36;

        let mut stops = [[0.0f32; 4]; CENTER_STOPS];
        let mut stop_count = 0u32;
        for (slot, color) in stops.iter_mut().zip(palette.colors().iter()) {
            *slot = [color[0], color[1], color[2], 1.0];
            stop_count += 1;
        }
        let mut ring_colors = [[0.0f32; 4]; CENTER_RINGS];
        for (slot, index) in ring_colors.iter_mut().zip(0..CENTER_RINGS) {
            let color = palette.color_at(index as f32 / (CENTER_RINGS - 1) as f32);
            *slot = [color[0], color[1], color[2], 1.0];
        }
        let center = CenterUniforms {
            center: [cx, cy],
            radius: center_radius,
            label_radius,
            spin: rotation,
            line_width: (center_radius * 0.012).max(1.2),
            ring_inner: label_radius * 1.12,
            ring_outer: center_radius * 0.99,
            bass,
            glow: config.center_glow.clamp(0.0, 2.0),
            stop_count,
            _pad: [0],
            stops,
            ring_colors,
        };

        // ---- 后处理参数 ----
        let afterglow = config.afterglow.clamp(AFTERGLOW_RANGE.0, AFTERGLOW_RANGE.1);
        let decay = if afterglow > 0.001 {
            let tau = 0.06 + 1.2 * afterglow * afterglow;
            (-(self.scene.last_dt / tau)).exp()
        } else {
            0.0
        };
        let crt = config.crt.clamp(0.0, 1.0);
        let barrel = config.barrel.clamp(0.0, 1.0) * crt * BARREL_MAX;
        let aberration = config.aberration.clamp(0.0, 1.0) * crt * ABERRATION_MAX;
        // 辉光跟着低频轻微呼吸：鼓点进来时整体更亮，安静时收回去。
        let energy = self.scene.bass().clamp(0.0, 1.0);
        let bloom = BLOOM_STRENGTH * config.glow.clamp(0.0, 2.0) * (0.82 + energy * 0.36);

        VizPrimitive {
            viewport: ViewportUniform {
                size: [width, height],
                decay,
                _pad: 0.0,
            },
            center,
            post: PostUniforms {
                bg: [0.0, 0.0, 0.0, 0.0],
                params: [barrel, aberration, bloom, 0.0],
            },
            bars,
            links,
            particles,
            center_on: center_radius > 2.0,
            bloom_on: bloom > 0.001,
        }
    }
}

/// 创建 iced 的 shader widget；宽高由调用方设置。
pub fn widget<'a>(
    scene: &'a RingScene,
    config: &'a VisualizerConfig,
    stage: f32,
    offset: [f32; 2],
) -> Shader<Message, VizProgram<'a>> {
    Shader::new(VizProgram {
        scene,
        config,
        stage,
        offset,
    })
}

impl Primitive for VizPrimitive {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        storage: &mut Storage,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        let scale = viewport.scale_factor() as f32;
        let physical = (
            ((bounds.width * scale).round() as u32).max(1),
            ((bounds.height * scale).round() as u32).max(1),
        );

        let recreate = match storage.get::<Gpu>() {
            Some(gpu) => gpu.physical != physical || gpu.format != format,
            None => true,
        };
        if recreate {
            storage.store(Gpu::new(device, physical.0, physical.1, format));
        }
        if let Some(gpu) = storage.get_mut::<Gpu>() {
            gpu.upload(queue, self);
        }
    }

    fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        storage: &Storage,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        let Some(gpu) = storage.get::<Gpu>() else {
            return;
        };
        if clip_bounds.width == 0 || clip_bounds.height == 0 {
            return;
        }
        gpu.render(encoder, target, clip_bounds, self.bloom_on);
    }
}

/// 常驻 GPU 资源。放在 iced 的 `shader::Storage` 里跨帧复用。
struct Gpu {
    physical: (u32, u32),
    format: wgpu::TextureFormat,

    bar_pipeline: wgpu::RenderPipeline,
    particle_pipeline: wgpu::RenderPipeline,
    center_pipeline: wgpu::RenderPipeline,
    fade_pipeline: wgpu::RenderPipeline,
    bloom_h_pipeline: wgpu::RenderPipeline,
    bloom_v_pipeline: wgpu::RenderPipeline,
    crt_pipeline: wgpu::RenderPipeline,

    viewport_buffer: wgpu::Buffer,
    viewport_bind_group: wgpu::BindGroup,
    center_buffer: wgpu::Buffer,
    center_bind_group: wgpu::BindGroup,
    post_buffer: wgpu::Buffer,

    bar_buffer: wgpu::Buffer,
    link_buffer: wgpu::Buffer,
    particle_buffer: wgpu::Buffer,

    _scene_textures: [wgpu::Texture; 2],
    scene_views: [wgpu::TextureView; 2],
    fade_bind_groups: [wgpu::BindGroup; 2],
    bloom_h_bind_groups: [wgpu::BindGroup; 2],
    _bloom_a_texture: wgpu::Texture,
    bloom_a_view: wgpu::TextureView,
    _bloom_b_texture: wgpu::Texture,
    bloom_b_view: wgpu::TextureView,
    bloom_v_bind_group: wgpu::BindGroup,
    crt_bind_groups: [wgpu::BindGroup; 2],

    /// 上一帧写入的纹理下标；本帧写另一张、读这张。
    scene_index: usize,
    cur: usize,
    prev: usize,
    /// 本帧的实例数量（条 / 连线段 / 粒子）。
    counts: (u32, u32, u32),
    center_on: bool,
    decay: f32,
}

impl Gpu {
    fn new(device: &wgpu::Device, width: u32, height: u32, format: wgpu::TextureFormat) -> Self {
        let bar_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viz bars"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../assets/viz/bar.wgsl").into()),
        });
        let particle_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viz particles"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../assets/viz/particle.wgsl").into()),
        });
        let center_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viz center"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../assets/viz/center.wgsl").into()),
        });
        let afterglow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viz afterglow"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../assets/viz/afterglow.wgsl").into()),
        });
        let bloom_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viz bloom"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../assets/viz/bloom.wgsl").into()),
        });
        let crt_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viz crt"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../assets/viz/crt.wgsl").into()),
        });

        let viewport_bgl = uniform_layout(device, "viz viewport");
        let center_bgl = uniform_layout(device, "viz center");
        let texture_bgl = texture_layout(device, "viz texture");

        let bar_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viz bars"),
            bind_group_layouts: &[&viewport_bgl],
            push_constant_ranges: &[],
        });
        let bar_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<BarInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &BAR_ATTRIBUTES,
        };
        let bar_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viz bars"),
            layout: Some(&bar_layout),
            vertex: wgpu::VertexState {
                module: &bar_shader,
                entry_point: "vs_main",
                buffers: &[bar_vertex_layout],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &bar_shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: SCENE_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
        });

        let particle_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ParticleInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &PARTICLE_ATTRIBUTES,
        };
        let particle_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viz particles"),
            layout: Some(&bar_layout),
            vertex: wgpu::VertexState {
                module: &particle_shader,
                entry_point: "vs_main",
                buffers: &[particle_vertex_layout],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &particle_shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: SCENE_FORMAT,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
        });

        let center_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viz center"),
            bind_group_layouts: &[&viewport_bgl, &center_bgl],
            push_constant_ranges: &[],
        });
        let center_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viz center"),
            layout: Some(&center_layout),
            vertex: wgpu::VertexState {
                module: &center_shader,
                entry_point: "vs_main",
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &center_shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: SCENE_FORMAT,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
        });

        let fade_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viz afterglow"),
            bind_group_layouts: &[&viewport_bgl, &texture_bgl],
            push_constant_ranges: &[],
        });
        let fade_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viz afterglow"),
            layout: Some(&fade_layout),
            vertex: wgpu::VertexState {
                module: &afterglow_shader,
                entry_point: "vs_full",
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &afterglow_shader,
                entry_point: "fs_fade",
                targets: &[Some(wgpu::ColorTargetState {
                    format: SCENE_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
        });

        let bloom_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viz bloom"),
            bind_group_layouts: &[&texture_bgl],
            push_constant_ranges: &[],
        });
        let make_bloom_pipeline = |entry: &'static str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&bloom_layout),
                vertex: wgpu::VertexState {
                    module: &bloom_shader,
                    entry_point: "vs_full",
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &bloom_shader,
                    entry_point: entry,
                    targets: &[Some(wgpu::ColorTargetState {
                        format: SCENE_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview: None,
            })
        };
        let bloom_h_pipeline = make_bloom_pipeline("fs_blur_h");
        let bloom_v_pipeline = make_bloom_pipeline("fs_blur_v");

        let crt_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viz crt"),
            entries: &[
                sampler_entry(),
                tex_entry(1),
                tex_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let crt_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("viz crt"),
            bind_group_layouts: &[&crt_bgl],
            push_constant_ranges: &[],
        });
        let crt_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viz crt"),
            layout: Some(&crt_layout),
            vertex: wgpu::VertexState {
                module: &crt_shader,
                entry_point: "vs_full",
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &crt_shader,
                entry_point: "fs_crt",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // 预乘 alpha：透明区域让 iced 已经画好的页面背景透出来。
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
        });

        let make_buffer = |label: &str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size.max(4),
                usage,
                mapped_at_creation: false,
            })
        };
        let uniform_usage = wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST;
        let viewport_buffer = make_buffer(
            "viz viewport",
            std::mem::size_of::<ViewportUniform>() as u64,
            uniform_usage,
        );
        let center_buffer = make_buffer(
            "viz center",
            std::mem::size_of::<CenterUniforms>() as u64,
            uniform_usage,
        );
        let post_buffer = make_buffer(
            "viz post",
            std::mem::size_of::<PostUniforms>() as u64,
            uniform_usage,
        );
        let instance_usage = wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST;
        let bar_buffer = make_buffer(
            "viz bar instances",
            (MAX_BARS * std::mem::size_of::<BarInstance>()) as u64,
            instance_usage,
        );
        let link_buffer = make_buffer(
            "viz link instances",
            (MAX_BARS * 2 * std::mem::size_of::<BarInstance>()) as u64,
            instance_usage,
        );
        let particle_buffer = make_buffer(
            "viz particle instances",
            (MAX_PARTICLES * std::mem::size_of::<ParticleInstance>()) as u64,
            instance_usage,
        );

        let viewport_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viz viewport"),
            layout: &viewport_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: viewport_buffer.as_entire_binding(),
            }],
        });
        let center_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viz center"),
            layout: &center_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: center_buffer.as_entire_binding(),
            }],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("viz post"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let (scene_a_texture, scene_a_view) = create_texture(device, "viz scene a", width, height);
        let (scene_b_texture, scene_b_view) = create_texture(device, "viz scene b", width, height);
        let scene_views = [scene_a_view, scene_b_view];
        let scene_textures = [scene_a_texture, scene_b_texture];

        let bw = (width / 4).max(1);
        let bh = (height / 4).max(1);
        let (bloom_a_texture, bloom_a_view) = create_texture(device, "viz bloom a", bw, bh);
        let (bloom_b_texture, bloom_b_view) = create_texture(device, "viz bloom b", bw, bh);

        let textured_bind = |label: &str, view: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &texture_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                ],
            })
        };
        let fade_bind_groups = [
            textured_bind("viz fade a", &scene_views[0]),
            textured_bind("viz fade b", &scene_views[1]),
        ];
        let bloom_h_bind_groups = [
            textured_bind("viz bloom h a", &scene_views[0]),
            textured_bind("viz bloom h b", &scene_views[1]),
        ];
        let bloom_v_bind_group = textured_bind("viz bloom v", &bloom_a_view);
        let make_crt_bind = |label: &str, scene: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &crt_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(scene),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&bloom_b_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: post_buffer.as_entire_binding(),
                    },
                ],
            })
        };
        let crt_bind_groups = [
            make_crt_bind("viz crt a", &scene_views[0]),
            make_crt_bind("viz crt b", &scene_views[1]),
        ];

        Self {
            physical: (width, height),
            format,
            bar_pipeline,
            particle_pipeline,
            center_pipeline,
            fade_pipeline,
            bloom_h_pipeline,
            bloom_v_pipeline,
            crt_pipeline,
            viewport_buffer,
            viewport_bind_group,
            center_buffer,
            center_bind_group,
            post_buffer,
            bar_buffer,
            link_buffer,
            particle_buffer,
            _scene_textures: scene_textures,
            scene_views,
            fade_bind_groups,
            bloom_h_bind_groups,
            _bloom_a_texture: bloom_a_texture,
            bloom_a_view,
            _bloom_b_texture: bloom_b_texture,
            bloom_b_view,
            bloom_v_bind_group,
            crt_bind_groups,
            scene_index: 0,
            cur: 1,
            prev: 0,
            counts: (0, 0, 0),
            center_on: false,
            decay: 0.0,
        }
    }

    fn upload(&mut self, queue: &wgpu::Queue, primitive: &VizPrimitive) {
        queue.write_buffer(
            &self.viewport_buffer,
            0,
            bytemuck::bytes_of(&primitive.viewport),
        );
        queue.write_buffer(
            &self.center_buffer,
            0,
            bytemuck::bytes_of(&primitive.center),
        );
        queue.write_buffer(&self.post_buffer, 0, bytemuck::bytes_of(&primitive.post));
        if !primitive.bars.is_empty() {
            queue.write_buffer(&self.bar_buffer, 0, bytemuck::cast_slice(&primitive.bars));
        }
        if !primitive.links.is_empty() {
            queue.write_buffer(&self.link_buffer, 0, bytemuck::cast_slice(&primitive.links));
        }
        if !primitive.particles.is_empty() {
            queue.write_buffer(
                &self.particle_buffer,
                0,
                bytemuck::cast_slice(&primitive.particles),
            );
        }

        // 与参考项目一致：本帧写另一张、读上一张。
        self.prev = self.scene_index;
        self.cur = 1 - self.prev;
        self.scene_index = self.cur;
        self.counts = (
            primitive.bars.len() as u32,
            (primitive.links.len() / 2) as u32,
            primitive.particles.len() as u32,
        );
        self.center_on = primitive.center_on;
        self.decay = primitive.viewport.decay;
    }

    fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
        bloom_on: bool,
    ) {
        let transparent = wgpu::Color {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        };

        // 1) 场景 → scene[cur]：淡出的上一帧打底，再画中心 / 条 / 连线 / 粒子。
        {
            let attachment = Some(wgpu::RenderPassColorAttachment {
                view: &self.scene_views[self.cur],
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(transparent),
                    store: wgpu::StoreOp::Store,
                },
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viz scene"),
                color_attachments: &[attachment],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // decay 已经由上传的 uniform 控制；为 0 时 fade 输出全透明，直接跳过省一趟。
            if self.viewport_decay() > 0.0 {
                pass.set_pipeline(&self.fade_pipeline);
                pass.set_bind_group(0, &self.viewport_bind_group, &[]);
                pass.set_bind_group(1, &self.fade_bind_groups[self.prev], &[]);
                pass.draw(0..3, 0..1);
            }
            if self.center_on {
                pass.set_pipeline(&self.center_pipeline);
                pass.set_bind_group(0, &self.viewport_bind_group, &[]);
                pass.set_bind_group(1, &self.center_bind_group, &[]);
                pass.draw(0..6, 0..1);
            }

            if self.bar_count() > 0 {
                pass.set_pipeline(&self.bar_pipeline);
                pass.set_bind_group(0, &self.viewport_bind_group, &[]);
                pass.set_vertex_buffer(0, self.bar_buffer.slice(..));
                pass.draw(0..6, 0..self.bar_count());
            }
            if self.link_count() > 0 {
                pass.set_pipeline(&self.bar_pipeline);
                pass.set_bind_group(0, &self.viewport_bind_group, &[]);
                pass.set_vertex_buffer(0, self.link_buffer.slice(..));
                pass.draw(0..6, 0..(self.link_count() * 2));
            }
            if self.particle_count() > 0 {
                pass.set_pipeline(&self.particle_pipeline);
                pass.set_bind_group(0, &self.viewport_bind_group, &[]);
                pass.set_vertex_buffer(0, self.particle_buffer.slice(..));
                pass.draw(0..6, 0..self.particle_count());
            }
        }

        // 2) bloom：scene --H--> a --V--> b
        if bloom_on {
            for (label, pipeline, bind_group, view) in [
                (
                    "viz bloom h",
                    &self.bloom_h_pipeline,
                    &self.bloom_h_bind_groups[self.cur],
                    &self.bloom_a_view,
                ),
                (
                    "viz bloom v",
                    &self.bloom_v_pipeline,
                    &self.bloom_v_bind_group,
                    &self.bloom_b_view,
                ),
            ] {
                let attachment = Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(transparent),
                        store: wgpu::StoreOp::Store,
                    },
                });
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(label),
                    color_attachments: &[attachment],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
        }

        // 3) CRT 合成 → iced 的目标纹理；只画在 widget 区域里。
        {
            let attachment = Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    // 保留页面背景，叠加式合成。
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viz crt"),
                color_attachments: &[attachment],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_viewport(
                clip_bounds.x as f32,
                clip_bounds.y as f32,
                clip_bounds.width as f32,
                clip_bounds.height as f32,
                0.0,
                1.0,
            );
            pass.set_scissor_rect(
                clip_bounds.x,
                clip_bounds.y,
                clip_bounds.width,
                clip_bounds.height,
            );
            pass.set_pipeline(&self.crt_pipeline);
            pass.set_bind_group(0, &self.crt_bind_groups[self.cur], &[]);
            pass.draw(0..3, 0..1);
        }
    }

    // 实例数量没有在 Gpu 里单独记录：uniform 里带着场景参数，但绘制数量需要运行时知道。
    // 这些值由 `upload` 时写入的“最后一份 primitive”提供；为避免再次加锁，直接在 Gpu
    // 里缓存计数。
    fn bar_count(&self) -> u32 {
        self.counts.0
    }

    fn link_count(&self) -> u32 {
        self.counts.1
    }

    fn particle_count(&self) -> u32 {
        self.counts.2
    }

    fn viewport_decay(&self) -> f32 {
        self.decay
    }
}

fn uniform_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
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
    })
}

fn texture_layout(device: &wgpu::Device, label: &str) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[sampler_entry(), tex_entry(1)],
    })
}

fn sampler_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn create_texture(
    device: &wgpu::Device,
    label: &str,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SCENE_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rings_are_evenly_spaced_from_outside_in() {
        let config = VisualizerConfig::default();
        let short = 600.0;

        let first = center_ring_radius(short, &config, 1);
        let last = center_ring_radius(short, &config, CENTER_RINGS);

        assert!(first > last, "从外往里应该越来越小");
        assert!(
            (first - last).abs() > 1.0,
            "环之间要有可见的间距：{first} vs {last}"
        );

        // 序号越界时钳到两端，不会把封面算到环外。
        assert_eq!(center_ring_radius(short, &config, 0), first);
        assert_eq!(center_ring_radius(short, &config, CENTER_RINGS + 3), last);
    }

    #[test]
    fn center_scale_tracks_bass_within_three_percent() {
        assert_eq!(center_scale(0.0), 1.0);
        assert!((center_scale(1.0) - 1.03).abs() < 1e-6);
        // 静音不会把中心 / 封面缩小，爆音也不会无限放大。
        assert_eq!(center_scale(-1.0), 1.0);
        assert!((center_scale(9.0) - 1.03).abs() < 1e-6);
    }

    #[test]
    fn sixth_ring_sits_between_inner_and_outer() {
        let config = VisualizerConfig::default();
        let short = 600.0;

        let sixth = center_ring_radius(short, &config, 6);
        let outer = center_ring_radius(short, &config, 1);
        let inner = center_ring_radius(short, &config, CENTER_RINGS);

        assert!(sixth < outer && sixth > inner);
        // 第 6 个圈在 10 个环里偏内：应小于环带中点。
        assert!(sixth < (inner + outer) * 0.5);
    }

    #[test]
    fn taller_bars_push_the_rings_inward() {
        let mut config = VisualizerConfig::default();
        let before = center_ring_radius(600.0, &config, 6);
        config.height = (config.height + 0.2).min(1.0);
        let after = center_ring_radius(600.0, &config, 6);
        assert!(after < before);
    }

    #[test]
    fn unlocked_ranges_can_overflow_the_stage() {
        let config = VisualizerConfig {
            size: 1.5,
            height: 3.0,
            ..VisualizerConfig::default()
        };
        let (outer, base, max_len) = geometry(600.0, &config);
        // size 允许超过 100%：600 的短边下外径 = 300 * 1.5。
        assert!((outer - 450.0).abs() < 1e-3);
        // 条高超过 100% 时底座收到圆心，条顺着往外溢出。
        assert_eq!(base, 0.0);
        assert!((max_len - 1_350.0).abs() < 1e-3);
        // 中心环也跟着收到圆心，不会算出负半径。
        assert_eq!(center_ring_radius(600.0, &config, 6), 0.0);
    }
}

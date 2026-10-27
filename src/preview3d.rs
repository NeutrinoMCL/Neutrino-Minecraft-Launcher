//! 3D 预览：用 wgpu 离屏渲染到纹理，再把纹理交给 Slint 的 `image` 属性显示。
//!
//! 目前渲染 Minecraft 皮肤（Steve / Alex / 自定义皮肤，含第二层帽子/外套/袖子/裤子）；
//! 之后原理图预览复用同一套设备、管线和目标纹理。

use image::RgbaImage;
use slint::wgpu_30::wgpu;
use wgpu::util::DeviceExt;

/// 预览纹理边长（正方形，显示时由 Slint 缩放）。
const TEXTURE_SIZE: u32 = 512;
/// 拖动一个像素对应的弧度。
const DRAG_SENSITIVITY: f32 = 0.01;
/// 披风自然下垂时向外倾斜的角度。
const CAPE_TILT: f32 = 20.0 * std::f32::consts::PI / 180.0;
/// 滚轮缩放的上下限，避免把模型放大到穿模或缩成一个小点。
const MIN_ZOOM: f32 = 0.30;
const MAX_ZOOM: f32 = 1.80;

fn apply_zoom(zoom: f32, delta: f32) -> f32 {
    // Slint 的 delta-y 正数对应滚轮前推：前推放大（相机拉近）。
    (zoom - delta * 0.001).clamp(MIN_ZOOM, MAX_ZOOM)
}
/// 俯仰限制：最多转到头朝外（+90°）或脚朝外（-90°）。
const MAX_PITCH: f32 = std::f32::consts::FRAC_PI_2;

type Mat4 = [[f32; 4]; 4];

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    /// 列主序的 model-view-projection 矩阵。
    mvp: Mat4,
}

pub struct Preview3d {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    skin_bind_group: wgpu::BindGroup,
    /// 披风纹理和网格都是可选的；没有自定义披风时不绘制。
    cape_bind_group: Option<wgpu::BindGroup>,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    vertices: Option<wgpu::Buffer>,
    indices: Option<wgpu::Buffer>,
    index_count: u32,
    cape_vertices: Option<wgpu::Buffer>,
    cape_indices: Option<wgpu::Buffer>,
    cape_index_count: u32,
    cape_enabled: bool,
    /// 皮肤 / 披风纹理要一直持有，否则 bind group 会引用已释放的资源。
    _skin_texture: Option<wgpu::Texture>,
    _cape_texture: Option<wgpu::Texture>,
    color_texture: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    /// 水平旋转角（绕世界 Y 轴，360° 自由）。
    yaw: f32,
    /// 俯仰角，限制在 ±90°：立正姿势最多转到头朝外或脚朝外。
    pitch: f32,
    /// 相机距离缩放：1.0 为默认，越小越近。
    zoom: f32,
    /// 浅色主题下用白色背景。
    light_background: bool,
    /// 上一次的拖动偏移（相对按下点的绝对偏移）。
    last_offset: (f32, f32),
}

impl Preview3d {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("preview3d.shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("preview3d.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("preview3d.bind-group-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("preview3d.uniforms"),
            contents: bytemuck::bytes_of(&Uniforms { mvp: identity() }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // 皮肤要用最近邻采样，保持像素风。
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("preview3d.sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let placeholder = create_texture(&device, &queue, 1, 1, &[255, 255, 255, 255]);
        let skin_bind_group = create_bind_group(
            &device,
            &bind_group_layout,
            &uniforms,
            &placeholder,
            &sampler,
        );

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("preview3d.pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
            wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("preview3d.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &ATTRIBUTES,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    // 第二层（帽子/外套等）有半透明像素，需要 alpha 混合。
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        // Slint 只接受 Rgba8Unorm / Rgba8UnormSrgb，并且必须同时带
        // RENDER_ATTACHMENT + TEXTURE_BINDING 才能导入成 Image。
        let color_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("preview3d.color"),
            size: wgpu::Extent3d {
                width: TEXTURE_SIZE,
                height: TEXTURE_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let color_view = color_texture.create_view(&Default::default());

        let depth_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("preview3d.depth"),
            size: wgpu::Extent3d {
                width: TEXTURE_SIZE,
                height: TEXTURE_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth_texture.create_view(&Default::default());

        Self {
            device,
            queue,
            pipeline,
            bind_group_layout,
            skin_bind_group,
            cape_bind_group: None,
            sampler,
            uniforms,
            vertices: None,
            indices: None,
            index_count: 0,
            cape_vertices: None,
            cape_indices: None,
            cape_index_count: 0,
            cape_enabled: false,
            _skin_texture: Some(placeholder),
            _cape_texture: None,
            color_texture,
            color_view,
            depth_view,
            // 稍微转一点，露出正面和侧面
            yaw: 0.35,
            pitch: 0.0,
            zoom: 1.0,
            light_background: false,
            last_offset: (0.0, 0.0),
        }
    }

    /// 把当前纹理包装成 Slint 图片。纹理内容变了之后要重新调用一次并塞回属性。
    pub fn image(&self) -> Option<slint::Image> {
        slint::Image::try_from(self.color_texture.clone()).ok()
    }

    /// 换一张皮肤并重建网格。返回 `false` 表示图片不是合法的皮肤。
    pub fn set_skin(&mut self, skin: &RgbaImage) -> bool {
        let Some((vertices, indices)) = build_skin(skin) else {
            return false;
        };

        let (width, height) = skin.dimensions();
        let texture = create_texture(&self.device, &self.queue, width, height, skin.as_raw());
        self.skin_bind_group = create_bind_group(
            &self.device,
            &self.bind_group_layout,
            &self.uniforms,
            &texture,
            &self.sampler,
        );
        self.vertices = Some(
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("preview3d.vertices"),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
        );
        self.indices = Some(
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("preview3d.indices"),
                    contents: bytemuck::cast_slice(&indices),
                    usage: wgpu::BufferUsages::INDEX,
                }),
        );
        self.index_count = indices.len() as u32;
        self._skin_texture = Some(texture);
        true
    }

    /// 设置自定义披风并重建网格；返回 `false` 表示不是合法的披风模板。
    pub fn set_cape(&mut self, cape: &RgbaImage) -> bool {
        let Some((vertices, indices)) = build_cape(cape) else {
            return false;
        };

        let (width, height) = cape.dimensions();
        let texture = create_texture(&self.device, &self.queue, width, height, cape.as_raw());
        let bind_group = create_bind_group(
            &self.device,
            &self.bind_group_layout,
            &self.uniforms,
            &texture,
            &self.sampler,
        );
        self.cape_vertices = Some(self.device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("preview3d.cape-vertices"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            },
        ));
        self.cape_indices = Some(self.device.create_buffer_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("preview3d.cape-indices"),
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX,
            },
        ));
        self.cape_index_count = indices.len() as u32;
        self.cape_bind_group = Some(bind_group);
        self._cape_texture = Some(texture);
        self.cape_enabled = true;
        true
    }

    /// 移除披风并停止绘制。
    pub fn clear_cape(&mut self) {
        self.cape_vertices = None;
        self.cape_indices = None;
        self.cape_index_count = 0;
        self.cape_bind_group = None;
        self._cape_texture = None;
        self.cape_enabled = false;
    }

    /// 开启 / 关闭披风绘制；没有披风网格时会保持关闭。
    pub fn set_cape_visible(&mut self, visible: bool) {
        self.cape_enabled = visible && self.cape_index_count > 0;
    }

    /// 开始一次拖拽：偏移量从 0 重新算起。
    pub fn begin_drag(&mut self) {
        self.last_offset = (0.0, 0.0);
    }

    /// 切换预览背景：浅色主题用白色，深色主题保持暗色。
    pub fn set_light_background(&mut self, light: bool) {
        self.light_background = light;
    }

    /// `dx` / `dy` 是鼠标相对按下点的位移（像素）。
    pub fn drag_to(&mut self, dx: f32, dy: f32) {
        let step_x = dx - self.last_offset.0;
        let step_y = dy - self.last_offset.1;
        self.last_offset = (dx, dy);

        self.yaw += step_x * DRAG_SENSITIVITY;
        self.pitch = (self.pitch + step_y * DRAG_SENSITIVITY).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// 滚轮缩放：`delta` 是 Slint 的滚轮 delta-y，正数（前推）拉近。
    pub fn zoom_by(&mut self, delta: f32) {
        self.zoom = apply_zoom(self.zoom, delta);
    }

    pub fn render(&mut self) {
        let projection = perspective(32f32.to_radians(), 1.0, 0.1, 500.0);
        let view = look_at(
            [0.0, 9.0 * self.zoom, 78.0 * self.zoom],
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        );

        // 先水平转（绕世界 Y），再俯仰（绕世界 X），俯仰角已经被限制住
        let model = mul(rotate_x(self.pitch), rotate_y(self.yaw));
        let uniforms = Uniforms {
            mvp: mul(mul(projection, view), model),
        };
        self.queue
            .write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("preview3d.encoder"),
            });

        let clear = if self.light_background {
            wgpu::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            }
        } else {
            wgpu::Color {
                r: 0.07,
                g: 0.09,
                b: 0.13,
                a: 1.0,
            }
        };

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("preview3d.pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if let (Some(vertices), Some(indices)) = (&self.vertices, &self.indices) {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.skin_bind_group, &[]);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(0..self.index_count, 0, 0..1);
            }

            if self.cape_enabled
                && let (Some(vertices), Some(indices), Some(bind_group)) = (
                    &self.cape_vertices,
                    &self.cape_indices,
                    &self.cape_bind_group,
                )
            {
                pass.set_bind_group(0, bind_group, &[]);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint16);
                pass.draw_indexed(0..self.cape_index_count, 0, 0..1);
            }
        }

        self.queue.submit([encoder.finish()]);
    }
}

fn create_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("preview3d.texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * width),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    texture
}

fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniforms: &wgpu::Buffer,
    skin: &wgpu::Texture,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("preview3d.bind-group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(
                    &skin.create_view(&Default::default()),
                ),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

/// 一个长方体在模型里的位置、尺寸，以及它在皮肤模板里的展开原点。
struct SkinBox {
    center: [f32; 3],
    size: [f32; 3],
    /// 模板坐标（64×64 皮肤里的像素坐标）。
    uv: [f32; 2],
    /// 旧版 64×32 皮肤没有左半身，复用右半身并水平镜像。
    mirror: bool,
    /// 外层（帽子/外套/袖子/裤子）向外膨胀的厚度，内层为 0。
    inflate: f32,
}

/// 从皮肤 PNG 生成模型网格。模型以原点为中心，y 范围约 -16..16。
///
/// 64×64 皮肤会把第二层（帽子/外套/袖子/裤子）一起渲染，半透明像素走 alpha 混合。
fn build_skin(skin: &RgbaImage) -> Option<(Vec<Vertex>, Vec<u16>)> {
    let (width, height) = skin.dimensions();
    let scale = width / 64;
    if scale == 0 || width % 64 != 0 {
        return None;
    }

    let template_height = height / scale;
    let legacy = template_height == 32;
    if template_height != 64 && !legacy {
        return None;
    }

    // 64×64 皮肤里 (54, 20) 透明表示 Alex 的细手臂。
    let slim = !legacy && skin.get_pixel(54 * scale, 20 * scale)[3] == 0;
    let arm_width = if slim { 3.0 } else { 4.0 };

    let uv_scale = [1.0 / 64.0, 1.0 / template_height as f32];
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    let boxes = [
        // ---- 内层 ----
        SkinBox {
            center: [0.0, 12.0, 0.0],
            size: [8.0, 8.0, 8.0],
            uv: [0.0, 0.0],
            mirror: false,
            inflate: 0.0,
        },
        SkinBox {
            center: [0.0, 2.0, 0.0],
            size: [8.0, 12.0, 4.0],
            uv: [16.0, 16.0],
            mirror: false,
            inflate: 0.0,
        },
        SkinBox {
            center: [-(4.0 + arm_width / 2.0), 2.0, 0.0],
            size: [arm_width, 12.0, 4.0],
            uv: [40.0, 16.0],
            mirror: false,
            inflate: 0.0,
        },
        SkinBox {
            center: [4.0 + arm_width / 2.0, 2.0, 0.0],
            size: [arm_width, 12.0, 4.0],
            uv: if legacy { [40.0, 16.0] } else { [32.0, 48.0] },
            mirror: legacy,
            inflate: 0.0,
        },
        SkinBox {
            center: [-2.0, -10.0, 0.0],
            size: [4.0, 12.0, 4.0],
            uv: [0.0, 16.0],
            mirror: false,
            inflate: 0.0,
        },
        SkinBox {
            center: [2.0, -10.0, 0.0],
            size: [4.0, 12.0, 4.0],
            uv: if legacy { [0.0, 16.0] } else { [16.0, 48.0] },
            mirror: legacy,
            inflate: 0.0,
        },
        // ---- 外层（旧版 64×32 没有第二层）----
        SkinBox {
            center: [0.0, 12.0, 0.0],
            size: [8.0, 8.0, 8.0],
            uv: [32.0, 0.0],
            mirror: false,
            inflate: 0.5,
        },
        SkinBox {
            center: [0.0, 2.0, 0.0],
            size: [8.0, 12.0, 4.0],
            uv: [16.0, 32.0],
            mirror: false,
            inflate: 0.25,
        },
        SkinBox {
            center: [-(4.0 + arm_width / 2.0), 2.0, 0.0],
            size: [arm_width, 12.0, 4.0],
            uv: [40.0, 32.0],
            mirror: false,
            inflate: 0.25,
        },
        SkinBox {
            center: [4.0 + arm_width / 2.0, 2.0, 0.0],
            size: [arm_width, 12.0, 4.0],
            uv: [48.0, 48.0],
            mirror: false,
            inflate: 0.25,
        },
        SkinBox {
            center: [-2.0, -10.0, 0.0],
            size: [4.0, 12.0, 4.0],
            uv: [0.0, 32.0],
            mirror: false,
            inflate: 0.25,
        },
        SkinBox {
            center: [2.0, -10.0, 0.0],
            size: [4.0, 12.0, 4.0],
            uv: [0.0, 48.0],
            mirror: false,
            inflate: 0.25,
        },
    ];

    for skin_box in boxes.iter().take(if legacy { 6 } else { 12 }) {
        push_box(&mut vertices, &mut indices, uv_scale, skin_box);
    }

    Some((vertices, indices))
}

/// 从 64×32 / 128×64 的披风模板生成带厚度的网格。
///
/// 采用标准模板布局（正面 / 背面 / 左右侧条 / 上下边条），模型尺寸为
/// 10×16×1，挂在肩膀下方、身体后方。
fn build_cape(cape: &RgbaImage) -> Option<(Vec<Vertex>, Vec<u16>)> {
    let (width, height) = cape.dimensions();
    if width < 64 || width % 64 != 0 || height != width / 2 {
        return None;
    }

    let scale = (width / 64) as f32;
    let uv_scale = [1.0 / width as f32, 1.0 / height as f32];
    // 标准 64×32 披风模板：正面 (1,1)-(10,16)，背面 (12,1)-(21,16)，
    // 左右侧条 (0,1) / (11,1)，上下边条 (1,0) / (11,0)。HD 模板按 2× 缩放。
    let front = [scale, scale, 10.0 * scale, 16.0 * scale];
    let back = [12.0 * scale, scale, 10.0 * scale, 16.0 * scale];
    let right = [0.0, scale, scale, 16.0 * scale];
    let left = [11.0 * scale, scale, scale, 16.0 * scale];
    let top = [scale, 0.0, 10.0 * scale, scale];
    let bottom = [11.0 * scale, 0.0, 10.0 * scale, scale];
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    // 背面（朝 -Z，从身后看）
    push_face(
        &mut vertices,
        &mut indices,
        uv_scale,
        back,
        [0.0, 0.0, -3.4],
        [0.0, 0.0, -1.0],
        [-1.0, 0.0, 0.0],
        [0.0, -1.0, 0.0],
        10.0,
        16.0,
        false,
    );
    // 正面（朝 +Z，披风内侧）
    push_face(
        &mut vertices,
        &mut indices,
        uv_scale,
        front,
        [0.0, 0.0, -2.4],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 0.0],
        [0.0, -1.0, 0.0],
        10.0,
        16.0,
        false,
    );

    // 四个侧面把前后两层连成一个有厚度的薄盒。
    let edge_z = -2.9;
    let thickness = 1.0;
    // 左侧（-X）
    push_face(
        &mut vertices,
        &mut indices,
        uv_scale,
        left,
        [-5.0, 0.0, edge_z],
        [-1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, -1.0, 0.0],
        thickness,
        16.0,
        false,
    );
    // 右侧（+X）
    push_face(
        &mut vertices,
        &mut indices,
        uv_scale,
        right,
        [5.0, 0.0, edge_z],
        [1.0, 0.0, 0.0],
        [0.0, 0.0, -1.0],
        [0.0, -1.0, 0.0],
        thickness,
        16.0,
        false,
    );
    // 顶部（+Y）
    push_face(
        &mut vertices,
        &mut indices,
        uv_scale,
        top,
        [0.0, 8.0, edge_z],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        10.0,
        thickness,
        false,
    );
    // 底部（-Y）
    push_face(
        &mut vertices,
        &mut indices,
        uv_scale,
        bottom,
        [0.0, -8.0, edge_z],
        [0.0, -1.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 0.0, -1.0],
        10.0,
        thickness,
        false,
    );

    tilt_cape(&mut vertices);
    Some((vertices, indices))
}

/// 让披风以肩膀为轴向外倾斜，而不是垂直贴在后背上。
fn tilt_cape(vertices: &mut [Vertex]) {
    let (sin, cos) = CAPE_TILT.sin_cos();
    let pivot_y = 8.0;
    let pivot_z = -2.9;

    for vertex in vertices {
        let dy = vertex.position[1] - pivot_y;
        let dz = vertex.position[2] - pivot_z;
        vertex.position[1] = pivot_y + dy * cos - dz * sin;
        vertex.position[2] = pivot_z + dy * sin + dz * cos;

        let normal_y = vertex.normal[1];
        let normal_z = vertex.normal[2];
        vertex.normal[1] = normal_y * cos - normal_z * sin;
        vertex.normal[2] = normal_y * sin + normal_z * cos;
    }
}

fn push_box(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u16>,
    uv_scale: [f32; 2],
    skin_box: &SkinBox,
) {
    let inflate = skin_box.inflate;
    let [tex_width, tex_height, tex_depth] = skin_box.size;
    // 几何尺寸：外层（帽子/外套等）向外膨胀；贴图仍按原始展开尺寸计算 UV
    let width = tex_width + inflate * 2.0;
    let height = tex_height + inflate * 2.0;
    let depth = tex_depth + inflate * 2.0;
    let [center_x, center_y, center_z] = skin_box.center;
    let [u, v] = skin_box.uv;
    let mirror = skin_box.mirror;

    // 右（-X）
    push_face(
        vertices,
        indices,
        uv_scale,
        [u, v + tex_depth, tex_depth, tex_height],
        [center_x - width / 2.0, center_y, center_z],
        [-1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, -1.0, 0.0],
        depth,
        height,
        mirror,
    );
    // 左（+X）
    push_face(
        vertices,
        indices,
        uv_scale,
        [
            u + tex_depth + tex_width,
            v + tex_depth,
            tex_depth,
            tex_height,
        ],
        [center_x + width / 2.0, center_y, center_z],
        [1.0, 0.0, 0.0],
        [0.0, 0.0, -1.0],
        [0.0, -1.0, 0.0],
        depth,
        height,
        mirror,
    );
    // 前（+Z）
    push_face(
        vertices,
        indices,
        uv_scale,
        [u + tex_depth, v + tex_depth, tex_width, tex_height],
        [center_x, center_y, center_z + depth / 2.0],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 0.0],
        [0.0, -1.0, 0.0],
        width,
        height,
        mirror,
    );
    // 后（-Z）
    push_face(
        vertices,
        indices,
        uv_scale,
        [
            u + tex_depth + tex_width + tex_depth,
            v + tex_depth,
            tex_width,
            tex_height,
        ],
        [center_x, center_y, center_z - depth / 2.0],
        [0.0, 0.0, -1.0],
        [-1.0, 0.0, 0.0],
        [0.0, -1.0, 0.0],
        width,
        height,
        mirror,
    );
    // 顶（+Y）
    push_face(
        vertices,
        indices,
        uv_scale,
        [u + tex_depth, v, tex_width, tex_depth],
        [center_x, center_y + height / 2.0, center_z],
        [0.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        width,
        depth,
        mirror,
    );
    // 底（-Y）
    push_face(
        vertices,
        indices,
        uv_scale,
        [u + tex_depth + tex_width, v, tex_width, tex_depth],
        [center_x, center_y - height / 2.0, center_z],
        [0.0, -1.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 0.0, -1.0],
        width,
        depth,
        mirror,
    );
}

/// 往网格里加一个面。`rect` 是皮肤模板里的像素矩形；`u_dir` / `v_dir` 是纹理
/// u / v 增大方向在模型空间里的方向（v 向下），`u_size` / `v_size` 是面的尺寸。
#[allow(clippy::too_many_arguments)]
fn push_face(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u16>,
    uv_scale: [f32; 2],
    rect: [f32; 4],
    center: [f32; 3],
    normal: [f32; 3],
    u_dir: [f32; 3],
    v_dir: [f32; 3],
    u_size: f32,
    v_size: f32,
    mirror: bool,
) {
    let base = vertices.len() as u16;
    let half_u = mul3(u_dir, u_size / 2.0);
    let half_v = mul3(v_dir, v_size / 2.0);

    let corner =
        |u_sign: f32, v_sign: f32| add(center, add(mul3(half_u, u_sign), mul3(half_v, v_sign)));
    let positions = [
        corner(-1.0, -1.0),
        corner(-1.0, 1.0),
        corner(1.0, 1.0),
        corner(1.0, -1.0),
    ];

    let [u0, v0, rect_width, rect_height] = rect;
    let (u_left, u_right) = if mirror {
        (u0 + rect_width, u0)
    } else {
        (u0, u0 + rect_width)
    };
    let uvs = [
        [u_left, v0],
        [u_left, v0 + rect_height],
        [u_right, v0 + rect_height],
        [u_right, v0],
    ];

    for (position, uv) in positions.into_iter().zip(uvs) {
        vertices.push(Vertex {
            position,
            normal,
            uv: [uv[0] * uv_scale[0], uv[1] * uv_scale[1]],
        });
    }

    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn mul3(a: [f32; 3], scale: f32) -> [f32; 3] {
    [a[0] * scale, a[1] * scale, a[2] * scale]
}

fn identity() -> Mat4 {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// 列主序矩阵相乘：`a * b`。
fn mul(a: Mat4, b: Mat4) -> Mat4 {
    let mut out = [[0.0f32; 4]; 4];
    for column in 0..4 {
        for row in 0..4 {
            out[column][row] = (0..4).map(|k| a[k][row] * b[column][k]).sum();
        }
    }
    out
}

fn rotate_x(angle: f32) -> Mat4 {
    let (sin, cos) = angle.sin_cos();
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, cos, sin, 0.0],
        [0.0, -sin, cos, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn rotate_y(angle: f32) -> Mat4 {
    let (sin, cos) = angle.sin_cos();
    [
        [cos, 0.0, -sin, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [sin, 0.0, cos, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// wgpu 的深度范围是 0..1（和 OpenGL 不同）。
fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fovy / 2.0).tan();
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, far / (near - far), -1.0],
        [0.0, 0.0, near * far / (near - far), 0.0],
    ]
}

fn look_at(eye: [f32; 3], center: [f32; 3], up: [f32; 3]) -> Mat4 {
    let f = normalize(sub(center, eye));
    let s = normalize(cross(f, up));
    let u = cross(s, f);

    [
        [s[0], u[0], -f[0], 0.0],
        [s[1], u[1], -f[1], 0.0],
        [s[2], u[2], -f[2], 0.0],
        [-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0],
    ]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(a: [f32; 3]) -> [f32; 3] {
    let length = dot(a, a).sqrt();
    if length == 0.0 {
        a
    } else {
        [a[0] / length, a[1] / length, a[2] / length]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skin(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_pixel(width, height, image::Rgba([128, 128, 128, 255]))
    }

    #[test]
    fn skin_mesh_has_outward_winding() {
        let (vertices, indices) = build_skin(&skin(64, 64)).expect("64x64 是合法皮肤");

        // 6 个内层 + 6 个外层，每个 6 面 × 4 顶点
        assert_eq!(vertices.len(), 12 * 6 * 4);
        assert_eq!(indices.len(), 12 * 6 * 6);

        for triangle in indices.chunks(3) {
            let a = vertices[triangle[0] as usize];
            let b = vertices[triangle[1] as usize];
            let c = vertices[triangle[2] as usize];

            let edge1 = sub(b.position, a.position);
            let edge2 = sub(c.position, a.position);
            let facing = dot(cross(edge1, edge2), a.normal);

            assert!(facing > 0.0, "三角形绕向和法线不一致");
        }
    }

    #[test]
    fn legacy_skin_has_no_outer_layer() {
        let (vertices, _) = build_skin(&skin(64, 32)).expect("64x32 是合法皮肤");
        assert_eq!(vertices.len(), 6 * 6 * 4);
    }

    #[test]
    fn cape_mesh_is_closed_and_outward() {
        let (vertices, indices) = build_cape(&skin(64, 32)).expect("64x32 是合法披风");

        // 前后两面 + 四个侧面，每个面 4 个顶点、2 个三角形
        assert_eq!(vertices.len(), 24);
        assert_eq!(indices.len(), 36);

        let min_z = vertices
            .iter()
            .map(|vertex| vertex.position[2])
            .fold(f32::MAX, f32::min);
        assert!(min_z < -7.0, "披风底部应该向外倾斜");

        for triangle in indices.chunks(3) {
            let a = vertices[triangle[0] as usize];
            let b = vertices[triangle[1] as usize];
            let c = vertices[triangle[2] as usize];

            let edge1 = sub(b.position, a.position);
            let edge2 = sub(c.position, a.position);
            let facing = dot(cross(edge1, edge2), a.normal);
            assert!(facing > 0.0, "披风三角形绕向和法线不一致");
        }
    }

    #[test]
    fn rejects_images_that_are_not_cape_templates() {
        assert!(build_cape(&skin(64, 64)).is_none());
        assert!(build_cape(&skin(32, 16)).is_none());
        assert!(build_cape(&skin(128, 64)).is_some());
    }

    #[test]
    fn cape_uv_stays_inside_the_texture() {
        for (width, height) in [(64, 32), (128, 64)] {
            let (vertices, _) = build_cape(&skin(width, height)).expect("合法披风");
            for vertex in vertices {
                assert!((0.0..=1.0).contains(&vertex.uv[0]));
                assert!((0.0..=1.0).contains(&vertex.uv[1]));
            }
        }
    }

    #[test]
    fn skin_uv_stays_inside_the_texture() {
        for (width, height) in [(64, 64), (64, 32), (128, 128)] {
            let (vertices, _) = build_skin(&skin(width, height)).expect("合法皮肤");
            for vertex in vertices {
                assert!(
                    (0.0..=1.0).contains(&vertex.uv[0]),
                    "u 越界：{:?} ({width}x{height})",
                    vertex.uv
                );
                assert!(
                    (0.0..=1.0).contains(&vertex.uv[1]),
                    "v 越界：{:?} ({width}x{height})",
                    vertex.uv
                );
            }
        }
    }

    #[test]
    fn rejects_images_that_are_not_skin_templates() {
        assert!(build_skin(&skin(32, 32)).is_none());
        assert!(build_skin(&skin(64, 48)).is_none());
    }

    #[test]
    fn slim_arms_are_detected_from_the_skin() {
        let mut classic = skin(64, 64);
        classic.put_pixel(54, 20, image::Rgba([0, 0, 0, 255]));
        let mut slim = skin(64, 64);
        slim.put_pixel(54, 20, image::Rgba([0, 0, 0, 0]));

        let (classic_vertices, _) = build_skin(&classic).unwrap();
        let (slim_vertices, _) = build_skin(&slim).unwrap();

        // 手臂变窄后，左右手臂外沿（含 0.25 的外层膨胀）会从 ±8.25 变成 ±7.25
        let min_x = slim_vertices
            .iter()
            .map(|v| v.position[0])
            .fold(f32::MAX, f32::min);
        let classic_min_x = classic_vertices
            .iter()
            .map(|v| v.position[0])
            .fold(f32::MAX, f32::min);

        assert!((classic_min_x + 8.25).abs() < 1e-4);
        assert!((min_x + 7.25).abs() < 1e-4);
    }

    #[test]
    fn overlay_layers_are_slightly_larger() {
        let (vertices, _) = build_skin(&skin(64, 64)).unwrap();

        // 帽子的外层比头大 1（每边 0.5），即 x/z 到 ±4.5、y 到 16.5
        let hat_max_y = vertices
            .iter()
            .map(|v| v.position[1])
            .fold(f32::MIN, f32::max);
        let hat_max_x = vertices
            .iter()
            .map(|v| v.position[0])
            .fold(f32::MIN, f32::max);

        assert!((hat_max_y - 16.5).abs() < 1e-4);
        assert!((hat_max_x - 8.25).abs() < 1e-4, "手臂外层最宽到 8.25");
    }

    #[test]
    fn zoom_is_limited() {
        let mut zoom = 1.0;
        for _ in 0..100 {
            zoom = apply_zoom(zoom, 100.0);
        }
        assert!((zoom - MIN_ZOOM).abs() < 1e-6);

        for _ in 0..100 {
            zoom = apply_zoom(zoom, -100.0);
        }
        assert!((zoom - MAX_ZOOM).abs() < 1e-6);

        assert!(apply_zoom(1.0, 10.0) < 1.0);
        assert!(apply_zoom(1.0, -10.0) > 1.0);
    }

    #[test]
    fn perspective_maps_near_far_to_zero_one() {
        let projection = perspective(40f32.to_radians(), 1.0, 0.5, 50.0);
        let near_point = transform(projection, [0.0, 0.0, -0.5]);
        let far_point = transform(projection, [0.0, 0.0, -50.0]);

        assert!((near_point - 0.0).abs() < 1e-5, "near 应该在 0");
        assert!((far_point - 1.0).abs() < 1e-5, "far 应该在 1");
    }

    #[test]
    fn drag_direction_follows_the_mouse() {
        // 向右拖：yaw 增加，正面（+Z）朝 +X 移动
        let model = mul(rotate_x(0.0), rotate_y(0.3));
        let front = rotate_dir(model, [0.0, 0.0, 1.0]);
        assert!(front[0] > 0.2, "向右拖时正面应该往右走");

        // 向下拖：pitch 增加，正面朝 -Y 移动
        let model = mul(rotate_x(0.3), rotate_y(0.0));
        let front = rotate_dir(model, [0.0, 0.0, 1.0]);
        assert!(front[1] < -0.2, "向下拖时正面应该往下走");
    }

    #[test]
    fn pitch_is_clamped_to_head_or_feet_outward() {
        // 一直向下拖：pitch 顶到 +90°，头朝外（+Z）
        let mut pitch: f32 = 0.0;
        for _ in 0..100 {
            pitch = (pitch + 0.05).clamp(-MAX_PITCH, MAX_PITCH);
        }
        assert!((pitch - MAX_PITCH).abs() < 1e-6);
        let head = rotate_dir(mul(rotate_x(pitch), rotate_y(0.0)), [0.0, 1.0, 0.0]);
        assert!(head[2] > 0.99, "转到底时头应该朝向相机");

        // 一直向上拖：pitch 顶到 -90°，脚朝外（+Z）
        let mut pitch: f32 = 0.0;
        for _ in 0..100 {
            pitch = (pitch - 0.05).clamp(-MAX_PITCH, MAX_PITCH);
        }
        assert!((pitch + MAX_PITCH).abs() < 1e-6);
        let feet = rotate_dir(mul(rotate_x(pitch), rotate_y(0.0)), [0.0, -1.0, 0.0]);
        assert!(feet[2] > 0.99, "转到底时脚应该朝向相机");
    }

    #[test]
    fn yaw_keeps_the_model_orthonormal() {
        let mut yaw: f32 = 0.0;
        for _ in 0..200 {
            yaw += 0.17;
        }

        let model = mul(rotate_x(0.4), rotate_y(yaw));
        let x = rotate_dir(model, [1.0, 0.0, 0.0]);
        let y = rotate_dir(model, [0.0, 1.0, 0.0]);
        let z = rotate_dir(model, [0.0, 0.0, 1.0]);

        for axis in [x, y, z] {
            assert!(
                (dot(axis, axis) - 1.0).abs() < 1e-4,
                "旋转矩阵应该保持单位长度"
            );
        }
        assert!(dot(x, y).abs() < 1e-4);
        assert!(dot(y, z).abs() < 1e-4);
        assert!(dot(x, z).abs() < 1e-4);
    }

    /// 只取旋转部分（3×3）作用到方向上。
    fn rotate_dir(m: Mat4, v: [f32; 3]) -> [f32; 3] {
        [
            m[0][0] * v[0] + m[1][0] * v[1] + m[2][0] * v[2],
            m[0][1] * v[0] + m[1][1] * v[1] + m[2][1] * v[2],
            m[0][2] * v[0] + m[1][2] * v[1] + m[2][2] * v[2],
        ]
    }

    fn transform(m: Mat4, point: [f32; 3]) -> f32 {
        let z = m[0][2] * point[0] + m[1][2] * point[1] + m[2][2] * point[2] + m[3][2];
        let w = m[0][3] * point[0] + m[1][3] * point[1] + m[2][3] * point[2] + m[3][3];
        z / w
    }
}

use std::{mem::size_of, num::NonZeroU64};

use bytemuck::{cast_slice, Pod, Zeroable};
use cgmath::Matrix3;

use rgine_graphics::{
    color::Color3,
    ctx::{Frame, GraphicsCtx},
};
use wgpu::{util::StagingBelt, *};

use crate::texture::{Atlas, DrawParams, Sprite, SpriteSheetsRegistry};

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct SpriteInstance {
    transform: [[f32; 3]; 3],
    tex_pos: [f32; 2],
    tex_dims: [f32; 2],
    tint: [f32; 3],
    z_index: f32,
}

pub struct SpriteRenderer {
    pipeline: RenderPipeline,
    depth_texture: Texture,
    depth_texture_view: TextureView,
    depth_texture_sampler: Sampler,
    quad_vertex_buf: Buffer,
    quad_index_buf: Buffer,
    sprite_instance_buf: Buffer,
    sprite_staging_belt: StagingBelt,

    proj_matrix: Matrix3<f32>,
    atlas: Atlas,
    queue: Vec<SpriteInstance>,
}

const MAX_BATCHES: u64 = 100;
const MAX_SPRITES_PER_BATCH: u64 = 5_000;

impl SpriteRenderer {
    pub fn new(
        ctx: &GraphicsCtx,
        window_size: (u32, u32),
        sprite_registry: SpriteSheetsRegistry,
    ) -> Self {
        let (sprite_pipeline, texture_bind_group_layout) =
            create_sprite_pipeline(&ctx.device, ctx.surface_texture_format);
        let (depth_texture, depth_texture_view, depth_texture_sampler) =
            create_depth_texture(&ctx.device, window_size);
        let (quad_vertex_buf, quad_index_buf) = create_quad_vertex_buf(&ctx.device);
        let sprite_instance_buf = create_sprite_instance_buf(&ctx.device);
        let sprite_staging_belt =
            StagingBelt::new(std::mem::size_of::<SpriteInstance>() as u64 * MAX_SPRITES_PER_BATCH);

        let queue = Vec::with_capacity(MAX_SPRITES_PER_BATCH as usize);

        let atlas = sprite_registry.build_atlas(ctx, &texture_bind_group_layout);

        let proj_matrix = compute_proj_matrix(window_size);

        Self {
            pipeline: sprite_pipeline,
            depth_texture,
            depth_texture_view,
            depth_texture_sampler,
            quad_vertex_buf,
            quad_index_buf,
            sprite_staging_belt,
            sprite_instance_buf,
            proj_matrix,
            queue,
            atlas,
        }
    }

    pub fn draw(&mut self, sprite: Sprite, params: DrawParams) {
        let spritesheet = self.atlas.sheets[sprite.sheet.0];

        self.queue.push(SpriteInstance {
            transform: (self.proj_matrix * params.transform).into(),
            tex_pos: spritesheet.tex_coords(sprite.position).into(),
            tex_dims: spritesheet.tex_dims(sprite.size).into(),
            tint: params.tint.into(),
            z_index: params.depth,
        })
    }

    pub fn resize(&mut self, ctx: &GraphicsCtx, window_size: (u32, u32)) {
        self.proj_matrix = compute_proj_matrix(window_size);
        let (depth_texture, depth_texture_view, depth_texture_sampler) =
            create_depth_texture(&ctx.device, window_size);
        self.depth_texture = depth_texture;
        self.depth_texture_view = depth_texture_view;
        self.depth_texture_sampler = depth_texture_sampler;
    }

    pub fn submit(&mut self, ctx: &GraphicsCtx, frame: &Frame) {
        if self.queue.is_empty() {
            return;
        }

        let mut encoder = ctx
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("Renderer 2D Command encoder"),
            });

        let queue = std::mem::replace(
            &mut self.queue,
            Vec::with_capacity(MAX_SPRITES_PER_BATCH as usize),
        );

        let rawqueue = cast_slice(&queue);

        self.sprite_staging_belt.recall();
        {
            let byte_size = (queue.len() * size_of::<SpriteInstance>()) as u64;
            let mut bufmut = self.sprite_staging_belt.write_buffer(
                &mut encoder,
                &self.sprite_instance_buf,
                0,
                NonZeroU64::new(byte_size).unwrap(),
                &ctx.device,
            );
            bufmut.clone_from_slice(rawqueue);
        }
        self.sprite_staging_belt.finish();

        {
            let mut render_pass: RenderPass<'_> =
                encoder.begin_render_pass(&RenderPassDescriptor {
                    label: Some("Sprite Render Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &frame.view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(Color3::gray(0.01).into()),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.depth_texture_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });

            render_pass.set_pipeline(&self.pipeline);

            render_pass.set_vertex_buffer(0, self.quad_vertex_buf.slice(..));
            render_pass.set_vertex_buffer(1, self.sprite_instance_buf.slice(..));
            render_pass.set_bind_group(0, &self.atlas.bind_group, &[]);
            render_pass.set_index_buffer(self.quad_index_buf.slice(..), IndexFormat::Uint16);
            render_pass.draw_indexed(0..6, 0, 0..queue.len() as u32);
        }

        ctx.queue.submit(std::iter::once(encoder.finish()));
    }
}

fn create_sprite_pipeline(
    device: &Device,
    surface_texture_format: TextureFormat,
) -> (RenderPipeline, BindGroupLayout) {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
    });

    let texture_bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
            label: Some("bind_group_layout"),
        });

    let render_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Render Pipeline Layout"),
        bind_group_layouts: &[&texture_bind_group_layout],
        push_constant_ranges: &[],
    });

    let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("2d_render_pipeline"),
        layout: Some(&render_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs_main",
            buffers: &[
                wgpu::VertexBufferLayout {
                    array_stride: 4 * std::mem::size_of::<f32>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: std::mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
                            shader_location: 1,
                        },
                    ],
                },
                wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<SpriteInstance>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 2,
                            format: wgpu::VertexFormat::Float32x3,
                        },
                        wgpu::VertexAttribute {
                            offset: std::mem::size_of::<[f32; 3]>() as wgpu::BufferAddress,
                            shader_location: 3,
                            format: wgpu::VertexFormat::Float32x3,
                        },
                        wgpu::VertexAttribute {
                            offset: std::mem::size_of::<[f32; 6]>() as wgpu::BufferAddress,
                            shader_location: 4,
                            format: wgpu::VertexFormat::Float32x3,
                        },
                        wgpu::VertexAttribute {
                            offset: std::mem::size_of::<[f32; 9]>() as wgpu::BufferAddress,
                            shader_location: 5,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: std::mem::size_of::<[f32; 11]>() as wgpu::BufferAddress,
                            shader_location: 6,
                            format: wgpu::VertexFormat::Float32x2,
                        },
                        wgpu::VertexAttribute {
                            offset: std::mem::size_of::<[f32; 13]>() as wgpu::BufferAddress,
                            shader_location: 7,
                            format: wgpu::VertexFormat::Float32x3,
                        },
                        wgpu::VertexAttribute {
                            offset: std::mem::size_of::<[f32; 16]>() as wgpu::BufferAddress,
                            shader_location: 8,
                            format: wgpu::VertexFormat::Float32,
                        },
                    ],
                },
            ],
            compilation_options: PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs_main",
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_texture_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: 1,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview: None,
    });

    (render_pipeline, texture_bind_group_layout)
}

fn create_quad_vertex_buf(device: &Device) -> (Buffer, Buffer) {
    #[rustfmt::skip]
    let vertex_data: [f32; 16] = [
//    [ x,    y,    u,    v   ]
        0.0,  0.0,  0.0,  1.0, // bottom left
        1.0,  0.0,  1.0,  1.0, // bottom right
        1.0,  1.0,  1.0,  0.0, // top right
        0.0,  1.0,  0.0,  0.0, // top left
    ];

    let index_data = &[0u16, 1, 2, 0, 2, 3];

    let vertex_buffer = wgpu::util::DeviceExt::create_buffer_init(
        device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("quad_vertex_buffer"),
            contents: unsafe {
                // SAFETY: Safe as long as vertex_data is [f32]
                std::slice::from_raw_parts(
                    vertex_data.as_ptr() as *const u8,
                    vertex_data.len() * std::mem::size_of::<f32>(),
                )
            },
            usage: wgpu::BufferUsages::VERTEX,
        },
    );

    let index_buffer = wgpu::util::DeviceExt::create_buffer_init(
        device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("quad_index_buffer"),
            contents: unsafe {
                // SAFETY: Safe as long as index_data is [u16]
                std::slice::from_raw_parts(
                    index_data.as_ptr() as *const u8,
                    index_data.len() * std::mem::size_of::<u16>(),
                )
            },
            usage: wgpu::BufferUsages::INDEX,
        },
    );

    (vertex_buffer, index_buffer)
}

fn create_sprite_instance_buf(device: &Device) -> Buffer {
    let bufdesc = BufferDescriptor {
        label: Some("Sprite instance buffer"),
        size: MAX_SPRITES_PER_BATCH * MAX_BATCHES * std::mem::size_of::<SpriteInstance>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    };

    device.create_buffer(&bufdesc)
}

pub fn create_depth_texture(
    device: &wgpu::Device,
    (width, height): (u32, u32),
) -> (Texture, TextureView, Sampler) {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let desc = wgpu::TextureDescriptor {
        label: Some("Depth texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    };
    let texture = device.create_texture(&desc);

    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Nearest,
        compare: Some(wgpu::CompareFunction::LessEqual),
        lod_min_clamp: 0.0,
        lod_max_clamp: 100.0,
        ..Default::default()
    });

    (texture, view, sampler)
}

fn compute_proj_matrix((w, h): (u32, u32)) -> Matrix3<f32> {
    let (w, h) = (w as f32, h as f32);
    let (x, y) = if w < h { (1.0, w / h) } else { (h / w, 1.0) };
    Matrix3::from_nonuniform_scale(x, y)
}

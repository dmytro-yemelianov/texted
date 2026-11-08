//! Instanced rectangles and cached glyph masks, rendered by Metal through wgpu.
use crate::graphics::{CachedGlyph, DrawList, GlyphKey};
use std::{
    collections::HashMap,
    ops::Range,
    sync::{Arc, Mutex},
};
use winit::window::Window;

const ATLAS_SIZE: u32 = 1024;
const MAX_ATLAS_PAGES: usize = 8; // At most 8 MiB of GPU glyph coverage.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    rect: [f32; 4],
    uv: [f32; 4],
    color: [f32; 4],
}
#[derive(Clone, Copy)]
struct Region {
    page: usize,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}
#[derive(Clone, Copy)]
struct Shelf {
    x: u32,
    y: u32,
    height: u32,
}
impl Default for Shelf {
    fn default() -> Self {
        Self {
            x: 2,
            y: 0,
            height: 2,
        }
    }
}
impl Shelf {
    fn allocate(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        if width == 0 || height == 0 || width >= ATLAS_SIZE || height >= ATLAS_SIZE {
            return None;
        }
        let (mut x, mut y, mut row_height) = (self.x, self.y, self.height);
        if x + width + 1 > ATLAS_SIZE {
            x = 0;
            y += row_height;
            row_height = 0;
        }
        if y + height + 1 > ATLAS_SIZE {
            return None;
        }
        self.x = x + width + 1;
        self.y = y;
        self.height = row_height.max(height + 1);
        Some((x, y))
    }
}
struct AtlasPage {
    texture: wgpu::Texture,
    bind: wgpu::BindGroup,
    shelf: Shelf,
    used: u64,
}
struct Batch {
    page: usize,
    instances: Range<u32>,
}
struct Painter {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    viewport: wgpu::Buffer,
    viewport_bind: wgpu::BindGroup,
    atlas_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pages: Vec<AtlasPage>,
    glyphs: HashMap<GlyphKey, Region>,
    instances: Vec<Instance>,
    batches: Vec<Batch>,
    instance_buffer: wgpu::Buffer,
    instance_capacity: u64,
    frame: u64,
    srgb: bool,
}
impl Painter {
    fn new(device: wgpu::Device, queue: wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let viewport = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewport"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let viewport_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewport layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let viewport_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewport"),
            layout: &viewport_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: viewport.as_entire_binding(),
            }],
        });
        let atlas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glyph atlas layout"),
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
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("text pipeline layout"),
            bind_group_layouts: &[&viewport_layout, &atlas_layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("text and rectangles"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gpu.wgsl").into()),
        });
        let attributes = wgpu::vertex_attr_array![0=>Float32x4,1=>Float32x4,2=>Float32x4];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("text and rectangles"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attributes,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pixel-aligned glyph masks"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let instance_capacity = 4096;
        let instance_buffer = Self::buffer(&device, instance_capacity);
        Self {
            device,
            queue,
            pipeline,
            viewport,
            viewport_bind,
            atlas_layout,
            sampler,
            pages: Vec::new(),
            glyphs: HashMap::new(),
            instances: Vec::new(),
            batches: Vec::new(),
            instance_buffer,
            instance_capacity,
            frame: 0,
            srgb: format.is_srgb(),
        }
    }
    fn buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad instances"),
            size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
    fn add_page(&mut self) {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(1),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glyph atlas"),
            layout: &self.atlas_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.pages.push(AtlasPage {
            texture,
            bind,
            shelf: Shelf::default(),
            used: 0,
        });
    }
    fn region(&mut self, glyph: &CachedGlyph) -> Result<Region, String> {
        if let Some(&r) = self.glyphs.get(&glyph.key) {
            self.pages[r.page].used = self.frame;
            return Ok(r);
        }
        if glyph.width >= ATLAS_SIZE || glyph.height >= ATLAS_SIZE {
            return Err("A glyph exceeds the GPU atlas dimensions".into());
        }
        let mut allocated = self.pages.iter_mut().enumerate().find_map(|(page, p)| {
            p.shelf
                .allocate(glyph.width, glyph.height)
                .map(|(x, y)| (page, x, y))
        });
        if allocated.is_none() {
            let page = if self.pages.len() < MAX_ATLAS_PAGES {
                self.add_page();
                self.pages.len() - 1
            } else {
                // Never invalidate coordinates already emitted in this frame.
                let page = self
                    .pages
                    .iter()
                    .position(|p| p.used != self.frame)
                    .ok_or("Visible glyphs exceed the GPU atlas budget")?;
                self.glyphs.retain(|_, r| r.page != page);
                self.pages[page].shelf = Shelf::default();
                page
            };
            allocated = self.pages[page]
                .shelf
                .allocate(glyph.width, glyph.height)
                .map(|(x, y)| (page, x, y));
        }
        let (page, x, y) = allocated.ok_or("Could not allocate GPU glyph")?;
        let p = &mut self.pages[page];
        p.used = self.frame;
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &p.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            &glyph.coverage,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(glyph.width),
                rows_per_image: Some(glyph.height),
            },
            wgpu::Extent3d {
                width: glyph.width,
                height: glyph.height,
                depth_or_array_layers: 1,
            },
        );
        let r = Region {
            page,
            x,
            y,
            width: glyph.width,
            height: glyph.height,
        };
        self.glyphs.insert(glyph.key, r);
        Ok(r)
    }
    fn prepare(&mut self, list: &DrawList, width: u32, height: u32) -> Result<(), String> {
        self.frame = self.frame.wrapping_add(1);
        self.instances.clear();
        self.batches.clear();
        if self.pages.is_empty() {
            self.add_page();
        }
        for draw in &list.draws {
            let (page, uv) = if let Some(glyph) = &draw.glyph {
                let r = self.region(glyph)?;
                let [u0, v0, u1, v1] = draw.crop;
                (
                    r.page,
                    [
                        (r.x as f32 + u0 * r.width as f32) / ATLAS_SIZE as f32,
                        (r.y as f32 + v0 * r.height as f32) / ATLAS_SIZE as f32,
                        (r.x as f32 + u1 * r.width as f32) / ATLAS_SIZE as f32,
                        (r.y as f32 + v1 * r.height as f32) / ATLAS_SIZE as f32,
                    ],
                )
            } else {
                self.pages[0].used = self.frame;
                (0, [0.5 / ATLAS_SIZE as f32; 4])
            };
            let index = self.instances.len() as u32;
            self.instances.push(Instance {
                rect: draw.rect,
                uv,
                color: color(draw.color, self.srgb),
            });
            if let Some(batch) = self.batches.last_mut().filter(|b| b.page == page) {
                batch.instances.end = index + 1;
            } else {
                self.batches.push(Batch {
                    page,
                    instances: index..index + 1,
                });
            }
        }
        let bytes = bytemuck::cast_slice(&self.instances);
        if bytes.len() as u64 > self.instance_capacity {
            self.instance_capacity = (bytes.len() as u64).next_power_of_two();
            self.instance_buffer = Self::buffer(&self.device, self.instance_capacity);
        }
        if !bytes.is_empty() {
            self.queue.write_buffer(&self.instance_buffer, 0, bytes);
        }
        self.queue.write_buffer(
            &self.viewport,
            0,
            bytemuck::cast_slice(&[width as f32, height as f32, 0.0, 0.0]),
        );
        Ok(())
    }
    fn encode(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, clear: u32) {
        let c = color(clear, self.srgb);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Texted frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: c[0] as f64,
                        g: c[1] as f64,
                        b: c[2] as f64,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.viewport_bind, &[]);
        pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
        for batch in &self.batches {
            pass.set_bind_group(1, &self.pages[batch.page].bind, &[]);
            pass.draw(0..6, batch.instances.clone());
        }
    }
}
fn color(rgb: u32, srgb: bool) -> [f32; 4] {
    let channel = |shift: u32| {
        let v = ((rgb >> shift) & 255u32) as f32 / 255.0;
        if !srgb {
            v
        } else if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    [channel(16), channel(8), channel(0), 1.0]
}
fn instance() -> wgpu::Instance {
    wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::METAL,
        ..Default::default()
    })
}
async fn device(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue), String> {
    adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("Texted GPU"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        })
        .await
        .map_err(|e| format!("Could not initialize GPU: {e}"))
}
pub(crate) struct Renderer {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    painter: Painter,
    presenter: crate::presentation::PixelPresenter,
    failure: Arc<Mutex<Option<String>>>,
    pub description: String,
}
impl Renderer {
    pub async fn new(window: Arc<Window>) -> Result<Self, String> {
        let instance = instance();
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("Could not create GPU surface: {e}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| format!("No Metal GPU is available: {e}"))?;
        let info = adapter.get_info();
        let description = format!("{:?}: {}", info.backend, info.name);
        let (device, queue) = device(&adapter).await?;
        let failure = Arc::new(Mutex::new(None));
        let mark = failure.clone();
        device.set_device_lost_callback(move |reason, message| {
            if let Ok(mut failure) = mark.lock() {
                *failure = Some(format!("GPU device lost ({reason:?}): {message}"));
            }
        });
        let mark = failure.clone();
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            if let Ok(mut failure) = mark.lock() {
                *failure = Some(error.to_string());
            }
        }));
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("No compatible GPU surface configuration")?;
        if let Some(format) = surface
            .get_capabilities(&adapter)
            .formats
            .into_iter()
            .find(|f| !f.is_srgb())
        {
            config.format = format;
        }
        config.present_mode = wgpu::PresentMode::Fifo;
        config.desired_maximum_frame_latency = 2;
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let painter = Painter::new(device.clone(), queue, config.format);
        let presenter = crate::presentation::PixelPresenter::new(&device, config.format);
        if let Some(error) = device.pop_error_scope().await {
            return Err(format!("GPU pipeline validation failed: {error}"));
        }
        surface.configure(&device, &config);
        eprintln!("Texted renderer: {description}");
        Ok(Self {
            surface,
            config,
            painter,
            presenter,
            failure,
            description,
        })
    }
    pub fn render(
        &mut self,
        list: &DrawList,
        width: u32,
        height: u32,
        mapping: texted_app::framebuffer::ScreenMapping,
    ) -> Result<bool, String> {
        if width == 0 || height == 0 {
            return Ok(false);
        }
        if let Some(error) = self
            .failure
            .lock()
            .map_err(|_| "GPU error state unavailable")?
            .as_ref()
        {
            return Err(error.clone());
        }
        if width > self.painter.device.limits().max_texture_dimension_2d
            || height > self.painter.device.limits().max_texture_dimension_2d
        {
            return Err("Window exceeds the GPU texture dimensions".into());
        }
        if self.config.width != width || self.config.height != height {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.painter.device, &self.config);
        }
        let output = match self.surface.get_current_texture() {
            Ok(output) => output,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.painter.device, &self.config);
                return Ok(false);
            }
            Err(wgpu::SurfaceError::Timeout | wgpu::SurfaceError::Other) => return Ok(false),
            Err(wgpu::SurfaceError::OutOfMemory) => return Err("GPU memory exhausted".into()),
        };
        self.painter
            .prepare(list, mapping.source.0, mapping.source.1)?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            self.painter
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Texted frame"),
                });
        if mapping.pixelated {
            self.presenter
                .set_crt(&self.painter.queue, mapping.crt.params());
            let source = self.presenter.source(&self.painter.device, mapping.source);
            self.painter.encode(&mut encoder, source, list.clear);
            self.presenter.encode(&mut encoder, &view, mapping);
        } else {
            self.presenter.release();
            self.painter.encode(&mut encoder, &view, list.clear);
        }
        self.painter.queue.submit([encoder.finish()]);
        output.present();
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shader_validates_without_a_gpu() {
        let module = wgpu::naga::front::wgsl::parse_str(include_str!("gpu.wgsl")).unwrap();
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }
    #[test]
    fn shelf_failure_does_not_consume_space_and_leaves_white_texel_reserved() {
        let mut shelf = Shelf::default();
        assert_eq!(shelf.allocate(10, 20), Some((2, 0)));
        let before = shelf;
        assert!(shelf.allocate(ATLAS_SIZE, 5).is_none());
        assert_eq!(
            (shelf.x, shelf.y, shelf.height),
            (before.x, before.y, before.height)
        );
        assert_eq!(shelf.allocate(ATLAS_SIZE - 1, 4), Some((0, 21)));
    }
    #[test]
    #[ignore = "Requires a real Metal GPU; run explicitly on a Mac"]
    fn metal_crt_curves_the_picture_and_darkens_its_corners() {
        pollster::block_on(async {
            use texted_app::framebuffer::{CrtEffect, ScreenMapping};
            let adapter = instance()
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .unwrap();
            let (device, queue) = device(&adapter).await.unwrap();
            device.push_error_scope(wgpu::ErrorFilter::Validation);
            let format = wgpu::TextureFormat::Rgba8Unorm;
            let mut painter = Painter::new(device.clone(), queue, format);
            let mut presenter = crate::presentation::PixelPresenter::new(&device, format);
            let mut list = DrawList::default();
            list.reset(0);
            crate::graphics::draw_rect(&mut list, 8, 8, 0, 0, 8, 8, 0xc0c0c0);
            painter.prepare(&list, 8, 8).unwrap();
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("crt readback"),
                size: wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 64 * 64 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mapping = ScreenMapping::new((64, 64), Some((8, 8))).with_crt(CrtEffect::Strong);
            presenter.set_crt(&painter.queue, mapping.crt.params());
            let mut encoder = device.create_command_encoder(&Default::default());
            let source = presenter.source(&device, (8, 8));
            painter.encode(&mut encoder, source, list.clear);
            presenter.encode(
                &mut encoder,
                &target.create_view(&Default::default()),
                mapping,
            );
            encoder.copy_texture_to_buffer(
                target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(64),
                    },
                },
                wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
            );
            painter.queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            let bytes = readback.slice(..).get_mapped_range();
            let pixel = |x: usize, y: usize| bytes[(y * 64 + x) * 4];
            assert_eq!(pixel(0, 0), 0, "the corner is black glass");
            assert!(pixel(32, 32) > 0x80, "the centre keeps the picture");
            assert!(
                pixel(4, 32) < pixel(32, 32),
                "the edges are darker than the centre"
            );
            drop(bytes);
            presenter.release();
            assert!(device.pop_error_scope().await.is_none());
        });
    }
    #[test]
    #[ignore = "Requires a real Metal GPU; run explicitly on a Mac"]
    fn metal_framebuffer_preserves_exact_texels_and_letterboxing() {
        pollster::block_on(async {
            use texted_app::framebuffer::ScreenMapping;
            let adapter = instance()
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .unwrap();
            assert_ne!(adapter.get_info().device_type, wgpu::DeviceType::Cpu);
            let (device, queue) = device(&adapter).await.unwrap();
            device.push_error_scope(wgpu::ErrorFilter::Validation);
            let format = wgpu::TextureFormat::Rgba8Unorm;
            let mut painter = Painter::new(device.clone(), queue, format);
            let mut presenter = crate::presentation::PixelPresenter::new(&device, format);
            let mut list = DrawList::default();
            list.reset(0);
            for y in 0..3 {
                for x in 0..3 {
                    let color = (((20 + 80 * x) as u32) << 16) | (((20 + 80 * y) as u32) << 8) | 70;
                    crate::graphics::draw_rect(&mut list, 3, 3, x, y, 1, 1, color);
                }
            }
            painter.prepare(&list, 3, 3).unwrap();
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("presentation readback"),
                size: wgpu::Extent3d {
                    width: 64,
                    height: 32,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 64 * 32 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            let source = presenter.source(&device, (3, 3));
            painter.encode(&mut encoder, source, list.clear);
            presenter.encode(
                &mut encoder,
                &target.create_view(&Default::default()),
                ScreenMapping::new((64, 32), Some((3, 3))),
            );
            encoder.copy_texture_to_buffer(
                target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(32),
                    },
                },
                wgpu::Extent3d {
                    width: 64,
                    height: 32,
                    depth_or_array_layers: 1,
                },
            );
            painter.queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            let bytes = readback.slice(..).get_mapped_range();
            for y in 0..32 {
                for x in 0..64 {
                    let expected = if !(16..48).contains(&x) {
                        [0, 0, 0, 255]
                    } else {
                        // Pixel centers map to exact texels even at the non-integer 32/3 scale.
                        let sx = ((x - 16) * 2 + 1) * 3 / 64;
                        let sy = (y * 2 + 1) * 3 / 64;
                        [(20 + 80 * sx) as u8, (20 + 80 * sy) as u8, 70, 255]
                    };
                    let at = (y * 64 + x) * 4;
                    assert_eq!(
                        &bytes[at..at + 4],
                        &expected,
                        "exact source texel at ({x},{y})"
                    );
                }
            }
            // Replacing the cached target and returning to Native are valid resource transitions.
            presenter.source(&device, (256, 192));
            presenter.source(&device, (320, 200));
            presenter.release();
            assert!(device.pop_error_scope().await.is_none());
        });
    }
    #[test]
    #[ignore = "Requires a real Metal GPU; run explicitly on a Mac"]
    fn metal_renders_clipping_binary_masks_and_paint_order() {
        pollster::block_on(async {
            let adapter = instance()
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("Metal adapter");
            assert_ne!(adapter.get_info().device_type, wgpu::DeviceType::Cpu);
            let (device, queue) = device(&adapter).await.unwrap();
            device.push_error_scope(wgpu::ErrorFilter::Validation);
            let mut painter = Painter::new(device.clone(), queue, wgpu::TextureFormat::Rgba8Unorm);
            let mut list = DrawList::default();
            list.reset(0);
            crate::graphics::draw_rect(&mut list, 64, 32, 0, 0, 64, 32, 0xff0000);
            crate::graphics::draw_rect(&mut list, 64, 32, 8, 8, 8, 8, 0x00ff00);
            let mut font = crate::graphics::FontEngine::new(18.0);
            font.draw_string(&mut list, 64, 12, 20, 0, "їA", 0xffffff);
            painter.prepare(&list, 64, 32).unwrap();
            let warm_entries = painter.glyphs.len();
            let warm_pages = painter.pages.len();
            painter.prepare(&list, 64, 32).unwrap();
            assert_eq!(painter.glyphs.len(), warm_entries);
            assert_eq!(painter.pages.len(), warm_pages);
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 64,
                    height: 32,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 64 * 32 * 4,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            painter.encode(
                &mut encoder,
                &target.create_view(&wgpu::TextureViewDescriptor::default()),
                list.clear,
            );
            encoder.copy_texture_to_buffer(
                target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(32),
                    },
                },
                wgpu::Extent3d {
                    width: 64,
                    height: 32,
                    depth_or_array_layers: 1,
                },
            );
            painter.queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            let pixels = readback.slice(..).get_mapped_range();
            assert_eq!(&pixels[0..4], &[255, 0, 0, 255]);
            let at = (10 * 64 + 10) * 4;
            assert_eq!(&pixels[at..at + 4], &[0, 255, 0, 255]);
            for y in 12..32 {
                for x in 20..64 {
                    let at = (y * 64 + x) * 4;
                    assert_eq!(
                        &pixels[at..at + 4],
                        &[255, 0, 0, 255],
                        "glyph clipping at ({x},{y})"
                    );
                }
            }
            assert!(
                pixels.chunks_exact(4).any(|p| p == [255, 255, 255, 255]),
                "native bitmap glyphs render opaque white pixels"
            );
            assert!(
                pixels.chunks_exact(4).all(|p| p == [255, 0, 0, 255]
                    || p == [0, 255, 0, 255]
                    || p == [255, 255, 255, 255]),
                "binary masks must never introduce antialiased edge colors"
            );
            // Simulate a full cache: a live frame must never have its atlas coordinates evicted.
            let retained_key = list
                .draws
                .iter()
                .find_map(|d| d.glyph.as_ref())
                .unwrap()
                .key;
            while painter.pages.len() < MAX_ATLAS_PAGES {
                painter.add_page();
            }
            for page in &mut painter.pages {
                page.shelf = Shelf {
                    x: 0,
                    y: ATLAS_SIZE,
                    height: 0,
                };
                page.used = painter.frame;
            }
            let mut next = DrawList::default();
            crate::graphics::FontEngine::new(20.0)
                .draw_char(&mut next, 64, 32, 0, 0, 'B', 0xffffff);
            let next_glyph = next.draws[0].glyph.as_ref().unwrap();
            assert!(painter.region(next_glyph).is_err());
            assert!(painter.glyphs.contains_key(&retained_key));
            painter.frame += 1;
            assert_eq!(painter.region(next_glyph).unwrap().page, 0);
            assert!(!painter.glyphs.contains_key(&retained_key));
            assert!(painter.glyphs.contains_key(&next_glyph.key));
            assert_eq!(painter.pages.len(), MAX_ATLAS_PAGES);
            assert!(device.pop_error_scope().await.is_none());
            eprintln!("Hardware GPU validation passed: {:?}", adapter.get_info());
        });
    }
}

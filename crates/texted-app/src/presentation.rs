//! A cached fixed-size GPU target and a nearest-texel presentation pass. No CPU framebuffer copies.
use texted_app::framebuffer::ScreenMapping;
use texted_app::visuals::MatrixBackground;
/// Seven vec4s of effect parameters, in presentation.wgsl's `Effects` order.
const UNIFORM_BYTES: u64 = 112;
/// The effect pass parameters for a mapping, as presentation.wgsl reads them.
fn uniforms(mapping: &ScreenMapping) -> [f32; 28] {
    let e = mapping.effects;
    let gap = match e.matrix_background {
        MatrixBackground::Black => [0.0; 3],
        MatrixBackground::Theme => {
            let c = mapping.background;
            [(c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff].map(|v| v as f32 / 255.0)
        }
    };
    [
        e.brightness,
        e.curvature,
        e.chromatic_aberration,
        e.afterglow,
        e.scanlines,
        e.pixel_grid,
        e.jitter,
        e.antenna_hum,
        e.noise,
        e.flicker,
        e.vignette,
        e.bloom,
        e.separation.index() as f32,
        e.separation_strength,
        e.matrix_spacing,
        e.inner_square,
        gap[0],
        gap[1],
        gap[2],
        f32::from(mapping.phosphor.index()),
        mapping.effect_time,
        mapping.effect_pixel,
        f32::from(u8::from(!e.is_neutral())),
        f32::from(u8::from(e.animated())),
        mapping.destination.2 as f32,
        mapping.destination.3 as f32,
        0.0,
        0.0,
    ]
}
pub(crate) struct PixelPresenter {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    /// The effect pass parameters; new buffers start zeroed, which draws flat.
    effects: wgpu::Buffer,
    format: wgpu::TextureFormat,
    target: Option<Target>,
}
struct Target {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
    size: (u32, u32),
}
impl PixelPresenter {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fixed framebuffer binding"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
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
        let effects = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screen effects"),
            size: UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nearest framebuffer presentation"),
            source: wgpu::ShaderSource::Wgsl(include_str!("presentation.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("framebuffer presentation"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nearest framebuffer presentation"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        Self {
            pipeline,
            layout,
            effects,
            format,
            target: None,
        }
    }
    pub fn source(&mut self, device: &wgpu::Device, size: (u32, u32)) -> &wgpu::TextureView {
        if self.target.as_ref().is_none_or(|t| t.size != size) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("limited resolution framebuffer"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fixed framebuffer"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.effects.as_entire_binding(),
                    },
                ],
            });
            self.target = Some(Target {
                _texture: texture,
                view,
                bind,
                size,
            });
        }
        &self.target.as_ref().unwrap().view
    }
    /// Effect amounts, time and pixel size for the next presentation, in presentation.wgsl order.
    pub fn set_effects(&self, queue: &wgpu::Queue, mapping: &ScreenMapping) {
        queue.write_buffer(&self.effects, 0, bytemuck::cast_slice(&uniforms(mapping)));
    }
    pub fn release(&mut self) {
        self.target = None;
    }
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        mapping: ScreenMapping,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("aspect-fit pixel presentation"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        let (x, y, w, h) = mapping.destination;
        pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(
            0,
            &self
                .target
                .as_ref()
                .expect("source prepared before presentation")
                .bind,
            &[],
        );
        pass.draw(0..3, 0..1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use texted_app::visuals::{MatrixBackground, Phosphor, Separation, VisualEffects};
    #[test]
    fn effect_uniforms_follow_the_shader_layout() {
        let effects = VisualEffects {
            brightness: 0.9,
            curvature: 0.2,
            chromatic_aberration: 0.3,
            afterglow: 0.4,
            scanlines: 0.5,
            pixel_grid: 0.6,
            jitter: 0.7,
            antenna_hum: 0.8,
            noise: 0.1,
            flicker: 1.5,
            vignette: 2.5,
            bloom: 1.25,
            separation: Separation::GameboyMatrix,
            separation_strength: 0.35,
            matrix_spacing: 0.2,
            inner_square: 0.0,
            matrix_background: MatrixBackground::Theme,
        };
        let mapping = ScreenMapping::new((800, 600), Some((160, 144)))
            .with_effects(effects, 12.5, 5.0)
            .with_glass(Phosphor::White, 0x336699);
        let u = uniforms(&mapping);
        assert_eq!(std::mem::size_of_val(&u), UNIFORM_BYTES as usize);
        assert_eq!(u[..4], [0.9, 0.2, 0.3, 0.4]);
        assert_eq!(u[4..8], [0.5, 0.6, 0.7, 0.8]);
        assert_eq!(u[8..12], [0.1, 1.5, 2.5, 1.25]);
        assert_eq!(u[12..16], [6.0, 0.35, 0.2, 0.0]);
        assert_eq!(
            u[16..20],
            [
                0x33 as f32 / 255.0,
                0x66 as f32 / 255.0,
                0x99 as f32 / 255.0,
                3.0
            ]
        );
        assert_eq!(u[20..24], [12.5, 5.0, 1.0, 1.0]);
        let (_, _, w, h) = mapping.destination;
        assert_eq!(u[24..28], [w as f32, h as f32, 0.0, 0.0]);
        // ditherGL's black gap ignores the theme; neutral effects take the exact path.
        let black = ScreenMapping {
            effects: VisualEffects {
                matrix_background: MatrixBackground::Black,
                ..effects
            },
            ..mapping
        };
        assert_eq!(uniforms(&black)[16..19], [0.0, 0.0, 0.0]);
        let flat = ScreenMapping::new((800, 600), None);
        assert_eq!(uniforms(&flat)[22..24], [0.0, 0.0]);
    }
    #[test]
    fn presentation_shader_validates_without_a_gpu() {
        let m = wgpu::naga::front::wgsl::parse_str(include_str!("presentation.wgsl")).unwrap();
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::empty(),
        )
        .validate(&m)
        .unwrap();
    }
}

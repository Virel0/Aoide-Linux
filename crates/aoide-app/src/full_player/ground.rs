//! The moving ground: `ground.wgsl` drawn over the whole player through
//! iced's shader widget, from the cover's four main colours and a clock that
//! runs only while the music plays. Without the GPU renderer nothing is
//! drawn, and the still blurred cover under it shows instead.

use iced::widget::shader::{self, Viewport};
use iced::{Rectangle, mouse, wgpu};

use super::Ground;

/// The uniform block's floats: four colours (RGBA), then time, aspect,
/// mode and whether to write sRGB.
const FLOATS: usize = 20;

/// What one frame of the ground needs.
#[derive(Debug, Clone, Copy)]
pub struct Program {
    pub ground: Ground,
    /// The cover's main colours, as linear light, most of the cover first.
    pub palette: [[f32; 3]; 4],
    /// Seconds the ground has moved.
    pub clock: f32,
}

impl<Message> shader::Program<Message> for Program {
    type State = ();
    type Primitive = Primitive;

    fn draw(&self, _state: &(), _cursor: mouse::Cursor, bounds: Rectangle) -> Primitive {
        Primitive {
            floats: floats(self, bounds),
        }
    }
}

/// The uniforms for `program` in `bounds`, the sRGB flag left for the
/// pipeline to set.
fn floats(program: &Program, bounds: Rectangle) -> [f32; FLOATS] {
    let mut floats = [0.0; FLOATS];
    for (slot, [r, g, b]) in floats
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(program.palette)
    {
        slot.copy_from_slice(&[r, g, b, 1.0]);
    }
    floats[16] = program.clock;
    floats[17] = bounds.width / bounds.height.max(1.0);
    floats[18] = match program.ground {
        Ground::Still | Ground::Silk => 1.0,
        Ground::Aurora => 2.0,
        Ground::Ink => 3.0,
    };
    floats
}

#[derive(Debug)]
pub struct Primitive {
    floats: [f32; FLOATS],
}

impl shader::Primitive for Primitive {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Pipeline,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _bounds: &Rectangle,
        _viewport: &Viewport,
    ) {
        let mut floats = self.floats;
        floats[19] = if pipeline.encode_srgb { 1.0 } else { 0.0 };
        let bytes: Vec<u8> = floats.iter().flat_map(|f| f.to_ne_bytes()).collect();
        queue.write_buffer(&pipeline.uniforms, 0, &bytes);
    }

    fn draw(&self, pipeline: &Pipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        pass.set_pipeline(&pipeline.pipeline);
        pass.set_bind_group(0, &pipeline.bind_group, &[]);
        pass.draw(0..3, 0..1);
        true
    }
}

/// The ground's shader, its uniforms, and whether the screen's format
/// leaves sRGB to the shader.
pub struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    encode_srgb: bool,
}

impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let label = Some("aoide ground");
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label,
            source: wgpu::ShaderSource::Wgsl(SOURCE.into()),
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label,
            size: (FLOATS * 4) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label,
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label,
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label,
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview: None,
            cache: None,
        });
        Self {
            pipeline,
            uniforms,
            bind_group,
            encode_srgb: !format.is_srgb(),
        }
    }
}

const SOURCE: &str = include_str!("ground.wgsl");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shader_is_valid_wgsl() {
        let module = naga::front::wgsl::parse_str(SOURCE).expect("the shader parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("the shader validates");
        for entry in ["vs_main", "fs_main"] {
            assert!(module.entry_points.iter().any(|e| e.name == entry));
        }
    }

    #[test]
    fn the_uniforms_carry_the_colours_the_clock_and_the_shape() {
        let program = Program {
            ground: Ground::Aurora,
            palette: [[0.1, 0.2, 0.3], [0.4, 0.5, 0.6], [0.7, 0.8, 0.9], [0.0; 3]],
            clock: 12.5,
        };
        let floats = floats(
            &program,
            Rectangle::new(iced::Point::ORIGIN, iced::Size::new(1600.0, 900.0)),
        );
        assert_eq!(&floats[0..4], &[0.1, 0.2, 0.3, 1.0]);
        assert_eq!(&floats[8..12], &[0.7, 0.8, 0.9, 1.0]);
        assert_eq!(floats[16], 12.5);
        assert!((floats[17] - 16.0 / 9.0).abs() < 1e-6);
        assert_eq!(floats[18], 2.0);
    }
}

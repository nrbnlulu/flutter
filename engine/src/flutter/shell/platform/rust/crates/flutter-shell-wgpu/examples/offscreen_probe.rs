//! Display-free wgpu baseline, independent of the Flutter engine.
use flutter_shell_wgpu::{GpuContext, OffscreenReadback};
use std::{io::Write, path::PathBuf, time::Duration};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("usage: offscreen_probe ARTIFACT_DIR")?,
    );
    std::fs::create_dir_all(&output)?;
    // A failed rerun must never retain an earlier passing result.
    match std::fs::remove_file(output.join("PASS")) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return Err("unset DISPLAY and WAYLAND_DISPLAY for this probe".into());
    }
    let context = GpuContext::headless()?;
    let fingerprint = format!(
        "wgpu-only baseline; NOT a Flutter/Impeller capability result\n{:#?}\nformat=Rgba8Unorm; linear; top-to-bottom; RGBA bytes; alpha preserved\n",
        context.adapter_info()
    );
    eprint!("{fingerprint}");
    std::fs::write(output.join("adapter.txt"), fingerprint)?;
    let device = context.device();
    let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("offscreen channel/alpha/orientation pattern"),
        source: wgpu::ShaderSource::Wgsl(
            r#"
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let p = array(vec2f(-1., -1.), vec2f(3., -1.), vec2f(-1., 3.));
    return vec4f(p[i], 0., 1.);
}
@fragment fn fs(@builtin(position) p: vec4f) -> @location(0) vec4f {
    if p.y < 32. {
        if p.x < 64. { return vec4f(1., 0., 0., 1.); }
        return vec4f(0., 1., 0., 1.);
    }
    if p.x < 64. { return vec4f(0., 0., 1., 1.); }
    return vec4f(1., 1., 1., 128. / 255.);
}
"#
            .into(),
        ),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("offscreen probe"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });
    for (generation, (width, height)) in [(257, 129), (131, 67), (257, 129)].into_iter().enumerate()
    {
        let mut target = OffscreenReadback::new(context.clone(), width, height)?;
        let view = target.texture().create_view(&Default::default());
        for frame in 0..4 {
            let marker = if frame % 2 == 0 {
                [255, 0, 255, 255]
            } else {
                [0, 255, 255, 255]
            };
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("offscreen probe"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: marker[0] as f64 / 255.,
                                g: marker[1] as f64 / 255.,
                                b: 1.,
                                a: 1.,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                // An alternating right column detects stale readback on reuse.
                pass.set_pipeline(&pipeline);
                pass.set_scissor_rect(0, 0, width - 1, height);
                pass.draw(0..3, 0..1);
            }
            context.shared_queue().submit([encoder.finish()]);
            let rgba = target.read_rgba(Duration::from_secs(10))?;
            for y in 0..height {
                for x in 0..width {
                    let expected = if x == width - 1 {
                        marker
                    } else if y < 32 {
                        if x < 64 {
                            [255, 0, 0, 255]
                        } else {
                            [0, 255, 0, 255]
                        }
                    } else if x < 64 {
                        [0, 0, 255, 255]
                    } else {
                        [255, 255, 255, 128]
                    };
                    let offset = ((y * width + x) * 4) as usize;
                    if rgba[offset..offset + 4] != expected {
                        return Err(format!("generation {generation} frame {frame} pixel ({x},{y}): {:?}, expected {expected:?}", &rgba[offset..offset+4]).into());
                    }
                }
            }
            if frame == 3 {
                std::fs::write(
                    output.join(format!("generation-{generation}-{width}x{height}.rgba")),
                    &rgba,
                )?;
                let mut ppm =
                    std::fs::File::create(output.join(format!("generation-{generation}.ppm")))?;
                write!(ppm, "P6\n{width} {height}\n255\n")?;
                for pixel in rgba.chunks_exact(4) {
                    ppm.write_all(&pixel[..3])?;
                }
            }
        }
    }
    if let Some(error) = pollster::block_on(errors.pop()) {
        return Err(format!("wgpu validation failed: {error}").into());
    }
    std::fs::write(
        output.join("PASS"),
        "12 exact-pixel wgpu captures passed; Flutter/Impeller gate remains pending\n",
    )?;
    eprintln!("PASS: 12 captures, odd widths, alpha, orientation, reuse and target recreation");
    Ok(())
}

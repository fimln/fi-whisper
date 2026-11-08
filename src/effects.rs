use crate::{App, Phase};
use rune::compose::{Compositor, DeviceKey, Frame};
use rune_fx::{
    Fx, FxFrame, FxLayer, FxOptions, FxRenderer, GlowParams, GlowStyle, PlasticParams, PulseParams,
    PulseStyle, Shape, ShimmerParams, rgb,
};
#[derive(Default)]
pub struct Glow {
    gpu: Option<(DeviceKey, wgpu::TextureFormat, FxRenderer)>,
    pending: Option<(
        DeviceKey,
        wgpu::TextureFormat,
        std::sync::mpsc::Receiver<FxRenderer>,
    )>,
    animating: bool,
}
impl Compositor<App> for Glow {
    fn animating(&self) -> bool {
        self.animating
    }
    fn render(&mut self, f: &mut Frame<'_, App>) {
        let phase = f.rt.state.core.status().phase;
        let active = matches!(
            phase,
            Phase::Recording | Phase::Loading | Phase::Transcribing
        );
        let reduced = f.rt.reduced_motion;
        let now = if reduced { 0.0 } else { f.rt.t as f32 };
        let flash = (1.0 - (f.rt.state.now - f.rt.state.result_flash) as f32 / 0.8).clamp(0.0, 1.0);
        let copy = (1.0 - (f.rt.state.now - f.rt.state.copy_flash) as f32 / 0.6).clamp(0.0, 1.0);
        let pointer = f.rt.pointer();
        let mut layers: Vec<FxLayer> = Vec::new();
        for key in [
            "dictation",
            "result",
            "settings",
            "model",
            "dictate",
            "meter",
            "copy",
        ] {
            let Some((r, a)) = f.rt.presented(key) else {
                continue;
            };
            let hover = pointer.is_some_and(|(x, y)| {
                x >= r[0] && y >= r[1] && x <= r[0] + r[2] && y <= r[1] + r[3]
            });
            let button = key == "dictate";
            let meter = key == "meter";
            let strength = if key == "copy" {
                copy
            } else if key == "result" {
                0.12 + flash * 0.65
            } else if meter {
                if phase == Phase::Recording {
                    f.rt.state.level * 0.7
                } else {
                    0.0
                }
            } else if button {
                if active {
                    0.6
                } else if hover {
                    0.5
                } else {
                    0.18
                }
            } else if hover {
                0.24
            } else {
                0.09
            };
            if strength <= 0.001 {
                continue;
            }
            let shape = Shape::rounded(
                r[0],
                r[1],
                r[2],
                r[3],
                if button || meter { 6.0 } else { 12.0 },
            );
            if !button && !meter && key != "copy" {
                layers.push(
                    Fx::fill(
                        PlasticParams {
                            color: rgb(0x78502a),
                            roughness: 0.6,
                            dome: 0.1,
                            bevel: 2.0,
                            ..Default::default()
                        }
                        .border(1.0),
                    )
                    .on(shape)
                    .over()
                    .at(now)
                    .opacity(a * 0.3)
                    .reduced(reduced),
                );
            }
            layers.push(
                Fx::glow(GlowParams {
                    style: if button && active {
                        GlowStyle::Aura
                    } else {
                        GlowStyle::Soft
                    },
                    color: rgb(0xf59e0b),
                    core: rgb(0xfbbf24),
                    third: rgb(0xb45309),
                    radius: if button { 12.0 } else { 8.0 },
                    intensity: strength,
                    breathe: if active && button { 0.12 } else { 0.0 },
                    aura_speed: if active { 0.1 } else { 0.0 },
                    inner: 0.0,
                    ..Default::default()
                })
                .on(shape)
                .over()
                .at(now)
                .opacity(a)
                .reduced(reduced),
            );
            if button && phase == Phase::Recording {
                layers.push(
                    Fx::pulse(PulseParams {
                        style: PulseStyle::Breathe,
                        color: rgb(0xf59e0b),
                        core: rgb(0xfbbf24),
                        intensity: 0.18,
                        reach: 10.0,
                        period: 2.4,
                        ..Default::default()
                    })
                    .on(shape)
                    .over()
                    .at(now)
                    .opacity(a)
                    .reduced(reduced),
                );
            }
            if button && matches!(phase, Phase::Loading | Phase::Transcribing) {
                layers.push(
                    Fx::shimmer(ShimmerParams {
                        color: rgb(0xfbbf24),
                        intensity: 0.12,
                        period: 2.0,
                        ..Default::default()
                    })
                    .on(shape)
                    .over()
                    .at(now)
                    .opacity(a)
                    .reduced(reduced),
                );
            }
        }
        if phase == Phase::Recording {
            let fraction = crate::ui::meter_fraction(f.rt.state.level);
            for i in 0..32 {
                if i as f32 >= fraction * 32.0 {
                    break;
                }
                if let Some((r, a)) = f.rt.presented(&format!("bar-{i}")) {
                    layers.push(
                        Fx::glow(GlowParams {
                            color: rgb(0xf59e0b),
                            radius: 3.0,
                            intensity: 0.12,
                            inner: 0.0,
                            ..Default::default()
                        })
                        .on(Shape::rounded(r[0], r[1], r[2], r[3], 2.0))
                        .over()
                        .at(now)
                        .opacity(a)
                        .reduced(reduced),
                    );
                }
            }
        }
        self.animating = !reduced && (active || flash > 0.0 || copy > 0.0);
        let device_key = f.device_key();
        if let Some((key, format, rx)) = &self.pending {
            if *key != device_key || *format != f.format {
                self.pending = None;
            } else if let Ok(fx) = rx.try_recv() {
                self.gpu = Some((*key, *format, fx));
                self.pending = None;
            }
        }
        if self
            .gpu
            .as_ref()
            .is_none_or(|(key, format, _)| *key != device_key || *format != f.format)
        {
            if rune::is_headless() {
                self.gpu = Some((
                    device_key,
                    f.format,
                    FxRenderer::new(f.device, f.format, FxOptions { dither: true }),
                ));
            } else {
                if self.pending.is_none() {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let (device, queue, format) = (f.device.clone(), f.queue.clone(), f.format);
                    let srgb = f.srgb_blend();
                    // FXC compiles these shaders slowly; let the first UI frame present first.
                    if std::thread::Builder::new()
                        .name("fi-whisper-effects".into())
                        .spawn(move || {
                            let fx = prepare(&device, &queue, format, srgb);
                            if tx.send(fx).is_ok()
                                && let Some(h) = crate::APP_HANDLE.get()
                            {
                                h.wake();
                            }
                        })
                        .is_ok()
                    {
                        self.pending = Some((device_key, format, rx));
                    }
                }
                f.draw_ui();
                return;
            }
        }
        let srgb = f.srgb_blend();
        let fx = &mut self.gpu.as_mut().unwrap().2;
        fx.set_srgb_blend(srgb);
        fx.prepare(
            f.device,
            f.queue,
            &FxFrame {
                viewport: [f.size.0, f.size.1],
                scale: f.rt.env.scale,
                layers: &layers,
                backdrop: None,
                sources: &[],
            },
        );
        f.draw_ui();
        let mut pass = f.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("amber glow"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: f.target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        if let Some((r, _)) = f.rt.presented("content") {
            let scale = f.rt.env.scale;
            let top = ((r[1] - 12.0) * scale).max(0.0).min(f.size.1 as f32) as u32;
            let bottom = ((r[1] + r[3] + 12.0) * scale)
                .max(top as f32)
                .min(f.size.1 as f32) as u32;
            if bottom > top {
                let menu =
                    f.rt.state
                        .menu
                        .as_ref()
                        .and_then(|key| f.rt.presented(&format!("{key}-content")));
                // Card glows must stay behind an open dropdown's text.
                let regions = if let Some((m, _)) = menu {
                    let left = (m[0] * scale).floor().clamp(0.0, f.size.0 as f32) as u32;
                    let right = ((m[0] + m[2]) * scale)
                        .ceil()
                        .clamp(left as f32, f.size.0 as f32) as u32;
                    let above = (m[1] * scale).floor().clamp(top as f32, bottom as f32) as u32;
                    let below = ((m[1] + m[3]) * scale)
                        .ceil()
                        .clamp(above as f32, bottom as f32) as u32;
                    [
                        [0, top, f.size.0, above - top],
                        [0, below, f.size.0, bottom - below],
                        [0, above, left, below - above],
                        [right, above, f.size.0 - right, below - above],
                    ]
                } else {
                    [[0, top, f.size.0, bottom - top], [0; 4], [0; 4], [0; 4]]
                };
                for [x, y, width, height] in regions {
                    if width > 0 && height > 0 {
                        pass.set_scissor_rect(x, y, width, height);
                        fx.draw_all(&mut pass, &layers, None);
                    }
                }
            }
        }
    }
}

fn prepare(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    srgb: bool,
) -> FxRenderer {
    let mut fx = FxRenderer::new(device, format, FxOptions { dither: true });
    fx.set_srgb_blend(srgb);
    // Warm only our four effect families, including the busy states, on the worker.
    let shape = Shape::rounded(0.0, 0.0, 32.0, 32.0, 6.0);
    let layers = [
        Fx::glow(GlowParams::default()).on(shape).over(),
        Fx::fill(PlasticParams::default()).on(shape).over(),
        Fx::pulse(PulseParams::default()).on(shape).over(),
        Fx::shimmer(ShimmerParams::default()).on(shape).over(),
    ];
    fx.prepare(
        device,
        queue,
        &FxFrame {
            viewport: [32, 32],
            scale: 1.0,
            layers: &layers,
            backdrop: None,
            sources: &[],
        },
    );
    fx
}

#[cfg(test)]
mod tests {
    #[test]
    fn worker_prepares_all_used_effect_families() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let gpu = rune::render::Gpu::headless();
            let fx = super::prepare(
                &gpu.device,
                &gpu.queue,
                wgpu::TextureFormat::Bgra8Unorm,
                true,
            );
            tx.send(fx).unwrap();
        });
        let fx = rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap();
        assert_eq!(fx.built_pipelines(), 4);
    }
}

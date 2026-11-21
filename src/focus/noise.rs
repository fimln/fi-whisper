use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
fn fatal_audio_error(kind: cpal::ErrorKind) -> bool {
    !matches!(
        kind,
        cpal::ErrorKind::Xrun | cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied
    )
}
// High-pass Butterworth 20 Hz dari RBJ Audio EQ Cookbook.
// f64 menjaga presisi dekat DC pada sample rate tinggi.
struct BrownHighPass {
    b0: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}
impl BrownHighPass {
    fn new(sample_rate: u32) -> Self {
        let w = 2.0 * std::f64::consts::PI * 20.0 / sample_rate as f64;
        let alpha = w.sin() / std::f64::consts::SQRT_2;
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 + w.cos()) / (2.0 * a0),
            a1: -2.0 * w.cos() / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }
    fn next(&mut self, input: f32) -> f32 {
        let x = input as f64;
        let y = self.b0 * x + self.z1;
        self.z1 = -2.0 * self.b0 * x - self.a1 * y + self.z2;
        self.z2 = self.b0 * x - self.a2 * y;
        y as f32
    }
}
pub(super) fn open(
    state: Arc<AtomicU8>,
    failed: impl Fn() + Send + 'static,
) -> Result<cpal::Stream, String> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or("No audio output device")?;
    let config = device.default_output_config().map_err(|e| e.to_string())?;
    let channels = usize::from(config.channels());
    let sample = generator(state, channels, config.sample_rate());
    let err = move |e: cpal::Error| {
        if fatal_audio_error(e.kind()) {
            failed();
        }
    };
    let format = config.sample_format();
    let config = config.into();
    fn build<T: SizedSample + FromSample<f32>>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        channels: usize,
        mut sample: impl FnMut(usize) -> f32 + Send + 'static,
        err: impl FnMut(cpal::Error) + Send + 'static,
    ) -> Result<cpal::Stream, cpal::Error> {
        device.build_output_stream(
            config,
            move |data: &mut [T], _| {
                for frame in data.chunks_mut(channels) {
                    for (channel, value) in frame.iter_mut().enumerate() {
                        *value = T::from_sample(sample(channel));
                    }
                }
            },
            err,
            None,
        )
    }
    let stream = match format {
        cpal::SampleFormat::F32 => build::<f32>(&device, config, channels, sample, err),
        cpal::SampleFormat::I16 => build::<i16>(&device, config, channels, sample, err),
        cpal::SampleFormat::U16 => build::<u16>(&device, config, channels, sample, err),
        _ => return Err(format!("Unsupported output format ({format})")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(stream)
}

fn generator(state: Arc<AtomicU8>, channels: usize, sample_rate: u32) -> impl FnMut(usize) -> f32 {
    let mut generators: Vec<_> = (0..channels)
        .map(|channel| {
            (
                0x12345678u32.wrapping_add((channel as u32).wrapping_mul(0x9e3779b9)),
                [0f32; 7],
                0f32,
                BrownHighPass::new(sample_rate),
            )
        })
        .collect();
    // Kutub 5 Hz membatasi drift DC pada brown noise.
    let brown_pole = (-2.0 * std::f32::consts::PI * 5.0 / sample_rate as f32).exp();
    let brown_input = (1.0 - brown_pole * brown_pole).sqrt();
    let step = 1.0 / (sample_rate as f32 * 0.02);
    let mut mode = 0u8;
    let mut gain = 0f32;
    move |channel: usize| -> f32 {
        if channel == 0 {
            let target = state.load(Ordering::Relaxed);
            if target != mode {
                gain = (gain - step).max(0.0);
                if gain == 0.0 {
                    mode = target;
                }
            } else {
                gain = (gain + step).min(1.0);
            }
        }
        if mode == 0 {
            return 0.0;
        }
        let (seed, b, brown, high_pass) = &mut generators[channel];
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        let w = (*seed as f32 / u32::MAX as f32 * 2.0 - 1.0) / 30.0;
        let value = match mode {
            1 => w,
            2 => {
                b[0] = 0.99886 * b[0] + w * 0.0555179;
                b[1] = 0.99332 * b[1] + w * 0.0750759;
                b[2] = 0.969 * b[2] + w * 0.153852;
                b[3] = 0.8665 * b[3] + w * 0.3104856;
                b[4] = 0.55 * b[4] + w * 0.5329522;
                b[5] = -0.7616 * b[5] - w * 0.016898;
                let x = (b.iter().sum::<f32>() + w * 0.5362) * 0.33;
                b[6] = w * 0.115926;
                x
            }
            3 => {
                *brown = brown_pole * *brown + brown_input * w;
                high_pass.next(*brown) * 2.2
            }
            _ => 0.0,
        };
        value * gain
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "memerlukan mikrofon dan perangkat output Windows"]
    fn recording_completes_while_noise_is_playing() {
        use crate::audio::{self, CaptureEnd};
        use std::{sync::mpsc, thread, time::Duration};

        let mode = Arc::new(AtomicU8::new(super::super::Noise::White as u8));
        let (error_tx, error_rx) = mpsc::channel();
        let output = open(mode.clone(), move || {
            let _ = error_tx.send(());
        })
        .unwrap();
        let (tx, rx) = mpsc::channel();
        let capture = audio::start(None, move |end| {
            let _ = tx.send(end);
        })
        .unwrap();
        for noise in super::super::Noise::ALL {
            mode.store(noise as u8, Ordering::Relaxed);
            thread::sleep(Duration::from_millis(300));
        }
        capture.stop();
        match rx.recv_timeout(Duration::from_secs(4)).unwrap() {
            CaptureEnd::Done(recording) => {
                assert!(recording.samples.len() >= audio::RATE as usize / 2)
            }
            CaptureEnd::Failed(error) => panic!("{error}"),
            CaptureEnd::Cancelled => panic!("capture cancelled"),
        }
        assert!(matches!(
            error_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        drop(output);
    }

    #[test]
    fn brown_high_pass_rejects_dc_and_has_butterworth_cutoff() {
        for rate in [44100, 48000, 96000] {
            let mut filter = BrownHighPass::new(rate);
            for _ in 0..rate {
                filter.next(1.0);
            }
            assert!(filter.next(1.0).abs() < 1e-6);
            let mut filter = BrownHighPass::new(rate);
            let mut energy = 0.0f64;
            for n in 0..rate * 2 {
                let input = (2.0 * std::f64::consts::PI * 20.0 * n as f64 / rate as f64).sin();
                let output = filter.next(input as f32);
                if n >= rate {
                    energy += (output as f64).powi(2);
                }
            }
            assert!((energy / rate as f64 - 0.25).abs() < 0.001);
        }
    }
    #[test]
    fn recoverable_notifications_keep_playing() {
        for kind in [
            cpal::ErrorKind::Xrun,
            cpal::ErrorKind::DeviceChanged,
            cpal::ErrorKind::RealtimeDenied,
        ] {
            assert!(!fatal_audio_error(kind));
        }
        for kind in [
            cpal::ErrorKind::DeviceNotAvailable,
            cpal::ErrorKind::StreamInvalidated,
            cpal::ErrorKind::BackendError,
        ] {
            assert!(fatal_audio_error(kind));
        }
    }
}

//! Capture mikrofon CPAL/WASAPI, downmix mono, dan resampling ke 16 kHz.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use rubato::audioadapter_buffers::owned::InterleavedOwned;
use rubato::{Fft, FixedSync, Resampler};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub const RATE: u32 = 16_000;
pub const LIMIT: Duration = Duration::from_secs(120);
const DRAIN_EVERY: Duration = Duration::from_millis(20);

#[derive(Clone, Debug, PartialEq)]
pub struct Mic {
    pub id: String,
    pub name: String,
}

/// Daftar perangkat input dan ID default sistem.
pub fn list_inputs() -> Result<(Vec<Mic>, Option<String>), String> {
    let host = cpal::default_host();
    let default = host
        .default_input_device()
        .and_then(|d| d.id().ok())
        .map(|id| id.to_string());
    let devices = host
        .input_devices()
        .map_err(|e| format!("Perangkat input tidak dapat dibaca: {e}"))?;
    let mics = devices
        .filter_map(|d| {
            let id = d.id().ok()?.to_string();
            let name = d
                .description()
                .map(|desc| desc.name().to_owned())
                .unwrap_or_else(|_| id.clone());
            Some(Mic { id, name })
        })
        .collect();
    Ok((mics, default))
}

pub struct Recording {
    /// PCM mono f32 16 kHz.
    pub samples: Vec<f32>,
    pub captured: Duration,
    pub hit_limit: bool,
}

pub enum CaptureEnd {
    Done(Recording),
    Cancelled,
    Failed(String),
}

enum Signal {
    Stop,
    Cancel,
}

/// Sesi capture yang sedang berjalan. Stream dimiliki thread capture dan dilepas saat berhenti.
pub struct Capture {
    signal: mpsc::Sender<Signal>,
    pub level: Arc<AtomicU32>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Capture {
    pub fn stop(&self) {
        let _ = self.signal.send(Signal::Stop);
    }

    pub fn cancel(&self) {
        let _ = self.signal.send(Signal::Cancel);
    }
}

/// Mulai capture. Error setup (perangkat, izin, format) dikembalikan langsung;
/// hasil akhir dikirim lewat `on_end` dari thread capture.
pub fn start(
    mic: Option<String>,
    on_end: impl FnOnce(CaptureEnd) + Send + 'static,
) -> Result<Capture, String> {
    let (signal, signal_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let level = Arc::new(AtomicU32::new(0));
    let thread_level = level.clone();
    let thread = thread::Builder::new()
        .name("fi-whisper-capture".into())
        .spawn(move || capture_thread(mic, signal_rx, ready_tx, thread_level, on_end))
        .map_err(|e| format!("Thread audio gagal dibuat: {e}"))?;
    match ready_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(Capture {
            signal,
            level,
            thread: Some(thread),
        }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("Mikrofon tidak merespons saat dibuka.".into()),
    }
}

struct Shared {
    overflow: AtomicBool,
    level: Arc<AtomicU32>,
    error: Mutex<Option<String>>,
}

fn capture_thread(
    mic: Option<String>,
    signal: mpsc::Receiver<Signal>,
    ready: mpsc::SyncSender<Result<(), String>>,
    level: Arc<AtomicU32>,
    on_end: impl FnOnce(CaptureEnd),
) {
    let shared = Arc::new(Shared {
        overflow: AtomicBool::new(false),
        level,
        error: Mutex::new(None),
    });
    let (stream, mut consumer, rate) = match open(mic, shared.clone()) {
        Ok(v) => {
            if ready.send(Ok(())).is_err() {
                return;
            }
            v
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let limit = rate as usize * LIMIT.as_secs() as usize;
    let mut samples = Vec::with_capacity(limit);
    let drain = |samples: &mut Vec<f32>, consumer: &mut rtrb::Consumer<f32>| {
        let n = consumer.slots().min(limit - samples.len());
        if let Ok(chunk) = consumer.read_chunk(n) {
            let (a, b) = chunk.as_slices();
            samples.extend_from_slice(a);
            samples.extend_from_slice(b);
            chunk.commit_all();
        }
    };
    let end = loop {
        let signal = signal.recv_timeout(DRAIN_EVERY);
        drain(&mut samples, &mut consumer);
        if shared.overflow.load(Ordering::Relaxed) {
            break Err("Buffer audio penuh; rekaman dihentikan agar tidak ada audio yang hilang diam-diam.".to_owned());
        }
        if let Some(e) = shared.error.lock().ok().and_then(|mut e| e.take()) {
            break Err(e);
        }
        match signal {
            Ok(Signal::Stop) | Err(RecvTimeoutError::Disconnected) => break Ok(false),
            Ok(Signal::Cancel) => {
                drop(stream);
                on_end(CaptureEnd::Cancelled);
                return;
            }
            Err(RecvTimeoutError::Timeout) if samples.len() >= limit => break Ok(true),
            Err(RecvTimeoutError::Timeout) => {}
        }
    };
    // Hentikan stream dulu, lalu kuras sampel terakhir yang sudah masuk buffer.
    let _ = stream.pause();
    drop(stream);
    drain(&mut samples, &mut consumer);
    shared.level.store(0, Ordering::Relaxed);
    let hit_limit = match end {
        Ok(hit) => hit || samples.len() >= limit,
        Err(e) => return on_end(CaptureEnd::Failed(e)),
    };
    let captured = Duration::from_secs_f64(samples.len() as f64 / rate as f64);
    match resample(samples, rate) {
        Ok(samples) => on_end(CaptureEnd::Done(Recording {
            samples,
            captured,
            hit_limit,
        })),
        Err(e) => on_end(CaptureEnd::Failed(e)),
    }
}

fn open(
    mic: Option<String>,
    shared: Arc<Shared>,
) -> Result<(cpal::Stream, rtrb::Consumer<f32>, u32), String> {
    let host = cpal::default_host();
    let device = match &mic {
        Some(id) => id
            .parse()
            .ok()
            .and_then(|id| host.device_by_id(&id))
            .ok_or("Mikrofon pilihan tidak ditemukan. Pilih ulang di Pengaturan.")?,
        None => host
            .default_input_device()
            .ok_or("Tidak ada mikrofon default di sistem.")?,
    };
    let supported = device
        .default_input_config()
        .map_err(|e| describe(&e, "Konfigurasi mikrofon tidak dapat dibaca"))?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let channels = config.channels as usize;
    if channels == 0 {
        return Err("Mikrofon melaporkan 0 channel.".into());
    }
    // Buffer 2 detik; worker menguras setiap 20 ms.
    let (producer, consumer) = rtrb::RingBuffer::new(config.sample_rate as usize * 2);
    let stream = match format {
        SampleFormat::I8 => build::<i8>(&device, &config, producer, shared),
        SampleFormat::I16 => build::<i16>(&device, &config, producer, shared),
        SampleFormat::I24 => build::<cpal::I24>(&device, &config, producer, shared),
        SampleFormat::I32 => build::<i32>(&device, &config, producer, shared),
        SampleFormat::U8 => build::<u8>(&device, &config, producer, shared),
        SampleFormat::U16 => build::<u16>(&device, &config, producer, shared),
        SampleFormat::U32 => build::<u32>(&device, &config, producer, shared),
        SampleFormat::F32 => build::<f32>(&device, &config, producer, shared),
        SampleFormat::F64 => build::<f64>(&device, &config, producer, shared),
        other => return Err(format!("Format sampel mikrofon tidak didukung: {other}")),
    }?;
    stream
        .play()
        .map_err(|e| describe(&e, "Mikrofon tidak dapat dimulai"))?;
    Ok((stream, consumer, config.sample_rate))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut producer: rtrb::Producer<f32>,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    let errors = shared.clone();
    device
        .build_input_stream(
            *config,
            move |data: &[T], _: &_| {
                push_frames(
                    data,
                    channels,
                    &mut producer,
                    &shared.overflow,
                    &shared.level,
                )
            },
            move |e: cpal::Error| {
                if let Ok(mut slot) = errors.error.lock() {
                    slot.get_or_insert_with(|| describe(&e, "Mikrofon bermasalah"));
                }
            },
            None,
        )
        .map_err(|e| describe(&e, "Stream mikrofon gagal dibuat"))
}

fn describe(e: &cpal::Error, context: &str) -> String {
    match e.kind() {
        cpal::ErrorKind::PermissionDenied => {
            "Akses mikrofon ditolak. Izinkan aplikasi desktop memakai mikrofon di Pengaturan Privasi Windows.".into()
        }
        cpal::ErrorKind::DeviceNotAvailable => {
            "Mikrofon tidak tersedia atau terlepas. Pilih ulang di Pengaturan.".into()
        }
        _ => format!("{context}: {e}"),
    }
}

/// Callback audio: konversi ke f32, downmix rata-rata channel, masukkan ke ring buffer.
/// Tidak mengalokasi, tidak blocking, tidak melakukan I/O.
fn push_frames<T>(
    data: &[T],
    channels: usize,
    producer: &mut rtrb::Producer<f32>,
    overflow: &AtomicBool,
    level: &AtomicU32,
) where
    T: Sample,
    f32: FromSample<T>,
{
    let mut peak = 0.0f32;
    for frame in data.chunks_exact(channels) {
        let sum: f32 = frame.iter().map(|s| f32::from_sample(*s)).sum();
        let mono = sum / channels as f32;
        peak = peak.max(mono.abs());
        if producer.push(mono).is_err() {
            overflow.store(true, Ordering::Relaxed);
            return;
        }
    }
    // Bit f32 positif terurut sama dengan nilainya, jadi fetch_max aman untuk peak.
    level.fetch_max(peak.min(1.0).to_bits(), Ordering::Relaxed);
}

/// Ambil dan reset level puncak sejak pembacaan terakhir (0..1).
pub fn take_level(level: &AtomicU32) -> f32 {
    f32::from_bits(level.swap(0, Ordering::Relaxed))
}

/// Resample seluruh klip ke 16 kHz. Resampler baru tiap sesi; `process_all`
/// memotong delay awal dan memompa tail sampai panjang output lengkap.
pub fn resample(samples: Vec<f32>, rate: u32) -> Result<Vec<f32>, String> {
    if rate == RATE || samples.is_empty() {
        return Ok(samples);
    }
    let len = samples.len();
    let mut resampler = Fft::<f32>::new(rate as usize, RATE as usize, 1024, 1, FixedSync::Input)
        .map_err(|e| format!("Resampler gagal dibuat: {e}"))?;
    let input = InterleavedOwned::new_from(samples, 1, len)
        .map_err(|e| format!("Buffer resampling tidak valid: {e}"))?;
    resampler
        .process_all(&input, len, None)
        .map(InterleavedOwned::take_data)
        .map_err(|e| format!("Resampling gagal: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_capture_cancels_and_joins() {
        let (signal, rx) = mpsc::channel();
        let done = Arc::new(AtomicBool::new(false));
        let thread_done = done.clone();
        let capture = Capture {
            signal,
            level: Arc::new(AtomicU32::new(0)),
            thread: Some(thread::spawn(move || {
                assert!(matches!(rx.recv().unwrap(), Signal::Cancel));
                thread_done.store(true, Ordering::SeqCst);
            })),
        };
        drop(capture);
        assert!(done.load(Ordering::SeqCst));
    }

    fn run<T: Sample>(data: &[T], channels: usize, capacity: usize) -> (Vec<f32>, bool, f32)
    where
        f32: FromSample<T>,
    {
        let (mut p, mut c) = rtrb::RingBuffer::new(capacity);
        let overflow = AtomicBool::new(false);
        let level = AtomicU32::new(0);
        push_frames(data, channels, &mut p, &overflow, &level);
        let mut out = Vec::new();
        while let Ok(s) = c.pop() {
            out.push(s);
        }
        (out, overflow.load(Ordering::Relaxed), take_level(&level))
    }

    #[test]
    fn converts_signed_unsigned_and_downmixes() {
        let (out, overflow, peak) = run(&[i16::MAX, i16::MIN, 0, 0], 2, 8);
        assert!(!overflow);
        assert_eq!(out.len(), 2);
        assert!(out[0].abs() < 1e-3 && out[1] == 0.0);
        assert_eq!(peak, out[0].abs());

        let (out, ..) = run(&[u16::MAX, u16::MAX, 32768, 32768], 2, 8);
        assert!((out[0] - 1.0).abs() < 1e-3 && out[1].abs() < 1e-3);

        let (out, ..) = run(&[0u8, 128u8], 1, 8);
        assert!((out[0] + 1.0).abs() < 1e-6 && out[1].abs() < 1e-6);
    }

    #[test]
    fn overflow_is_reported() {
        let (out, overflow, _) = run(&[0.1f32; 10], 1, 4);
        assert_eq!(out.len(), 4);
        assert!(overflow);
    }

    #[test]
    fn resampling_keeps_duration_and_tail() {
        let rate = 48_000;
        // 1 detik sinus 440 Hz; ujung klip harus tetap ada setelah resampling.
        let input: Vec<f32> = (0..rate)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.5)
            .collect();
        let out = resample(input, rate).unwrap();
        assert_eq!(out.len(), RATE as usize);
        let tail = &out[out.len() - 160..];
        assert!(tail.iter().any(|s| s.abs() > 0.2), "tail hilang");
        let head = &out[..160];
        assert!(
            head.iter().any(|s| s.abs() > 0.2),
            "delay awal tidak dipotong"
        );
        assert_eq!(resample(vec![0.5; 10], RATE).unwrap().len(), 10);
    }
}

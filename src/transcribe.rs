//! Worker tunggal yang memiliki model ASR dan VAD; model dimuat sekali dan dipakai ulang.

use crate::config::Language;
use std::ffi::{CStr, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, Once};
use std::thread;
use std::time::{Duration, Instant};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
    WhisperVadContext, WhisperVadContextParams, WhisperVadParams,
};

const SAMPLES_PER_CS: f32 = 160.0; // 16 kHz, timestamp VAD dalam centidetik
const RANGE_PAD: usize = 3_200; // 200 ms padding di awal/akhir rentang ucapan
const MIN_ASR: usize = 16_000; // whisper.cpp kurang stabil untuk audio < 1 detik
const NO_SPEECH_MAX: f32 = 0.6; // sama dengan no_speech_thold bawaan Whisper

pub enum Job {
    Load {
        asr: PathBuf,
        vad: PathBuf,
    },
    Transcribe {
        session: u64,
        samples: Vec<f32>,
        language: Language,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Text(String),
    NoSpeech,
    Cancelled,
    Failed(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Timing {
    pub audio: Duration,
    pub vad: Duration,
    pub asr: Duration,
}

pub enum Event {
    Loaded {
        asr: PathBuf,
        vad: PathBuf,
        backend: String,
        diagnostics: Vec<String>,
        took: Duration,
    },
    LoadFailed(String),
    Finished {
        session: u64,
        outcome: Outcome,
        timing: Timing,
    },
}

pub struct Worker {
    jobs: mpsc::Sender<Job>,
    /// Sesi dengan ID <= nilai ini dibatalkan; dibaca abort callback whisper.cpp.
    cancelled: Arc<AtomicU64>,
}

impl Worker {
    pub fn spawn(events: impl Fn(Event) + Send + 'static) -> Self {
        install_log_capture();
        let (jobs, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicU64::new(0));
        let flag = cancelled.clone();
        thread::Builder::new()
            .name("fi-whisper-asr".into())
            .spawn(move || run(rx, flag, events))
            .expect("thread worker ASR");
        Self { jobs, cancelled }
    }

    pub fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    pub fn cancel(&self, session: u64) {
        self.cancelled.fetch_max(session, Ordering::SeqCst);
    }
}

struct Models {
    // `state` harus di-drop sebelum context.
    state: WhisperState,
    _ctx: WhisperContext,
    vad: WhisperVadContext,
}

fn run(rx: mpsc::Receiver<Job>, cancelled: Arc<AtomicU64>, events: impl Fn(Event)) {
    let mut models: Option<Models> = None;
    let threads = thread::available_parallelism().map_or(4, |n| n.get().min(8)) as i32;
    while let Ok(job) = rx.recv() {
        match job {
            Job::Load { asr, vad } => {
                let started = Instant::now();
                take_log();
                // Model lama tetap aktif sampai model baru berhasil dimuat.
                match load(&asr, &vad, threads) {
                    Ok(new) => {
                        drop(models.replace(new));
                        let diagnostics = take_log();
                        events(Event::Loaded {
                            backend: backend_summary(&diagnostics),
                            asr,
                            vad,
                            diagnostics,
                            took: started.elapsed(),
                        });
                    }
                    Err(e) => events(Event::LoadFailed(e)),
                }
            }
            Job::Transcribe {
                session,
                samples,
                language,
            } => {
                let timing = Timing {
                    audio: Duration::from_secs_f64(samples.len() as f64 / 16_000.0),
                    ..Timing::default()
                };
                let (outcome, timing) = match models.as_mut() {
                    None => (Outcome::Failed("Model belum dimuat.".into()), timing),
                    Some(m) => {
                        transcribe(m, session, &samples, language, threads, &cancelled, timing)
                    }
                };
                events(Event::Finished {
                    session,
                    outcome,
                    timing,
                });
            }
        }
    }
}

fn path_str(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("Path model tidak valid: {}", path.display()))
}

fn load(asr: &Path, vad: &Path, threads: i32) -> Result<Models, String> {
    for (path, label) in [(asr, "ASR"), (vad, "VAD")] {
        if !path.is_file() {
            return Err(format!(
                "File model {label} tidak ditemukan: {}",
                path.display()
            ));
        }
    }
    let mut params = WhisperContextParameters::default();
    params.use_gpu(cfg!(feature = "cuda"));
    let ctx = WhisperContext::new_with_params(path_str(asr)?, params).map_err(|e| {
        format!("Model ASR gagal dimuat ({e}). Pastikan file GGML whisper.cpp yang benar.")
    })?;
    let state = ctx.create_state().map_err(|e| {
        format!("State ASR gagal dibuat ({e}); memori GPU/RAM mungkin tidak cukup.")
    })?;
    let mut vad_params = WhisperVadContextParams::default();
    vad_params.set_n_threads(threads.min(4));
    vad_params.set_use_gpu(false);
    let vad = WhisperVadContext::new(path_str(vad)?, vad_params).map_err(|e| {
        format!("Model VAD gagal dimuat ({e}). Gunakan model Silero GGML untuk whisper.cpp.")
    })?;
    Ok(Models {
        state,
        _ctx: ctx,
        vad,
    })
}

/// Rentang dari ucapan pertama sampai terakhir (jeda di tengah dipertahankan), plus padding.
pub fn speech_range(segments: &[(f32, f32)], len: usize) -> Option<(usize, usize)> {
    let first = segments.first()?.0;
    let last = segments.iter().map(|s| s.1).fold(first, f32::max);
    let start = ((first * SAMPLES_PER_CS) as usize).saturating_sub(RANGE_PAD);
    let end = (((last * SAMPLES_PER_CS) as usize) + RANGE_PAD).min(len);
    (start < end).then_some((start, end))
}

fn transcribe(
    m: &mut Models,
    session: u64,
    samples: &[f32],
    language: Language,
    threads: i32,
    cancelled: &Arc<AtomicU64>,
    mut timing: Timing,
) -> (Outcome, Timing) {
    let is_cancelled = || cancelled.load(Ordering::SeqCst) >= session;
    if samples.is_empty() {
        return (Outcome::NoSpeech, timing);
    }
    let started = Instant::now();
    let segments: Vec<(f32, f32)> = match m
        .vad
        .segments_from_samples(WhisperVadParams::default(), samples)
    {
        Ok(segs) => segs.map(|s| (s.start, s.end)).collect(),
        Err(e) => return (Outcome::Failed(format!("VAD gagal: {e}")), timing),
    };
    timing.vad = started.elapsed();
    let Some((start, end)) = speech_range(&segments, samples.len()) else {
        return (Outcome::NoSpeech, timing);
    };
    if is_cancelled() {
        return (Outcome::Cancelled, timing);
    }
    let mut audio = samples[start..end].to_vec();
    if audio.len() < MIN_ASR {
        audio.resize(MIN_ASR, 0.0);
    }

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(threads);
    params.set_language(language.code());
    params.set_translate(false);
    params.set_no_context(true);
    params.set_no_timestamps(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_suppress_nst(true);
    // set_abort_callback_safe di whisper-rs 0.16 membaca user_data dengan tipe yang salah
    // (Box<dyn FnMut> dibaca sebagai F) sehingga encode bisa dibatalkan acak; pakai callback raw.
    let abort = AbortData {
        cancelled: cancelled.clone(),
        session,
    };
    // SAFETY: `abort` hidup sampai `full` selesai; callback hanya membaca atomic.
    unsafe {
        params.set_abort_callback(Some(abort_callback));
        params.set_abort_callback_user_data(&abort as *const AbortData as *mut c_void);
    }

    let started = Instant::now();
    let result = m.state.full(params, &audio);
    drop(abort);
    timing.asr = started.elapsed();
    if is_cancelled() {
        return (Outcome::Cancelled, timing);
    }
    if let Err(e) = result {
        return (Outcome::Failed(format!("Transkripsi gagal: {e}")), timing);
    }
    // Segmen yang menurut model kemungkinan besar bukan ucapan dibuang; ini menahan halusinasi
    // umum Whisper (mis. "Terima kasih.") pada noise singkat yang lolos VAD.
    let text = join_segments(m.state.as_iter().filter_map(|s| {
        (s.no_speech_probability() < NO_SPEECH_MAX)
            .then(|| s.to_str_lossy().ok().map(|t| t.into_owned()))
            .flatten()
    }));
    if text.is_empty() {
        (Outcome::NoSpeech, timing)
    } else {
        (Outcome::Text(text), timing)
    }
}

struct AbortData {
    cancelled: Arc<AtomicU64>,
    session: u64,
}

unsafe extern "C" fn abort_callback(user_data: *mut c_void) -> bool {
    // SAFETY: user_data menunjuk ke AbortData yang hidup selama whisper_full.
    let data = unsafe { &*(user_data as *const AbortData) };
    data.cancelled.load(Ordering::SeqCst) >= data.session
}

/// Gabungkan segmen dengan satu spasi dan rapikan whitespace; tidak menghapus kata.
pub fn join_segments(segments: impl IntoIterator<Item = String>) -> String {
    let mut out = String::new();
    for seg in segments {
        for word in seg.split_whitespace() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(word);
        }
    }
    out
}

// --- Log whisper.cpp/ggml: hanya baris diagnostik backend yang disimpan, tidak dicetak. ---

static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
static LOG_ONCE: Once = Once::new();

fn install_log_capture() {
    LOG_ONCE.call_once(|| unsafe {
        whisper_rs::whisper_rs_sys::whisper_log_set(Some(log_callback), std::ptr::null_mut());
        whisper_rs::whisper_rs_sys::ggml_log_set(Some(log_callback), std::ptr::null_mut());
    });
}

unsafe extern "C" fn log_callback(
    _level: whisper_rs::whisper_rs_sys::ggml_log_level,
    text: *const c_char,
    _: *mut c_void,
) {
    if text.is_null() {
        return;
    }
    // SAFETY: whisper.cpp mengirim string C null-terminated yang valid selama callback.
    let line = unsafe { CStr::from_ptr(text) }.to_string_lossy();
    let line = line.trim();
    let keep = [
        "backend",
        "CUDA",
        "GPU",
        "Device ",
        "use gpu",
        "compute capability",
    ]
    .iter()
    .any(|k| line.contains(k));
    if keep
        && let Ok(mut log) = LOG.lock()
        && log.len() < 40
    {
        log.push(line.to_owned());
    }
}

fn take_log() -> Vec<String> {
    LOG.lock()
        .map(|mut l| std::mem::take(&mut *l))
        .unwrap_or_default()
}

fn backend_summary(log: &[String]) -> String {
    let gpu = log.iter().find_map(|l| {
        l.strip_prefix("whisper_backend_init_gpu: using ")
            .and_then(|r| r.strip_suffix(" backend"))
    });
    match gpu {
        Some(name) => format!("GPU {name}"),
        None if cfg!(feature = "cuda") => "CPU · GPU tidak terinisialisasi".into(),
        None => "CPU · build tanpa CUDA".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_range_keeps_middle_pause_and_pads() {
        // Ucapan 1,00-2,00 dtk dan 5,00-6,00 dtk dalam klip 8 dtk.
        let range = speech_range(&[(100.0, 200.0), (500.0, 600.0)], 128_000).unwrap();
        assert_eq!(range, (16_000 - RANGE_PAD, 96_000 + RANGE_PAD));
        assert_eq!(speech_range(&[], 128_000), None);
        assert_eq!(speech_range(&[(0.0, 790.0)], 128_000), Some((0, 128_000)));
    }

    #[test]
    fn join_trims_and_spaces_segments() {
        let text = join_segments([" Halo,  dunia.".to_owned(), "  Apa kabar? ".to_owned()]);
        assert_eq!(text, "Halo, dunia. Apa kabar?");
        assert_eq!(join_segments(["   ".to_owned()]), "");
    }

    #[test]
    fn backend_summary_requires_gpu_log() {
        let log = vec!["whisper_backend_init_gpu: using CUDA0 backend".to_owned()];
        assert_eq!(backend_summary(&log), "GPU CUDA0");
        assert!(backend_summary(&[]).starts_with("CPU"));
    }

    /// Uji model nyata: `cargo test --release -- --ignored fixture` (butuh models/ dan fixtures/).
    #[test]
    #[ignore]
    fn fixture_transcription() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let (tx, rx) = mpsc::channel();
        let worker = Worker::spawn(move |e| {
            let _ = tx.send(e);
        });
        worker.send(Job::Load {
            asr: root.join("models").join(crate::config::ASR_FILE),
            vad: root.join("models").join(crate::config::VAD_FILE),
        });
        match rx.recv().unwrap() {
            Event::Loaded { backend, took, .. } => eprintln!("backend={backend} load={took:?}"),
            Event::LoadFailed(e) => panic!("{e}"),
            Event::Finished { .. } => unreachable!(),
        }
        let silence = vec![0.0; 16_000 * 3];
        worker.send(Job::Transcribe {
            session: 1,
            samples: silence,
            language: Language::Indonesian,
        });
        let Event::Finished { outcome, .. } = rx.recv().unwrap() else {
            panic!()
        };
        assert_eq!(outcome, Outcome::NoSpeech);
        for (i, name) in ["id_id_0000.wav", "id_id_0001.wav", "id_id_0002.wav"]
            .iter()
            .enumerate()
        {
            let bytes = std::fs::read(root.join("fixtures/fleurs-id").join(name)).unwrap();
            // Fixture terverifikasi: PCM16 mono 16 kHz dengan header 44 byte.
            let samples: Vec<f32> = bytes[44..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes(*b) as f32 / 32768.0)
                .collect();
            worker.send(Job::Transcribe {
                session: 2 + i as u64,
                samples,
                language: Language::Indonesian,
            });
            let Event::Finished {
                outcome, timing, ..
            } = rx.recv().unwrap()
            else {
                panic!()
            };
            let Outcome::Text(text) = outcome else {
                panic!("{outcome:?}")
            };
            eprintln!(
                "{name}: audio={:?} vad={:?} asr={:?} chars={}",
                timing.audio,
                timing.vad,
                timing.asr,
                text.chars().count()
            );
            eprintln!("  {text}");
        }
        // Noise pendek (klik 30 ms + desis) tidak boleh menghasilkan teks.
        let mut noise: Vec<f32> = (0..16_000 * 2)
            .map(|i| ((i * 7919 % 1000) as f32 / 1000.0 - 0.5) * 0.002)
            .collect();
        noise[16_000..16_480].iter_mut().for_each(|s| *s = 0.6);
        worker.send(Job::Transcribe {
            session: 10,
            samples: noise,
            language: Language::Indonesian,
        });
        let Event::Finished { outcome, .. } = rx.recv().unwrap() else {
            panic!()
        };
        // Diagnostik saja: hasil noise sintetis dicatat, bukan dijamin.
        eprintln!("noise: {outcome:?}");
    }
}

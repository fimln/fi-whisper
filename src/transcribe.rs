//! Worker tunggal yang memiliki model ASR dan VAD; model dimuat sekali dan dipakai ulang.

use crate::config::{Engine, Language};
use std::ffi::{CStr, c_char, c_void};
use std::panic::{self, AssertUnwindSafe};
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
/// Batas token keluaran Qwen3-ASR; 2 menit ucapan cepat < ~800 token.
const QWEN_MAX_TOKENS: usize = 1024;

pub enum Job {
    Shutdown,
    Load {
        engine: Engine,
        asr: PathBuf,
        vad: PathBuf,
    },
    Transcribe {
        session: u64,
        samples: Vec<f32>,
        language: Language,
    },
    /// Mensimulasikan panic di thread worker.
    #[cfg(test)]
    Panic,
    #[cfg(test)]
    Block {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    },
}

/// Job tidak terkirim karena thread worker sudah berhenti.
#[derive(Debug)]
pub struct WorkerGone;

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
        engine: Engine,
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
    /// Thread worker panic dan berhenti; model ikut hilang.
    Crashed(String),
}

pub struct Worker {
    jobs: mpsc::Sender<Job>,
    /// Sesi dengan ID <= nilai ini dibatalkan; dibaca abort callback whisper.cpp.
    cancelled: Arc<AtomicU64>,
    thread: Option<thread::JoinHandle<()>>,
    done: mpsc::Receiver<()>,
}

impl Worker {
    pub fn spawn(events: impl Fn(Event) + Send + 'static) -> Self {
        let (jobs, rx) = mpsc::channel();
        let (done_tx, done) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicU64::new(0));
        let flag = cancelled.clone();
        let thread = thread::Builder::new()
            .name("fi-whisper-asr".into())
            .spawn(move || {
                // `rx` ikut di-drop saat unwind, jadi `send` berikutnya langsung gagal.
                let result = panic::catch_unwind(AssertUnwindSafe(|| run(rx, flag, &events)));
                if let Err(payload) = result {
                    events(Event::Crashed(panic_message(&*payload)));
                }
                let _ = done_tx.send(());
            })
            .expect("thread worker ASR");
        Self {
            jobs,
            cancelled,
            thread: Some(thread),
            done,
        }
    }

    pub fn send(&self, job: Job) -> Result<(), WorkerGone> {
        self.jobs.send(job).map_err(|_| WorkerGone)
    }

    pub fn cancel(&self, session: u64) {
        self.cancelled.fetch_max(session, Ordering::SeqCst);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel(u64::MAX);
        let _ = self.jobs.send(Job::Shutdown);
        // Native/CUDA yang macet tidak boleh menahan penutupan aplikasi selamanya.
        // Controller melarang penggantian worker yang masih menjalankan job timeout.
        if let Some(thread) = self.thread.take()
            && self.done.recv_timeout(Duration::from_secs(2)).is_ok()
        {
            let _ = thread.join();
        }
    }
}

struct Models {
    asr: Asr,
    vad: WhisperVadContext,
}

enum Asr {
    Whisper(WhisperAsr),
    // Di-box: struct model candle jauh lebih besar dari handle whisper.cpp.
    Qwen(Box<qwen3_asr::AsrInference>),
}

struct WhisperAsr {
    // `state` harus di-drop sebelum context.
    state: WhisperState,
    _ctx: WhisperContext,
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic tanpa pesan".into())
}

fn run(rx: mpsc::Receiver<Job>, cancelled: Arc<AtomicU64>, events: &impl Fn(Event)) {
    let mut models: Option<Models> = None;
    let threads = thread::available_parallelism().map_or(4, |n| n.get().min(8)) as i32;
    while let Ok(job) = rx.recv() {
        if cancelled.load(Ordering::SeqCst) == u64::MAX {
            break;
        }
        match job {
            Job::Shutdown => break,
            Job::Load { engine, asr, vad } => {
                let started = Instant::now();
                take_log();
                // Lepas VRAM sebelum alokasi model baru, termasuk saat muat ulang.
                let had_models = models.take().is_some();
                if engine == Engine::GoogleChirp3 {
                    if had_models {
                        reset_cuda_device();
                    }
                    events(Event::Loaded {
                        engine,
                        asr,
                        vad,
                        backend: "Cloud dipilih".into(),
                        diagnostics: vec![],
                        took: started.elapsed(),
                    });
                    continue;
                }
                install_log_capture();
                match load(engine, &asr, &vad, threads) {
                    Ok((new, qwen_backend)) => {
                        models = Some(new);
                        let diagnostics = take_log();
                        let backend = qwen_backend.unwrap_or_else(|| backend_summary(&diagnostics));
                        events(Event::Loaded {
                            engine,
                            backend,
                            asr,
                            vad,
                            diagnostics,
                            took: started.elapsed(),
                        });
                    }
                    Err(e) => {
                        reset_cuda_device();
                        events(Event::LoadFailed(e));
                    }
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
            #[cfg(test)]
            Job::Panic => panic!("uji panic worker"),
            #[cfg(test)]
            Job::Block { entered, release } => {
                entered.send(()).unwrap();
                let _ = release.recv();
            }
        }
    }
    if models.take().is_some() {
        reset_cuda_device();
    }
}

/// Hancurkan primary context CUDA setelah semua handle model di-drop agar proses keluar dari daftar aktif GPU.
fn reset_cuda_device() {
    #[cfg(feature = "cuda")]
    unsafe {
        unsafe extern "C" {
            fn cudaDeviceReset() -> i32;
        }
        let _ = cudaDeviceReset();
    }
    #[cfg(all(feature = "qwen-cuda", not(feature = "cuda")))]
    unsafe {
        if let Ok(dev) = candle_core::cuda_backend::cudarc::driver::result::device::get(0) {
            let _ = candle_core::cuda_backend::cudarc::driver::result::primary_ctx::reset(dev);
        }
    }
}

fn path_str(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("Path model tidak valid: {}", path.display()))
}

/// Mengembalikan model dan, untuk Qwen3-ASR, ringkasan backend (Whisper memakai log ggml).
fn load(
    engine: Engine,
    asr: &Path,
    vad: &Path,
    threads: i32,
) -> Result<(Models, Option<String>), String> {
    if !vad.is_file() {
        return Err(format!("File model VAD tidak ditemukan: {}", vad.display()));
    }
    if engine == Engine::GoogleChirp3 {
        return Err("Cloud tidak memakai loader lokal.".into());
    }
    let mut vad_params = WhisperVadContextParams::default();
    vad_params.set_n_threads(threads.min(4));
    vad_params.set_use_gpu(false);
    let vad = WhisperVadContext::new(path_str(vad)?, vad_params).map_err(|e| {
        format!("Model VAD gagal dimuat ({e}). Gunakan model Silero GGML untuk whisper.cpp.")
    })?;
    let (asr, backend) = match engine {
        Engine::GoogleChirp3 => unreachable!(),
        Engine::Whisper => (Asr::Whisper(load_whisper(asr)?), None),
        Engine::Qwen3Asr | Engine::Qwen3Asr17 => {
            let (model, backend) = load_qwen(asr)?;
            (Asr::Qwen(Box::new(model)), Some(backend))
        }
    };
    Ok((Models { asr, vad }, backend))
}

fn load_whisper(asr: &Path) -> Result<WhisperAsr, String> {
    if !asr.is_file() {
        return Err(format!("File model ASR tidak ditemukan: {}", asr.display()));
    }
    let mut params = WhisperContextParameters::default();
    params.use_gpu(cfg!(feature = "cuda"));
    let ctx = WhisperContext::new_with_params(path_str(asr)?, params).map_err(|e| {
        format!("Model ASR gagal dimuat ({e}). Pastikan file GGML whisper.cpp yang benar.")
    })?;
    let state = ctx.create_state().map_err(|e| {
        format!("State ASR gagal dibuat ({e}); memori GPU/RAM mungkin tidak cukup.")
    })?;
    Ok(WhisperAsr { state, _ctx: ctx })
}

fn load_qwen(dir: &Path) -> Result<(qwen3_asr::AsrInference, String), String> {
    if !dir.is_dir() {
        return Err(format!(
            "Folder model Qwen3-ASR tidak ditemukan: {}",
            dir.display()
        ));
    }
    crate::config::validate_qwen_dir(dir)
        .map_err(|e| format!("Folder Qwen3-ASR tidak lengkap: {e} ({}).", dir.display()))?;
    #[cfg(feature = "qwen-cuda")]
    let cpu_note = match std::panic::catch_unwind(|| load_qwen_cuda(dir)) {
        Ok(Ok(model)) => return Ok((model, "GPU CUDA0".into())),
        Ok(Err(e)) => {
            reset_cuda_device();
            format!("CPU, CUDA gagal: {}", short_error(&e))
        }
        // cudarc panik bila DLL NVRTC/cuRAND/cuBLAS tidak ditemukan.
        Err(_) => {
            reset_cuda_device();
            "CPU, runtime CUDA tidak lengkap".into()
        }
    };
    #[cfg(not(feature = "qwen-cuda"))]
    let cpu_note = "CPU".to_owned();
    let model = qwen3_asr::AsrInference::load(dir, candle_core::Device::Cpu)
        .map_err(|e| format!("Model Qwen3-ASR gagal dimuat ({e:#})."))?;
    Ok((model, cpu_note))
}

/// Muat di GPU lalu jalankan satu inferensi pendek. Kernel candle di-JIT dari PTX saat pertama
/// dipakai; driver yang lebih lama dari toolkit build menolaknya, jadi diuji di sini (sekaligus
/// pemanasan) agar pemanggil bisa jatuh ke CPU.
#[cfg(feature = "qwen-cuda")]
fn load_qwen_cuda(dir: &Path) -> Result<qwen3_asr::AsrInference, String> {
    let device = candle_core::Device::new_cuda(0).map_err(|e| e.to_string())?;
    let model = qwen3_asr::AsrInference::load(dir, device).map_err(|e| format!("{e:#}"))?;
    let warmup = qwen3_asr::TranscribeOptions::default().with_max_new_tokens(1);
    model
        .transcribe_samples(&vec![0.0; MIN_ASR], warmup)
        .map_err(|e| format!("{e:#}"))?;
    Ok(model)
}

/// Potong pesan error panjang (mis. dari driver CUDA) untuk badge.
#[cfg_attr(not(feature = "qwen-cuda"), allow(dead_code))]
fn short_error(e: &str) -> String {
    let e = e.rsplit_once("DriverError(").map_or(e, |(_, r)| r);
    let e: String = e.chars().take(48).collect();
    e.split([',', '(']).next().unwrap_or(&e).trim().to_owned()
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
    if is_cancelled() {
        return (Outcome::Cancelled, timing);
    }
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

    let started = Instant::now();
    let result = match &mut m.asr {
        Asr::Whisper(w) => run_whisper(w, session, &audio, language, threads, cancelled),
        Asr::Qwen(q) => run_qwen(q, session, &audio, language, cancelled),
    };
    timing.asr = started.elapsed();
    if is_cancelled() {
        return (Outcome::Cancelled, timing);
    }
    match result {
        Err(e) => (Outcome::Failed(format!("Transkripsi gagal: {e}")), timing),
        Ok(text) if text.is_empty() => (Outcome::NoSpeech, timing),
        Ok(text) => (Outcome::Text(text), timing),
    }
}

fn run_whisper(
    w: &mut WhisperAsr,
    session: u64,
    audio: &[f32],
    language: Language,
    threads: i32,
    cancelled: &Arc<AtomicU64>,
) -> Result<String, String> {
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

    let result = w.state.full(params, audio);
    drop(abort);
    result.map_err(|e| e.to_string())?;
    // Segmen yang menurut model kemungkinan besar bukan ucapan dibuang; ini menahan halusinasi
    // umum Whisper (mis. "Terima kasih.") pada noise singkat yang lolos VAD.
    Ok(join_segments(w.state.as_iter().filter_map(|s| {
        (s.no_speech_probability() < NO_SPEECH_MAX)
            .then(|| s.to_str_lossy().ok().map(|t| t.into_owned()))
            .flatten()
    })))
}

fn run_qwen(
    q: &qwen3_asr::AsrInference,
    session: u64,
    audio: &[f32],
    language: Language,
    cancelled: &Arc<AtomicU64>,
) -> Result<String, String> {
    let flag = cancelled.clone();
    let mut options = qwen3_asr::TranscribeOptions::default()
        .with_max_new_tokens(QWEN_MAX_TOKENS)
        .with_abort(move || flag.load(Ordering::SeqCst) >= session);
    if let Some(name) = language.qwen_name() {
        options = options.with_language(name);
    }
    let result = q
        .transcribe_samples(audio, options)
        .map_err(|e| format!("{e:#}"))?;
    Ok(clean_qwen_text(&result.text))
}

/// Buang sisa penanda format Qwen3-ASR (`language X<asr_text>`) bila tokenizer tidak
/// menganggapnya token spesial, lalu rapikan whitespace.
pub fn clean_qwen_text(raw: &str) -> String {
    let text = raw.rsplit_once("<asr_text>").map_or(raw, |(_, t)| t);
    join_segments([text.to_owned()])
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
        None if cfg!(feature = "cuda") => "CPU, GPU tidak terinisialisasi".into(),
        None => "CPU, build tanpa CUDA".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_worker_joins_and_releases_callback() {
        let target = Arc::new(());
        let weak = Arc::downgrade(&target);
        let worker = Worker::spawn(move |_| {
            let _ = &target;
        });
        let cancelled = worker.cancelled.clone();
        drop(worker);
        assert_eq!(cancelled.load(Ordering::SeqCst), u64::MAX);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn stuck_worker_does_not_block_shutdown_indefinitely() {
        let (entered, started) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let worker = Worker::spawn(|_| {});
        worker
            .send(Job::Block {
                entered,
                release: wait,
            })
            .unwrap();
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        let at = Instant::now();
        drop(worker);
        let elapsed = at.elapsed();
        release.send(()).unwrap();
        assert!(elapsed < Duration::from_secs(3));
    }

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

    #[test]
    fn short_error_keeps_driver_code() {
        let e =
            "Inference failed: DriverError(CUDA_ERROR_UNSUPPORTED_PTX_VERSION, \"the provided\")";
        assert_eq!(short_error(e), "CUDA_ERROR_UNSUPPORTED_PTX_VERSION");
        assert_eq!(short_error("no device"), "no device");
    }

    #[test]
    fn qwen_text_drops_format_prefix() {
        assert_eq!(clean_qwen_text("  Halo   dunia. "), "Halo dunia.");
        assert_eq!(
            clean_qwen_text("language Indonesian<asr_text>Halo dunia."),
            "Halo dunia."
        );
        assert_eq!(clean_qwen_text("<asr_text> Apa kabar?"), "Apa kabar?");
        assert_eq!(clean_qwen_text("<asr_text>"), "");
    }

    #[test]
    fn worker_panic_is_reported_and_closes_channel() {
        let (tx, rx) = mpsc::channel();
        let worker = Worker::spawn(move |e| {
            let _ = tx.send(e);
        });
        worker.send(Job::Panic).unwrap();
        let event = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let Event::Crashed(msg) = event else {
            panic!("bukan Crashed")
        };
        assert_eq!(msg, "uji panic worker");
        let job = Job::Transcribe {
            session: 1,
            samples: Vec::new(),
            language: Language::Indonesian,
        };
        assert!(worker.send(job).is_err());
    }

    /// Uji model nyata: `cargo test --release -- --ignored fixture` (butuh models/ dan fixtures/).
    #[test]
    #[ignore]
    fn fixture_transcription() {
        run_fixture(Engine::Whisper);
    }

    /// Butuh `models/Qwen3-ASR-0.6B-hf/` (lihat README).
    #[test]
    #[ignore]
    fn fixture_transcription_qwen() {
        run_fixture(Engine::Qwen3Asr);
    }

    /// Butuh `models/Qwen3-ASR-1.7B-hf/` (lihat README).
    #[test]
    #[ignore]
    fn fixture_transcription_qwen17() {
        run_fixture(Engine::Qwen3Asr17);
    }

    fn fixture_samples(root: &Path, name: &str) -> Vec<f32> {
        let bytes = std::fs::read(root.join("fixtures/fleurs-id").join(name)).unwrap();
        // Fixture terverifikasi: PCM16 mono 16 kHz dengan header 44 byte.
        bytes[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b) as f32 / 32768.0)
            .collect()
    }

    fn word_errors(reference: &str, text: &str) -> (usize, usize) {
        let words = |s: &str| -> Vec<String> {
            s.to_lowercase()
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .map(str::to_owned)
                .collect()
        };
        let a = words(reference);
        let b = words(text);
        let mut row: Vec<_> = (0..=b.len()).collect();
        for (i, x) in a.iter().enumerate() {
            let mut diag = row[0];
            row[0] = i + 1;
            for (j, y) in b.iter().enumerate() {
                let prev = row[j + 1];
                row[j + 1] = (diag + usize::from(x != y)).min(row[j] + 1).min(prev + 1);
                diag = prev;
            }
        }
        (row[b.len()], a.len())
    }

    #[test]
    fn wer_counts_substitutions_deletions_insertions_and_normalizes() {
        assert_eq!(word_errors("Tim-tim, virtual!", "tim tim virtual"), (0, 3));
        assert_eq!(word_errors("a b c", "a x c"), (1, 3));
        assert_eq!(word_errors("a b c", "a c"), (1, 3));
        assert_eq!(word_errors("a b", "a x b"), (1, 2));
        assert_eq!(word_errors("a b", ""), (2, 2));
        assert_eq!(word_errors("", "a"), (1, 0));
    }

    fn run_fixture(engine: Engine) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let models = root.join("models");
        let (tx, rx) = mpsc::channel();
        let worker = Worker::spawn(move |e| {
            let _ = tx.send(e);
        });
        worker
            .send(Job::Load {
                engine,
                asr: models.join(engine.default_name()),
                vad: models
                    .join(crate::config::VAD_DIR)
                    .join(crate::config::VAD_FILE),
            })
            .unwrap();
        match rx
            .recv_timeout(Duration::from_secs(120))
            .expect("worker deadline")
        {
            Event::Loaded { backend, took, .. } => {
                eprintln!("backend={backend} load={took:?}");
                if cfg!(all(feature = "cuda", feature = "qwen-cuda")) {
                    assert!(backend.starts_with("GPU"), "fixture membutuhkan GPU");
                }
            }
            Event::LoadFailed(e) => panic!("{e}"),
            Event::Finished { .. } | Event::Crashed(_) => unreachable!(),
        }
        let silence = vec![0.0; 16_000 * 3];
        worker
            .send(Job::Transcribe {
                session: 1,
                samples: silence,
                language: Language::Indonesian,
            })
            .unwrap();
        let Event::Finished { outcome, .. } = rx
            .recv_timeout(Duration::from_secs(120))
            .expect("worker deadline")
        else {
            panic!()
        };
        assert!(
            matches!(outcome, Outcome::NoSpeech),
            "silence produced text"
        );
        let manifest: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join("fixtures/fleurs-id/manifest.json")).unwrap(),
        )
        .unwrap();
        let fixtures = manifest["samples"].as_array().unwrap();
        // Putaran pertama mengukur first inference; tiga putaran berikut memakai model hangat.
        let mut latencies = Vec::new();
        let mut audio_seconds = 0.0;
        let mut processing_seconds = 0.0;
        let mut errors = 0;
        let mut words = 0;
        for round in 0..4 {
            for (i, sample) in fixtures.iter().enumerate() {
                let name = sample["file"].as_str().unwrap();
                worker
                    .send(Job::Transcribe {
                        session: 2 + (round * fixtures.len() + i) as u64,
                        samples: fixture_samples(root, name),
                        language: Language::Indonesian,
                    })
                    .unwrap();
                let Event::Finished {
                    outcome, timing, ..
                } = rx
                    .recv_timeout(Duration::from_secs(120))
                    .expect("worker deadline")
                else {
                    panic!("expected transcription event")
                };
                let Outcome::Text(text) = outcome else {
                    panic!("fixture did not produce text")
                };
                let (edits, count) = word_errors(sample["reference"].as_str().unwrap(), &text);
                let seconds = (timing.vad + timing.asr).as_secs_f64();
                eprintln!(
                    "round={round} {name}: audio={:.3}s processing={seconds:.3}s WER={edits}/{count}",
                    timing.audio.as_secs_f64()
                );
                if round > 0 {
                    errors += edits;
                    words += count;
                    latencies.push(seconds);
                    audio_seconds += timing.audio.as_secs_f64();
                    processing_seconds += seconds;
                }
            }
        }
        latencies.sort_by(f64::total_cmp);
        let wer = errors as f64 / words as f64;
        eprintln!(
            "warm n={} WER={:.2}% median={:.3}s p95={:.3}s RTF={:.3}",
            latencies.len(),
            wer * 100.0,
            latencies[latencies.len() / 2],
            latencies[(latencies.len() * 95).div_ceil(100) - 1],
            processing_seconds / audio_seconds
        );
        assert!(wer <= 0.20, "fixture WER regression: {wer:.3}");
        // Deteksi bahasa otomatis juga harus menghasilkan teks bersih (tanpa prefiks bahasa).
        worker
            .send(Job::Transcribe {
                session: 100,
                samples: fixture_samples(root, "id_id_0001.wav"),
                language: Language::Auto,
            })
            .unwrap();
        let Event::Finished { outcome, .. } = rx
            .recv_timeout(Duration::from_secs(120))
            .expect("worker deadline")
        else {
            panic!()
        };
        let Outcome::Text(text) = outcome else {
            panic!("expected text")
        };
        assert!(!text.contains("language") && !text.contains("<asr_text>"));
        // Pembatalan saat proses berjalan harus berakhir dengan Cancelled.
        let long: Vec<f32> = ["id_id_0000.wav", "id_id_0001.wav", "id_id_0002.wav"]
            .iter()
            .flat_map(|n| fixture_samples(root, n))
            .collect();
        worker
            .send(Job::Transcribe {
                session: 101,
                samples: long,
                language: Language::Indonesian,
            })
            .unwrap();
        thread::sleep(Duration::from_millis(100));
        let cancelled_at = Instant::now();
        worker.cancel(101);
        let Event::Finished { outcome, .. } = rx
            .recv_timeout(Duration::from_secs(120))
            .expect("worker deadline")
        else {
            panic!()
        };
        eprintln!("cancel latency={:?}", cancelled_at.elapsed());
        assert!(cancelled_at.elapsed() < Duration::from_secs(5));
        assert!(matches!(outcome, Outcome::Cancelled), "cancellation failed");
        // Noise pendek (klik 30 ms + desis) tidak boleh menghasilkan teks.
        let mut noise: Vec<f32> = (0..16_000 * 2)
            .map(|i| ((i * 7919 % 1000) as f32 / 1000.0 - 0.5) * 0.002)
            .collect();
        noise[16_000..16_480].iter_mut().for_each(|s| *s = 0.6);
        worker
            .send(Job::Transcribe {
                session: 102,
                samples: noise,
                language: Language::Indonesian,
            })
            .unwrap();
        let Event::Finished { outcome, .. } = rx
            .recv_timeout(Duration::from_secs(120))
            .expect("worker deadline")
        else {
            panic!()
        };
        assert!(
            matches!(outcome, Outcome::NoSpeech),
            "synthetic noise produced text"
        );
        worker
            .send(Job::Load {
                engine: Engine::GoogleChirp3,
                asr: PathBuf::new(),
                vad: PathBuf::new(),
            })
            .unwrap();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(30))
                .expect("worker deadline"),
            Event::Loaded {
                engine: Engine::GoogleChirp3,
                ..
            }
        ));
        #[cfg(feature = "cuda")]
        unsafe {
            unsafe extern "C" {
                fn cuDevicePrimaryCtxGetState(dev: i32, flags: *mut u32, active: *mut i32) -> i32;
            }
            let (mut flags, mut active) = (0, 1);
            assert_eq!(cuDevicePrimaryCtxGetState(0, &mut flags, &mut active), 0);
            assert_eq!(active, 0, "primary context CUDA masih aktif");
        }
    }
}

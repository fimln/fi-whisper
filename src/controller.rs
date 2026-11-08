//! Koordinasi state di thread sendiri agar hotkey/tray tetap diproses saat jendela tersembunyi.

use crate::audio::{self, Capture, CaptureEnd};
use crate::cloud;
use crate::config::{Engine, Language};
use crate::transcribe::{self, Job, Outcome, Timing, Worker};
use crate::windows::{self, Inserted, Target};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    MissingModel,
    CloudIdle,
    Loading,
    Ready,
    Recording,
    Transcribing,
    Error,
}

impl Phase {
    pub fn busy(self) -> bool {
        matches!(self, Self::Loading | Self::Recording | Self::Transcribing)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Gui,
    Hotkey,
}

/// Snapshot yang dibaca UI.
pub struct Status {
    pub phase: Phase,
    /// Pesan kesalahan yang bisa ditindaklanjuti.
    pub error: String,
    /// Info singkat (sibuk, status penyisipan, dsb).
    pub note: String,
    pub recording_since: Option<Instant>,
    pub captured: Duration,
    pub level: Option<Arc<AtomicU32>>,
    pub result: String,
    pub result_revision: u64,
    pub result_note: String,
    pub backend: String,
    pub diagnostics: Vec<String>,
    /// Mesin, model ASR (file Whisper atau folder Qwen), dan model VAD yang aktif.
    pub loaded: Option<(Engine, PathBuf, PathBuf)>,
    pub load_time: Option<Duration>,
    pub timing: Option<(Timing, Duration)>,
}

pub enum Cmd {
    Shutdown,
    Google(crate::config::GoogleConfig),
    CloudFinished {
        session: u64,
        result: Result<String, String>,
    },
    Toggle(Source),
    Cancel,
    Load {
        engine: Engine,
        asr: PathBuf,
        vad: PathBuf,
    },
    Settings {
        mic: Option<String>,
        language: Language,
    },
    ClearResult,
    Captured {
        session: u64,
        end: CaptureEnd,
    },
    Worker {
        /// Generasi worker pengirim; event dari worker lama diabaikan.
        worker: u64,
        event: transcribe::Event,
    },
    Inserted {
        session: u64,
        outcome: Inserted,
    },
}

pub struct Handle {
    pub tx: Sender<Cmd>,
    pub status: Arc<Mutex<Status>>,
    thread: Option<thread::JoinHandle<()>>,
    shutting_down: Arc<AtomicBool>,
}

impl Handle {
    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    pub fn status(&self) -> MutexGuard<'_, Status> {
        self.status.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        self.send(Cmd::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn spawn(repaint: impl Fn() + Send + 'static) -> Handle {
    let (tx, rx) = mpsc::channel();
    let mut controller = Controller::new(tx.clone());
    let status = controller.status.clone();
    let shutting_down = controller.shutting_down.clone();
    let thread = thread::Builder::new()
        .name("fi-whisper-controller".into())
        .spawn(move || controller.run(rx, repaint))
        .expect("thread controller");
    Handle {
        tx,
        status,
        thread: Some(thread),
        shutting_down,
    }
}

fn spawn_worker(tx: &Sender<Cmd>, generation: u64) -> Worker {
    let tx = tx.clone();
    Worker::spawn(move |event| {
        let _ = tx.send(Cmd::Worker {
            worker: generation,
            event,
        });
    })
}

const NO_BACKEND: &str = "Belum diinisialisasi";

struct Controller {
    tx: Sender<Cmd>,
    status: Arc<Mutex<Status>>,
    worker: Worker,
    google: crate::config::GoogleConfig,
    cloud: Option<cloud::Session>,
    worker_gen: u64,
    mic: Option<String>,
    language: Language,
    /// ID sesi aktif; hasil dengan ID lain diabaikan.
    session: u64,
    capture: Option<Capture>,
    target: Option<Target>,
    stopped_at: Option<Instant>,
    /// Capture sudah dihentikan tetapi audio belum dikirim ke worker.
    awaiting_capture: bool,
    injecting: Option<thread::JoinHandle<()>>,
    shutting_down: Arc<AtomicBool>,
    cancelling: bool,
    worker_deadline: Option<Instant>,
    worker_stalled: bool,
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        if let Some(c) = self.capture.take() {
            c.cancel();
        }
        if let Some(thread) = self.injecting.take() {
            let _ = thread.join();
        }
    }
}

impl Controller {
    fn new(tx: Sender<Cmd>) -> Self {
        let status = Arc::new(Mutex::new(Status {
            phase: Phase::MissingModel,
            error: String::new(),
            note: String::new(),
            recording_since: None,
            captured: Duration::ZERO,
            level: None,
            result: String::new(),
            result_revision: 0,
            result_note: String::new(),
            backend: NO_BACKEND.into(),
            diagnostics: Vec::new(),
            loaded: None,
            load_time: None,
            timing: None,
        }));
        Self {
            worker: spawn_worker(&tx, 0),
            google: Default::default(),
            cloud: None,
            worker_gen: 0,
            tx,
            status,
            mic: None,
            language: Language::Indonesian,
            session: 0,
            capture: None,
            target: None,
            stopped_at: None,
            awaiting_capture: false,
            injecting: None,
            shutting_down: Arc::new(AtomicBool::new(false)),
            cancelling: false,
            worker_deadline: None,
            worker_stalled: false,
        }
    }

    fn run(&mut self, rx: Receiver<Cmd>, repaint: impl Fn()) {
        loop {
            let cmd = match self.worker_deadline {
                Some(deadline) => {
                    rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                }
                None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
            };
            if self.shutting_down.load(Ordering::SeqCst) {
                break;
            }
            self.check_worker_deadline(Instant::now());
            match cmd {
                Ok(Cmd::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(cmd) => self.handle(cmd),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            repaint();
        }
    }

    fn check_worker_deadline(&mut self, now: Instant) {
        if !self.worker_deadline.is_some_and(|deadline| now >= deadline) {
            return;
        }
        self.worker.cancel(self.session);
        self.worker_deadline = None;
        self.worker_stalled = true;
        self.session += 1;
        self.target = None;
        self.stopped_at = None;
        self.cancelling = false;
        let mut st = self.st();
        st.phase = Phase::Error;
        st.loaded = None;
        st.backend = NO_BACKEND.into();
        st.load_time = None;
        st.diagnostics.clear();
        st.note.clear();
        st.error =
            "Mesin lokal melewati batas waktu. Tunggu mesin berhenti atau mulai ulang aplikasi."
                .into();
    }

    fn st(&self) -> MutexGuard<'_, Status> {
        self.status.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Shutdown => {}
            Cmd::Google(settings) => {
                if !self.st().phase.busy() {
                    self.google = settings;
                }
            }
            Cmd::CloudFinished { session, result } => {
                if session != self.session {
                    return;
                }
                let premature = self.capture.is_some();
                if let Some(capture) = self.capture.take() {
                    capture.cancel();
                }
                self.cloud.take();
                if premature || result.is_err() {
                    self.target = None;
                }
                self.st().recording_since = None;
                self.st().level = None;
                let outcome = match result {
                    Ok(_) if premature => {
                        Outcome::Failed("Stream berakhir sebelum rekaman selesai.".into())
                    }
                    Ok(text) if text.is_empty() => Outcome::NoSpeech,
                    Ok(text) => Outcome::Text(text),
                    Err(e) => Outcome::Failed(e),
                };
                let timing = Timing {
                    audio: self.st().captured,
                    vad: Duration::ZERO,
                    asr: self.stopped_at.map_or(Duration::ZERO, |t| t.elapsed()),
                };
                self.worker_event(transcribe::Event::Finished {
                    session,
                    outcome,
                    timing,
                });
            }
            Cmd::Toggle(source) => self.toggle(source),
            Cmd::Cancel => self.cancel(),
            Cmd::Load { engine, asr, vad } => {
                if self.worker_stalled {
                    self.st().note =
                        "Mesin lama masih berhenti; model baru belum dapat dimuat.".into();
                    return;
                }
                let mut st = self.st();
                if st.phase.busy() {
                    st.note = "Model hanya dapat diganti saat idle.".into();
                    return;
                }
                st.phase = Phase::Loading;
                st.loaded = None;
                st.backend.clear();
                st.load_time = None;
                st.diagnostics.clear();
                st.error.clear();
                st.note.clear();
                drop(st);
                self.send_job(Job::Load { engine, asr, vad });
            }
            Cmd::Settings { mic, language } => {
                self.mic = mic;
                self.language = language;
            }
            Cmd::ClearResult => {
                let mut st = self.st();
                st.result.clear();
                st.result_note.clear();
            }
            Cmd::Captured { session, end } => self.captured(session, end),
            Cmd::Worker { worker, event } if worker == self.worker_gen => self.worker_event(event),
            Cmd::Worker { .. } => {}
            Cmd::Inserted { session, outcome } => {
                if let Some(thread) = self.injecting.take() {
                    let _ = thread.join();
                }
                if session != self.session {
                    return;
                }
                self.st().result_note = match outcome {
                    Inserted::Done => String::new(),
                    Inserted::Blocked(reason) => {
                        format!("Hasil siap, tidak dimasukkan otomatis: {reason} Salin manual.")
                    }
                    Inserted::Partial { sent, total } => format!(
                        "Penyisipan terhenti setelah {sent}/{total} karakter. Periksa jendela tujuan; salin manual bila perlu."
                    ),
                };
            }
        }
    }

    fn toggle(&mut self, source: Source) {
        let phase = self.st().phase;
        match phase {
            Phase::Recording => self.stop(),
            Phase::CloudIdle | Phase::Ready | Phase::Error if self.st().loaded.is_some() => {
                if self.injecting.is_some() {
                    self.st().note = "Sedang memasukkan teks; tunggu sebentar.".into();
                } else {
                    self.start(source);
                }
            }
            Phase::Loading | Phase::Transcribing => {
                self.st().note = "Sedang sibuk; shortcut diabaikan sampai selesai.".into();
            }
            _ => {
                self.st().note = "Muat model ASR dan VAD di Pengaturan terlebih dahulu.".into();
            }
        }
    }

    fn reserve_capture(&self) -> Option<bool> {
        let mut st = self.st();
        if !matches!(st.phase, Phase::CloudIdle | Phase::Ready | Phase::Error) {
            return None;
        }
        let cloud = st.loaded.as_ref()?.0 == Engine::GoogleChirp3;
        // Serialize capture startup with the UI's engine invalidation before opening audio.
        st.phase = Phase::Recording;
        Some(cloud)
    }

    fn start(&mut self, source: Source) {
        let Some(cloud) = self.reserve_capture() else {
            return;
        };
        // Target diambil saat shortcut ditekan; tombol GUI tidak pernah mengetik ke luar.
        self.target = match source {
            Source::Hotkey => windows::foreground_target(),
            Source::Gui => None,
        };
        self.session += 1;
        self.cancelling = false;
        let session = self.session;
        let tx = self.tx.clone();
        let on_end = move |end| {
            let _ = tx.send(Cmd::Captured { session, end });
        };
        let capture = if cloud {
            let tx = self.tx.clone();
            match cloud::Session::start(self.google.clone(), self.language, move |result| {
                let _ = tx.send(Cmd::CloudFinished { session, result });
            }) {
                Ok(session) => {
                    let sink = session.audio.as_ref().unwrap().clone();
                    self.cloud = Some(session);
                    audio::start_stream(self.mic.clone(), sink, on_end)
                }
                Err(e) => Err(e),
            }
        } else {
            audio::start(self.mic.clone(), on_end)
        };
        match capture {
            Ok(capture) => {
                let mut st = self.st();
                st.phase = Phase::Recording;
                st.error.clear();
                st.note = match (source, self.target) {
                    (Source::Hotkey, Some(_)) => String::new(),
                    (Source::Hotkey, None) => {
                        "Tidak ada jendela tujuan; hasil hanya tampil di sini.".into()
                    }
                    (Source::Gui, _) => "Hasil hanya tampil di sini.".into(),
                };
                st.recording_since = Some(Instant::now());
                st.captured = Duration::ZERO;
                st.level = Some(capture.level.clone());
                drop(st);
                self.capture = Some(capture);
            }
            Err(e) => {
                self.cloud.take();
                self.session += 1;
                let mut st = self.st();
                st.phase = Phase::Error;
                st.error = e;
                st.note.clear();
            }
        }
    }

    fn stop(&mut self) {
        if let Some(c) = &self.capture {
            c.stop();
        }
        self.stopped_at = Some(Instant::now());
        self.awaiting_capture = true;
        let mut st = self.st();
        st.phase = Phase::Transcribing;
        if let Some(since) = st.recording_since.take() {
            st.captured = since.elapsed().min(audio::LIMIT);
        }
        st.level = None;
    }

    fn cancel(&mut self) {
        if self.cloud.is_some() {
            self.session += 1;
            if let Some(c) = self.capture.take() {
                c.cancel();
            }
            self.cloud.take();
            self.target = None;
            self.finish_cancel();
            let mut st = self.st();
            st.phase = Phase::CloudIdle;
            st.recording_since = None;
            st.level = None;
            return;
        }
        let phase = self.st().phase;
        match phase {
            Phase::Recording => {
                if let Some(c) = self.capture.take() {
                    c.cancel();
                }
                self.session += 1; // hasil capture lama menjadi basi
                self.target = None;
                let mut st = self.st();
                st.phase = Phase::Ready;
                st.recording_since = None;
                st.level = None;
                st.captured = Duration::ZERO;
                st.note = "Rekaman dibatalkan; audio dibuang.".into();
            }
            Phase::Transcribing => {
                // Abort callback menghentikan inferensi; tunggu worker melapor sebelum Siap.
                self.worker.cancel(self.session);
                let deadline = Instant::now() + Duration::from_secs(5);
                self.worker_deadline =
                    Some(self.worker_deadline.map_or(deadline, |d| d.min(deadline)));
                if let Some(c) = self.capture.take() {
                    c.cancel();
                }
                self.cancelling = true;
                self.target = None;
                self.st().note = "Membatalkan proses…".into();
                if self.awaiting_capture {
                    // Capture belum sampai ke worker; tidak ada inferensi yang perlu ditunggu.
                    self.session += 1;
                    self.finish_cancel();
                }
            }
            _ => {}
        }
    }

    fn finish_cancel(&mut self) {
        self.cancelling = false;
        self.worker_deadline = None;
        self.awaiting_capture = false;
        self.stopped_at = None;
        let mut st = self.st();
        st.phase = Phase::Ready;
        st.note = "Proses dibatalkan; hasil tidak dimasukkan.".into();
    }

    fn captured(&mut self, session: u64, end: CaptureEnd) {
        if session != self.session {
            return;
        }
        self.capture = None;
        self.awaiting_capture = false;
        match end {
            CaptureEnd::Done(rec) => {
                let status = self.status.clone();
                let mut st = status.lock().unwrap_or_else(|e| e.into_inner());
                if st.phase == Phase::Recording {
                    // Batas 120 detik tercapai di thread capture.
                    st.phase = Phase::Transcribing;
                    st.recording_since = None;
                    st.level = None;
                    self.stopped_at = Some(Instant::now());
                }
                st.captured = rec.captured;
                if rec.hit_limit {
                    st.note = "Batas 2 menit tercapai; rekaman diproses.".into();
                }
                drop(st);
                if let Some(cloud) = &mut self.cloud {
                    cloud.finish();
                    return;
                }
                self.send_job(Job::Transcribe {
                    session,
                    samples: rec.samples,
                    language: self.language,
                });
            }
            CaptureEnd::Cancelled => {}
            CaptureEnd::Failed(e) => {
                self.session += 1;
                self.cloud.take();
                self.target = None;
                let mut st = self.st();
                st.phase = Phase::Error;
                st.error = e;
                st.recording_since = None;
                st.level = None;
                st.note.clear();
            }
        }
    }

    fn send_job(&mut self, job: Job) {
        self.worker_deadline = Some(
            Instant::now()
                + match &job {
                    Job::Load { .. } => Duration::from_secs(120),
                    Job::Transcribe { samples, .. } => {
                        Duration::from_secs(30)
                            + Duration::from_secs_f64(samples.len() as f64 / 16_000.0 * 2.0)
                    }
                    _ => Duration::from_secs(5),
                },
        );
        if self.worker.send(job).is_err() {
            self.worker_lost("thread worker tidak aktif");
        }
    }

    /// Worker mati: ganti dengan worker baru tanpa model dan keluar dari status sibuk.
    fn worker_lost(&mut self, reason: &str) {
        self.worker_deadline = None;
        self.worker_stalled = false;
        let reason: String = reason.chars().take(120).collect();
        self.worker_gen += 1;
        self.worker = spawn_worker(&self.tx, self.worker_gen);
        self.session += 1; // hasil sesi yang sedang berjalan tidak akan datang
        if let Some(c) = self.capture.take() {
            c.cancel();
        }
        self.target = None;
        self.stopped_at = None;
        self.awaiting_capture = false;
        self.cancelling = false;
        let mut st = self.st();
        st.phase = Phase::Error;
        st.error = format!("Mesin ASR berhenti ({reason}). Muat ulang model di Pengaturan.");
        st.note.clear();
        st.recording_since = None;
        st.level = None;
        st.loaded = None;
        st.backend = NO_BACKEND.into();
        st.diagnostics.clear();
        st.load_time = None;
    }

    fn worker_event(&mut self, event: transcribe::Event) {
        if matches!(&event, transcribe::Event::Finished { session, .. } if *session != self.session)
            && !self.worker_stalled
        {
            return;
        }
        self.worker_deadline = None;
        if self.worker_stalled && !matches!(&event, transcribe::Event::Crashed(_)) {
            self.worker_stalled = false;
            let mut st = self.st();
            st.note.clear();
            st.error = "Mesin lokal sudah berhenti. Muat ulang model di Pengaturan.".into();
            return;
        }
        match event {
            transcribe::Event::Crashed(reason) => self.worker_lost(&reason),
            transcribe::Event::Loaded {
                engine,
                asr,
                vad,
                backend,
                diagnostics,
                took,
            } => {
                let mut st = self.st();
                st.phase = if engine == Engine::GoogleChirp3 {
                    Phase::CloudIdle
                } else {
                    Phase::Ready
                };
                st.backend = backend;
                st.diagnostics = diagnostics;
                st.loaded = Some((engine, asr, vad));
                st.load_time = Some(took);
                st.error.clear();
                st.note.clear();
            }
            transcribe::Event::LoadFailed(e) => {
                let mut st = self.st();
                st.phase = Phase::Error;
                st.error = e;
                st.note.clear();
            }
            transcribe::Event::Finished {
                session,
                outcome,
                timing,
            } => {
                if self.cancelling || session != self.session {
                    if self.cancelling {
                        self.finish_cancel();
                    }
                    return;
                }
                let total = self
                    .stopped_at
                    .take()
                    .map_or(timing.vad + timing.asr, |t| t.elapsed());
                let target = self.target.take();
                let mut st = self.st();
                st.phase = if st
                    .loaded
                    .as_ref()
                    .is_some_and(|x| x.0 == Engine::GoogleChirp3)
                {
                    Phase::CloudIdle
                } else {
                    Phase::Ready
                };
                st.timing = Some((timing, total));
                match outcome {
                    Outcome::Text(text) => {
                        // Simpan dulu ke RAM sebelum mencoba injection.
                        st.result = text.clone();
                        st.result_revision = st.result_revision.wrapping_add(1);
                        st.note.clear();
                        st.result_note = if target.is_some() {
                            "Memasukkan teks…".into()
                        } else {
                            String::new()
                        };
                        drop(st);
                        if let Some(target) = target {
                            let tx = self.tx.clone();
                            let shutting_down = self.shutting_down.clone();
                            self.injecting = Some(thread::spawn(move || {
                                let outcome = windows::insert_text(target, &text, &shutting_down);
                                let _ = tx.send(Cmd::Inserted { session, outcome });
                            }));
                        }
                    }
                    Outcome::NoSpeech => {
                        st.note = "Tidak ada ucapan terdeteksi.".into();
                    }
                    Outcome::Cancelled => st.note = "Proses dibatalkan.".into(),
                    Outcome::Failed(e) => {
                        st.phase = Phase::Error;
                        st.error = e;
                        st.note.clear();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::Recording;

    fn finished(session: u64) -> transcribe::Event {
        transcribe::Event::Finished {
            session,
            outcome: Outcome::Text("test".into()),
            timing: Timing::default(),
        }
    }

    #[test]
    fn capture_startup_is_busy_before_audio_setup_and_preserves_loaded_engine() {
        let (tx, _rx) = mpsc::channel();
        let mut c = Controller::new(tx);
        let loaded = (Engine::Whisper, "asr".into(), "vad".into());
        {
            let mut st = c.st();
            st.phase = Phase::Ready;
            st.loaded = Some(loaded.clone());
        }
        assert_eq!(c.reserve_capture(), Some(false));
        assert!(c.st().phase.busy());
        c.handle(Cmd::Load {
            engine: Engine::Qwen3Asr,
            asr: "qwen".into(),
            vad: "vad".into(),
        });
        assert_eq!(c.st().phase, Phase::Recording);
        assert_eq!(c.st().loaded, Some(loaded));
        assert_eq!(c.reserve_capture(), None);
    }

    #[test]
    fn engine_invalidation_before_capture_startup_prevents_starting_old_backend() {
        let (tx, _rx) = mpsc::channel();
        let c = Controller::new(tx);
        {
            let mut st = c.st();
            st.phase = Phase::CloudIdle;
            st.loaded = Some((Engine::GoogleChirp3, PathBuf::new(), PathBuf::new()));
        }
        // The UI wins the status lock after toggle's idle check, before start reserves it.
        c.st().loaded = None;
        assert_eq!(c.reserve_capture(), None);
        assert_eq!(c.st().phase, Phase::CloudIdle);
        assert_eq!(c.session, 0);
    }

    #[test]
    fn timeout_blocks_reload_and_discards_late_result() {
        let (tx, _rx) = mpsc::channel();
        let mut c = Controller::new(tx);
        c.session = 3;
        c.st().phase = Phase::Transcribing;
        let deadline = Instant::now();
        c.worker_deadline = Some(deadline);
        c.check_worker_deadline(deadline);
        assert!(c.worker_stalled);
        assert_eq!(c.st().phase, Phase::Error);
        assert!(c.st().loaded.is_none());
        c.handle(load_cmd());
        assert!(c.worker_deadline.is_none());
        assert_eq!(c.worker_gen, 0);
        c.worker_event(finished(3));
        assert!(!c.worker_stalled);
        assert!(c.st().result.is_empty());
        assert!(c.injecting.is_none());
        c.handle(load_cmd());
        assert_eq!(c.st().phase, Phase::Loading);
    }

    #[test]
    fn stale_result_does_not_finish_cancellation_or_clear_deadline() {
        let (tx, _rx) = mpsc::channel();
        let mut c = Controller::new(tx);
        c.session = 8;
        c.cancelling = true;
        c.st().phase = Phase::Transcribing;
        let deadline = Instant::now() + Duration::from_secs(5);
        c.worker_deadline = Some(deadline);
        c.worker_event(finished(7));
        assert!(c.cancelling);
        assert_eq!(c.worker_deadline, Some(deadline));
        c.worker_event(finished(8));
        assert!(!c.cancelling);
        assert!(c.worker_deadline.is_none());
        assert!(c.st().result.is_empty());
    }

    #[test]
    fn cancel_before_capture_ignores_late_audio() {
        let (tx, _rx) = mpsc::channel();
        let mut c = Controller::new(tx);
        c.session = 5;
        c.st().phase = Phase::Transcribing;
        c.awaiting_capture = true;
        c.cancel();
        c.captured(
            5,
            CaptureEnd::Done(Recording {
                samples: vec![0.0; 16000],
                captured: Duration::from_secs(1),
                hit_limit: false,
            }),
        );
        assert_eq!(c.st().phase, Phase::Ready);
        assert!(c.worker_deadline.is_none());
        assert!(c.st().result.is_empty());
    }

    #[test]
    fn cloud_selection_skips_missing_checkpoints_and_stale_results() {
        let (tx, rx) = mpsc::channel();
        let mut c = Controller::new(tx);
        c.handle(Cmd::Load {
            engine: Engine::GoogleChirp3,
            asr: "absent-asr.bin".into(),
            vad: "absent-vad.bin".into(),
        });
        c.handle(recv(&rx));
        assert_eq!(c.st().phase, Phase::CloudIdle);
        assert_eq!(c.st().loaded.as_ref().unwrap().0, Engine::GoogleChirp3);
        c.session = 5;
        c.handle(Cmd::CloudFinished {
            session: 4,
            result: Ok("stale result".into()),
        });
        assert!(c.st().result.is_empty());
        assert!(c.injecting.is_none());
        c.handle(Cmd::CloudFinished {
            session: 5,
            result: Ok("current result".into()),
        });
        assert_eq!(c.st().result, "current result");
        assert!(c.injecting.is_none());
    }

    #[test]
    fn dropping_handle_stops_threads_despite_remaining_senders() {
        let repaint = Arc::new(());
        let weak_repaint = Arc::downgrade(&repaint);
        let handle = spawn(move || {
            let _ = &repaint;
        });
        let status = Arc::downgrade(&handle.status);
        let sender = handle.tx.clone();
        drop(handle);
        assert!(sender.send(Cmd::ClearResult).is_err());
        assert!(status.upgrade().is_none());
        assert!(weak_repaint.upgrade().is_none());
    }

    fn recv(rx: &Receiver<Cmd>) -> Cmd {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("event worker")
    }

    fn load_cmd() -> Cmd {
        Cmd::Load {
            engine: Engine::Whisper,
            asr: PathBuf::from("tidak-ada.bin"),
            vad: PathBuf::from("tidak-ada-vad.bin"),
        }
    }

    #[test]
    fn failed_reload_clears_previous_model_status() {
        let (tx, rx) = mpsc::channel();
        let mut c = Controller::new(tx);
        {
            let mut st = c.st();
            st.phase = Phase::Ready;
            st.loaded = Some((Engine::Whisper, "a".into(), "b".into()));
            st.backend = "GPU CUDA0".into();
            st.load_time = Some(Duration::from_secs(1));
        }
        c.handle(load_cmd());
        assert!(c.st().loaded.is_none());
        assert!(c.st().backend.is_empty());
        assert!(c.st().load_time.is_none());
        c.handle(recv(&rx));
        assert_eq!(c.st().phase, Phase::Error);
        assert!(c.st().loaded.is_none());
        assert!(!c.st().error.is_empty());
    }

    /// Bunuh worker aktif dan kembalikan event Crashed-nya tanpa diproses.
    fn kill_worker(c: &Controller, rx: &Receiver<Cmd>) -> Cmd {
        c.worker.send(Job::Panic).unwrap();
        let cmd = recv(rx);
        assert!(matches!(
            cmd,
            Cmd::Worker {
                event: transcribe::Event::Crashed(_),
                ..
            }
        ));
        cmd
    }

    #[test]
    fn crash_while_transcribing_leaves_busy_state() {
        let (tx, rx) = mpsc::channel();
        let mut c = Controller::new(tx);
        {
            let mut st = c.st();
            st.phase = Phase::Transcribing;
            st.loaded = Some((Engine::Whisper, "a".into(), "b".into()));
        }
        c.session = 3;
        c.stopped_at = Some(Instant::now());
        let crashed = kill_worker(&c, &rx);
        c.handle(crashed);
        {
            let st = c.st();
            assert_eq!(st.phase, Phase::Error);
            assert!(st.loaded.is_none());
            assert!(st.error.contains("uji panic worker"), "{}", st.error);
        }
        assert_eq!(c.worker_gen, 1);
        assert!(c.session > 3 && c.stopped_at.is_none());

        // Worker pengganti aktif: Load dijawab, bukan menggantung di Loading.
        c.handle(load_cmd());
        assert_eq!(c.st().phase, Phase::Loading);
        c.handle(recv(&rx));
        let st = c.st();
        assert_eq!(st.phase, Phase::Error);
        assert!(st.error.contains("VAD"), "{}", st.error);
    }

    #[test]
    fn send_to_dead_worker_is_reported() {
        let (tx, rx) = mpsc::channel();
        let mut c = Controller::new(tx);
        let stale = kill_worker(&c, &rx);

        // Load sebelum event Crashed diproses: pengiriman gagal, status tidak tersangkut.
        c.handle(load_cmd());
        assert_eq!(c.st().phase, Phase::Error);
        assert!(!c.st().phase.busy());
        assert_eq!(c.worker_gen, 1);
        // Event Crashed dari worker lama tidak memicu penggantian kedua.
        c.handle(stale);
        assert_eq!(c.worker_gen, 1);

        // Hasil rekaman dikirim ke worker yang ternyata sudah mati.
        let _ = kill_worker(&c, &rx);
        c.st().phase = Phase::Transcribing;
        c.awaiting_capture = true;
        c.handle(Cmd::Captured {
            session: c.session,
            end: CaptureEnd::Done(Recording {
                samples: vec![0.0; 16_000],
                captured: Duration::from_secs(1),
                hit_limit: false,
            }),
        });
        assert_eq!(c.st().phase, Phase::Error);
        assert!(!c.awaiting_capture);
        assert_eq!(c.worker_gen, 2);
    }
}

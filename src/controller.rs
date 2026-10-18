//! Koordinasi state di thread sendiri agar hotkey/tray tetap diproses saat jendela tersembunyi.

use crate::audio::{self, Capture, CaptureEnd};
use crate::config::{Engine, Language};
use crate::transcribe::{self, Job, Outcome, Timing, Worker};
use crate::windows::{self, Inserted, Target};
use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    MissingModel,
    Loading,
    Ready,
    Recording,
    Transcribing,
    Error,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::MissingModel => "Model belum dimuat",
            Self::Loading => "Memuat model",
            Self::Ready => "Siap",
            Self::Recording => "Merekam",
            Self::Transcribing => "Memproses",
            Self::Error => "Periksa pengaturan",
        }
    }

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
    pub result_note: String,
    pub backend: String,
    pub diagnostics: Vec<String>,
    /// Mesin, model ASR (file Whisper atau folder Qwen), dan model VAD yang aktif.
    pub loaded: Option<(Engine, PathBuf, PathBuf)>,
    pub load_time: Option<Duration>,
    pub timing: Option<(Timing, Duration)>,
}

pub enum Cmd {
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

#[derive(Clone)]
pub struct Handle {
    pub tx: Sender<Cmd>,
    pub status: Arc<Mutex<Status>>,
}

impl Handle {
    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    pub fn status(&self) -> MutexGuard<'_, Status> {
        self.status.lock().unwrap_or_else(|e| e.into_inner())
    }
}

pub fn spawn(repaint: impl Fn() + Send + 'static) -> Handle {
    let (tx, rx) = mpsc::channel();
    let mut controller = Controller::new(tx.clone());
    let handle = Handle {
        tx,
        status: controller.status.clone(),
    };
    thread::Builder::new()
        .name("fi-whisper-controller".into())
        .spawn(move || controller.run(rx, repaint))
        .expect("thread controller");
    handle
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
    injecting: bool,
    cancelling: bool,
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
            result_note: String::new(),
            backend: NO_BACKEND.into(),
            diagnostics: Vec::new(),
            loaded: None,
            load_time: None,
            timing: None,
        }));
        Self {
            worker: spawn_worker(&tx, 0),
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
            injecting: false,
            cancelling: false,
        }
    }

    fn run(&mut self, rx: Receiver<Cmd>, repaint: impl Fn()) {
        while let Ok(cmd) = rx.recv() {
            self.handle(cmd);
            repaint();
        }
        if let Some(c) = self.capture.take() {
            c.cancel();
        }
    }

    fn st(&self) -> MutexGuard<'_, Status> {
        self.status.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Toggle(source) => self.toggle(source),
            Cmd::Cancel => self.cancel(),
            Cmd::Load { engine, asr, vad } => {
                let mut st = self.st();
                if st.phase.busy() {
                    st.note = "Model hanya dapat diganti saat idle.".into();
                    return;
                }
                st.phase = Phase::Loading;
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
                self.injecting = false;
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
            Phase::Ready | Phase::Error if self.st().loaded.is_some() => {
                if self.injecting {
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

    fn start(&mut self, source: Source) {
        // Target diambil saat shortcut ditekan; tombol GUI tidak pernah mengetik ke luar.
        self.target = match source {
            Source::Hotkey => windows::foreground_target(),
            Source::Gui => None,
        };
        self.session += 1;
        self.cancelling = false;
        let session = self.session;
        let tx = self.tx.clone();
        match audio::start(self.mic.clone(), move |end| {
            let _ = tx.send(Cmd::Captured { session, end });
        }) {
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
                self.send_job(Job::Transcribe {
                    session,
                    samples: rec.samples,
                    language: self.language,
                });
            }
            CaptureEnd::Cancelled => {}
            CaptureEnd::Failed(e) => {
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
        if self.worker.send(job).is_err() {
            self.worker_lost("thread worker tidak aktif");
        }
    }

    /// Worker mati: ganti dengan worker baru tanpa model dan keluar dari status sibuk.
    fn worker_lost(&mut self, reason: &str) {
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
                st.phase = Phase::Ready;
                st.backend = backend;
                st.diagnostics = diagnostics;
                st.loaded = Some((engine, asr, vad));
                st.load_time = Some(took);
                st.error.clear();
                st.note.clear();
            }
            transcribe::Event::LoadFailed(e) => {
                let mut st = self.st();
                st.phase = if st.loaded.is_some() {
                    Phase::Ready
                } else {
                    Phase::Error
                };
                st.error = if st.loaded.is_some() {
                    format!("{e} Model sebelumnya tetap dipakai.")
                } else {
                    e
                };
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
                st.phase = Phase::Ready;
                st.timing = Some((timing, total));
                match outcome {
                    Outcome::Text(text) => {
                        // Simpan dulu ke RAM sebelum mencoba injection.
                        st.result = text.clone();
                        st.note.clear();
                        st.result_note = if target.is_some() {
                            "Memasukkan teks…".into()
                        } else {
                            String::new()
                        };
                        drop(st);
                        if let Some(target) = target {
                            self.injecting = true;
                            let tx = self.tx.clone();
                            thread::spawn(move || {
                                let outcome = windows::insert_text(target, &text);
                                let _ = tx.send(Cmd::Inserted { session, outcome });
                            });
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

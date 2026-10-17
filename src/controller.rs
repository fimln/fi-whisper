//! Koordinasi state di thread sendiri agar hotkey/tray tetap diproses saat jendela tersembunyi.

use crate::audio::{self, Capture, CaptureEnd};
use crate::config::Language;
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
    pub loaded: Option<(PathBuf, PathBuf)>,
    pub load_time: Option<Duration>,
    pub timing: Option<(Timing, Duration)>,
}

pub enum Cmd {
    Toggle(Source),
    Cancel,
    Load {
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
    Worker(transcribe::Event),
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
    let status = Arc::new(Mutex::new(Status {
        phase: Phase::MissingModel,
        error: String::new(),
        note: String::new(),
        recording_since: None,
        captured: Duration::ZERO,
        level: None,
        result: String::new(),
        result_note: String::new(),
        backend: "Belum diinisialisasi".into(),
        diagnostics: Vec::new(),
        loaded: None,
        load_time: None,
        timing: None,
    }));
    let handle = Handle {
        tx: tx.clone(),
        status: status.clone(),
    };
    let worker_tx = tx.clone();
    let worker = Worker::spawn(move |e| {
        let _ = worker_tx.send(Cmd::Worker(e));
    });
    let mut controller = Controller {
        tx,
        status,
        worker,
        mic: None,
        language: Language::Indonesian,
        session: 0,
        capture: None,
        target: None,
        stopped_at: None,
        awaiting_capture: false,
        injecting: false,
        cancelling: false,
    };
    thread::Builder::new()
        .name("fi-whisper-controller".into())
        .spawn(move || controller.run(rx, repaint))
        .expect("thread controller");
    handle
}

struct Controller {
    tx: Sender<Cmd>,
    status: Arc<Mutex<Status>>,
    worker: Worker,
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
            Cmd::Load { asr, vad } => {
                let mut st = self.st();
                if st.phase.busy() {
                    st.note = "Model hanya dapat diganti saat idle.".into();
                    return;
                }
                st.phase = Phase::Loading;
                st.error.clear();
                st.note = "Memuat model…".into();
                drop(st);
                self.worker.send(Job::Load { asr, vad });
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
            Cmd::Worker(event) => self.worker_event(event),
            Cmd::Inserted { session, outcome } => {
                self.injecting = false;
                if session != self.session {
                    return;
                }
                self.st().result_note = match outcome {
                    Inserted::Done => "Teks dikirim ke jendela tujuan.".into(),
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
                    (Source::Hotkey, Some(_)) => {
                        "Merekam. Hasil akan diketik ke jendela yang tadi fokus.".into()
                    }
                    (Source::Hotkey, None) => {
                        "Merekam. Tidak ada jendela tujuan; hasil hanya tampil di sini.".into()
                    }
                    (Source::Gui, _) => "Merekam dari tombol. Hasil hanya tampil di panel.".into(),
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
        st.note = "Memproses audio…".into();
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
                st.note = if rec.hit_limit {
                    "Batas 2 menit tercapai; rekaman dihentikan dan diproses.".into()
                } else {
                    "Mendeteksi ucapan dan mentranskripsi…".into()
                };
                drop(st);
                self.worker.send(Job::Transcribe {
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

    fn worker_event(&mut self, event: transcribe::Event) {
        match event {
            transcribe::Event::Loaded {
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
                st.loaded = Some((asr, vad));
                st.load_time = Some(took);
                st.error.clear();
                st.note = format!("Model dimuat dalam {:.1} dtk.", took.as_secs_f32());
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
                            "Hasil hanya ditampilkan di sini.".into()
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
                        st.note =
                            "Tidak ada ucapan terdeteksi; tidak ada teks yang dimasukkan.".into();
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

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod audio;
mod cloud;
mod config;
mod controller;
mod effects;
mod theme;
mod transcribe;
mod ui;
#[cfg(test)]
mod ui_tests;
mod windows;
use config::{Config, Engine, Language};
use controller::{Cmd, Handle, Phase, Source};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
static APP_HANDLE: OnceLock<rune::AppHandle<App>> = OnceLock::new();
/// Kondisi lintas thread untuk handler tray/hotkey.
struct Window {
    hwnd: AtomicIsize,
    quitting: AtomicBool,
    open_requested: AtomicBool,
}

struct App {
    core: Handle,
    window: Arc<Window>,
    config: Config,
    config_path: Option<PathBuf>,
    config_error: String,
    engine: Engine,
    asr: Option<PathBuf>,
    qwen: Option<PathBuf>,
    qwen17: Option<PathBuf>,
    vad: Option<PathBuf>,
    model_valid: [bool; 4],
    mics: Vec<audio::Mic>,
    default_mic: Option<String>,
    mic_error: String,
    shortcut_input: String,
    shortcut_error: String,
    hotkeys: Option<windows::Hotkeys>,
    tray: Option<windows::Tray>,
    tray_error: String,
    _activation: Option<windows::Activation>,
    shortcut_edit: rune::TextEdit,
    project_edit: rune::TextEdit,
    limit_edit: rune::TextEdit,
    menu: Option<String>,
    adc_open: bool,
    diagnostics_open: bool,
    level: f32,
    level_time: std::time::Instant,
    result_revision: u64,
    result_flash: f64,
    copy_flash: f64,
    now: f64,
    logo: rune::Pixels,
    icons: theme::Icons,
    cloud_usage: Option<(std::time::Instant, String, Result<u64, String>)>,
    preview: bool,
}
impl App {
    fn new() -> Self {
        Self::create(rune::is_headless())
    }
    fn create(preview: bool) -> Self {
        let window = Arc::new(Window {
            hwnd: AtomicIsize::new(0),
            quitting: AtomicBool::new(false),
            open_requested: AtomicBool::new(false),
        });
        let core = controller::spawn(|| {
            if let Some(h) = APP_HANDLE.get() {
                h.wake();
            }
        });
        let config_path = config::config_path();
        let (config, config_error) = match config_path
            .as_deref()
            .filter(|_| !preview)
            .map(config::load)
        {
            Some(Ok(Some(c))) => (c, String::new()),
            Some(Ok(None)) => (Config::default(), String::new()),
            Some(Err(e)) => (Config::default(), e),
            None if preview => (Config::default(), String::new()),
            None => (
                Config::default(),
                "LOCALAPPDATA tidak tersedia; konfigurasi tidak disimpan.".into(),
            ),
        };

        // Handler hotkey/tray berjalan di message loop thread utama, juga saat jendela tersembunyi.
        let tx = core.tx.clone();
        let hotkeys = if preview {
            Err(String::new())
        } else {
            windows::Hotkeys::new(move || {
                let _ = tx.send(Cmd::Toggle(Source::Hotkey));
            })
        };
        let (tx, win) = (core.tx.clone(), window.clone());
        let tray = if preview {
            Err(String::new())
        } else {
            windows::create_tray(move |action| match action {
                windows::TrayAction::Open => {
                    win.open_requested.store(true, Ordering::SeqCst);
                    if let Some(h) = APP_HANDLE.get() {
                        h.open();
                    }
                    if let Some(h) = APP_HANDLE.get() {
                        h.wake();
                    }
                }
                windows::TrayAction::Cancel => {
                    let _ = tx.send(Cmd::Cancel);
                }
                windows::TrayAction::Quit => {
                    win.quitting.store(true, Ordering::SeqCst);
                    // Tampilkan jendela agar event loop memproses penutupan.
                    if let Some(h) = APP_HANDLE.get() {
                        h.quit();
                    }
                    if let Some(h) = APP_HANDLE.get() {
                        h.wake();
                    }
                }
            })
        };
        let mut app = Self {
            _activation: if preview {
                None
            } else {
                let win = window.clone();
                windows::Activation::new(move || {
                    win.open_requested.store(true, Ordering::SeqCst);
                    if let Some(h) = APP_HANDLE.get() {
                        h.open();
                    }

                    if let Some(h) = APP_HANDLE.get() {
                        h.wake();
                    }
                })
            },
            core,
            cloud_usage: None,
            logo: theme::logo(),
            icons: theme::Icons::new(),
            window,

            engine: config.engine,
            asr: config.asr_model.clone().or_else(|| {
                (config.engine != Engine::GoogleChirp3)
                    .then(|| config::default_model(config::ASR_DIR, config::ASR_FILE))
                    .flatten()
            }),
            qwen: config.qwen_model.clone().or_else(|| {
                (config.engine != Engine::GoogleChirp3)
                    .then(|| config::default_model_dir(config::QWEN_DIR))
                    .flatten()
            }),
            qwen17: config.qwen17_model.clone().or_else(|| {
                (config.engine != Engine::GoogleChirp3)
                    .then(|| config::default_model_dir(config::QWEN17_DIR))
                    .flatten()
            }),
            vad: config.vad_model.clone().or_else(|| {
                (config.engine != Engine::GoogleChirp3)
                    .then(|| config::default_model(config::VAD_DIR, config::VAD_FILE))
                    .flatten()
            }),
            shortcut_input: config.shortcut.clone(),
            model_valid: [false; 4],
            config,
            config_path,
            config_error,
            mics: Vec::new(),
            default_mic: None,
            mic_error: String::new(),
            shortcut_error: String::new(),
            hotkeys: None,
            tray: None,
            tray_error: String::new(),
            level: 0.0,
            level_time: std::time::Instant::now(),
            result_revision: 0,
            result_flash: -10.0,
            copy_flash: -10.0,
            now: 0.0,
            shortcut_edit: rune::TextEdit::new(""),
            project_edit: rune::TextEdit::new(""),
            limit_edit: rune::TextEdit::new(""),
            menu: None,
            adc_open: false,
            diagnostics_open: false,
            preview,
        };
        match hotkeys {
            Ok(mut h) => {
                if let Err(e) = h.set(&app.config.shortcut) {
                    app.shortcut_error = e;
                }
                app.hotkeys = Some(h);
            }
            Err(e) => app.shortcut_error = e,
        }
        match tray {
            Ok(t) => app.tray = Some(t),
            Err(e) => app.tray_error = e,
        }
        app.shortcut_edit = rune::TextEdit::new(&app.shortcut_input);
        app.project_edit = rune::TextEdit::new(&app.config.google.project);
        app.limit_edit = rune::TextEdit::new(app.config.google.monthly_limit_minutes.to_string());
        if preview {
            app.hotkeys = None;
            app.tray = None;
            app._activation = None;
            return app;
        }
        app.refresh_mics();
        if app.engine != Engine::GoogleChirp3 {
            app.refresh_models();
        }
        app.push_settings();
        if app.engine == Engine::GoogleChirp3 {
            app.activate_cloud();
        } else if let (Some(asr), Some(vad)) = (app.selected_asr(), app.vad.clone()) {
            app.core.send(Cmd::Load {
                engine: app.engine,
                asr,
                vad,
            });
        }
        app
    }

    /// Model ASR untuk mesin yang sedang dipilih.
    fn selected_asr(&self) -> Option<PathBuf> {
        match self.engine {
            Engine::Whisper => self.asr.clone(),
            Engine::Qwen3Asr => self.qwen.clone(),
            Engine::Qwen3Asr17 => self.qwen17.clone(),
            Engine::GoogleChirp3 => None,
        }
    }

    fn asr_path_mut(&mut self, engine: Engine) -> (&mut Option<PathBuf>, &mut bool) {
        let path = match engine {
            Engine::Whisper => &mut self.asr,
            Engine::Qwen3Asr => &mut self.qwen,
            Engine::Qwen3Asr17 => &mut self.qwen17,
            Engine::GoogleChirp3 => unreachable!("Cloud tidak memiliki path model"),
        };
        (path, &mut self.model_valid[engine as usize])
    }

    fn refresh_models(&mut self) {
        if self.asr.is_none() {
            self.asr = config::default_model(config::ASR_DIR, config::ASR_FILE);
        }
        if self.vad.is_none() {
            self.vad = config::default_model(config::VAD_DIR, config::VAD_FILE);
        }
        for (path, dir) in [
            (&mut self.qwen, config::QWEN_DIR),
            (&mut self.qwen17, config::QWEN17_DIR),
        ] {
            if path.is_none() {
                *path = config::default_model_dir(dir);
            }
        }
        self.model_valid = [
            self.asr.as_deref().is_some_and(Path::is_file),
            self.qwen
                .as_deref()
                .is_some_and(|p| config::validate_qwen_dir(p).is_ok()),
            self.qwen17
                .as_deref()
                .is_some_and(|p| config::validate_qwen_dir(p).is_ok()),
            self.vad.as_deref().is_some_and(Path::is_file),
        ];
    }

    fn save_config(&mut self) {
        if self.preview || !self.config_error.is_empty() {
            return;
        }
        if let Some(path) = &self.config_path
            && let Err(e) = config::save(path, &self.config)
        {
            self.config_error = format!("Konfigurasi gagal disimpan: {e}");
        }
    }

    fn push_settings(&self) {
        if let Some(tray) = &self.tray {
            tray.set_english(self.config.language == Language::English);
        }
        self.core.send(Cmd::Settings {
            mic: self.config.microphone.clone(),
            language: self.config.language,
        });
    }

    fn refresh_mics(&mut self) {
        match audio::list_inputs() {
            Ok((mics, default)) => {
                self.mic_error = match &self.config.microphone {
                    Some(id) if !mics.iter().any(|m| &m.id == id) => {
                        "Mikrofon pilihan tidak ditemukan. Pilih ulang atau gunakan default sistem."
                            .into()
                    }
                    _ => String::new(),
                };
                self.mics = mics;
                self.default_mic = default;
            }
            Err(e) => self.mic_error = e,
        }
    }

    fn shortcut(&self) -> Option<&str> {
        self.hotkeys
            .as_ref()
            .and_then(|h| h.active.as_ref())
            .map(|(_, s)| s.as_str())
    }

    fn apply_shortcut(&mut self) {
        let Some(hotkeys) = self.hotkeys.as_mut() else {
            return;
        };
        match hotkeys.set(&self.shortcut_input) {
            Ok(()) => {
                self.shortcut_error.clear();
                self.config.shortcut = self.shortcut_input.trim().to_owned();
                self.save_config();
            }
            // Shortcut lama tetap terdaftar.
            Err(e) => self.shortcut_error = e,
        }
    }

    /// Satu baris dua kolom; kedua kartu disamakan tingginya memakai ukuran frame sebelumnya.
    fn activate_cloud(&self) {
        if self.preview {
            return;
        }
        self.core.send(Cmd::Google(self.config.google.clone()));
        self.core.send(Cmd::Load {
            engine: Engine::GoogleChirp3,
            asr: PathBuf::new(),
            vad: PathBuf::new(),
        });
    }

    fn t<'a>(&self, id: &'a str, en: &'a str) -> &'a str {
        if self.config.language == Language::English {
            en
        } else {
            id
        }
    }
    fn load_model(&mut self) {
        if self.preview || self.core.status().phase.busy() {
            return;
        }
        if self.engine == Engine::GoogleChirp3 {
            self.config.engine = self.engine;
            self.save_config();
            self.activate_cloud();
            return;
        }
        self.refresh_models();
        if let (Some(asr), Some(vad)) = (self.selected_asr(), self.vad.clone()) {
            self.config.engine = self.engine;
            match self.engine {
                Engine::Whisper => self.config.asr_model = Some(asr.clone()),
                Engine::Qwen3Asr => self.config.qwen_model = Some(asr.clone()),
                Engine::Qwen3Asr17 => self.config.qwen17_model = Some(asr.clone()),
                _ => {}
            }
            self.config.vad_model = Some(vad.clone());
            self.save_config();
            self.core.send(Cmd::Load {
                engine: self.engine,
                asr,
                vad,
            });
        }
    }
}
fn poll(s: &mut App, now: f64, tx: &mut rune::Tx) -> bool {
    s.now = now;
    if s.window.quitting.load(Ordering::SeqCst) {
        tx.quit();
    }
    s.window.open_requested.store(false, Ordering::SeqCst);
    let st = s.core.status();
    let busy = st.phase.busy();
    let revision = st.result_revision;
    let level = st
        .level
        .as_ref()
        .map_or(0.0, |level| audio::take_level(level));
    drop(st);
    let dt = s.level_time.elapsed().as_secs_f32();
    s.level_time = std::time::Instant::now();
    s.level = level.max(s.level * (-dt / 0.3).exp());
    if revision != s.result_revision {
        s.result_revision = revision;
        s.result_flash = now;
    }
    if s.engine == Engine::GoogleChirp3
        && !s.preview
        && s.cloud_usage.as_ref().is_none_or(|(at, project, _)| {
            at.elapsed() >= Duration::from_secs(1) || project != &s.config.google.project
        })
    {
        s.cloud_usage = Some((
            std::time::Instant::now(),
            s.config.google.project.clone(),
            cloud::usage_seconds(&s.config.google.project),
        ));
    }
    if busy || now - s.result_flash < 0.8 || now - s.copy_flash < 0.6 {
        tx.wake_after(Duration::from_millis(33));
    }
    true
}
fn main() {
    let _instance = if rune::is_headless() {
        None
    } else {
        let Some(guard) = windows::single_instance() else {
            return;
        };
        Some(guard)
    };
    let _ = rune::set_app_icon(include_bytes!("../assets/logo.png").to_vec());
    let spec: rune::AppSpec<App> = rune::AppSpec {
        scenes: &[
            ("cloud", |s| {
                s.engine = Engine::GoogleChirp3;
                s.config.engine = s.engine;
                let mut st = s.core.status();
                st.phase = Phase::CloudIdle;
                st.loaded = Some((Engine::GoogleChirp3, PathBuf::new(), PathBuf::new()));
            }),
            ("recording", |s| {
                s.core.status().phase = Phase::Recording;
                s.level = 0.65;
            }),
            ("processing", |s| {
                s.core.status().phase = Phase::Transcribing;
            }),
            ("english", |s| {
                s.config.language = Language::English;
            }),
        ],
        fonts: &[
            include_bytes!("../fonts/Inter-Regular.ttf"),
            include_bytes!("../fonts/Inter-Medium.ttf"),
            include_bytes!("../fonts/Inter-SemiBold.ttf"),
            include_bytes!("../fonts/JetBrainsMono-Regular.ttf"),
        ],
        families: ("Inter", "JetBrains Mono"),
        min_size: Some((560.0, 600.0)),
        poll: Some(poll),
        ..rune::AppSpec::new("fi-whisper", (700.0, 680.0), App::new, ui::view)
    };
    let app = rune::App::new(spec)
        .keep_running()
        .window_options(rune::WindowOptions {
            borderless: true,
            ..Default::default()
        })
        .compositor(effects::Glow::default())
        .on_close(|s, tx| {
            if !s.config.close_to_tray || s.tray.is_none() {
                tx.quit();
            } else if !s.core.status().phase.busy() {
                windows::trim_idle_memory();
            }
        })
        .on_window(|w| {
            let _ = w.with_raw_handle(|raw| {
                if let raw_window_handle::RawWindowHandle::Win32(v) = raw.as_raw() {
                    windows::configure_window(v.hwnd.get() as _);
                }
            });
            if let Some(h) = APP_HANDLE.get() {
                let w = w.clone();
                h.update(move |s, _| {
                    let _ = w.with_raw_handle(|raw| {
                        if let raw_window_handle::RawWindowHandle::Win32(v) = raw.as_raw() {
                            s.window.hwnd.store(v.hwnd.get(), Ordering::SeqCst);
                        }
                    });
                });
            }
        });
    let app = if std::env::args_os().any(|x| x == "--tray") {
        app.windowless()
    } else {
        app
    };
    let _ = APP_HANDLE.set(app.handle());
    app.run();
}
fn phase_label(phase: Phase, english: bool) -> &'static str {
    let t = |id, en| if english { en } else { id };
    match phase {
        Phase::CloudIdle | Phase::Ready => t("Siap", "Ready"),
        Phase::Recording => t("Merekam", "Recording"),
        Phase::Loading => t("Memuat", "Loading"),
        Phase::Transcribing => t("Memproses", "Processing"),
        Phase::MissingModel => t("Belum siap", "Not ready"),
        Phase::Error => t("Galat", "Error"),
    }
}

fn message(text: &str, english: bool) -> String {
    if !english {
        return text.to_owned();
    }
    let mut text = text.to_owned();
    for (id, en) in [
        ("Hasil hanya tampil di sini.", "Results appear only here."),
        (
            "Tidak ada jendela tujuan; hasil hanya tampil di sini.",
            "No target window; results appear only here.",
        ),
        ("Memasukkan teks…", "Inserting text…"),
        ("Tidak ada ucapan terdeteksi.", "No speech detected."),
        ("Proses dibatalkan.", "Processing cancelled."),
        (
            "Rekaman dibatalkan; audio dibuang.",
            "Recording cancelled; audio discarded.",
        ),
        ("Membatalkan proses…", "Cancelling…"),
        (
            "Proses dibatalkan; hasil tidak dimasukkan.",
            "Processing cancelled; result was not inserted.",
        ),
        (
            "Batas 2 menit tercapai; rekaman diproses.",
            "Two-minute limit reached; processing recording.",
        ),
        (
            "Sedang memasukkan teks; tunggu sebentar.",
            "Inserting text; please wait.",
        ),
        (
            "Sedang sibuk; shortcut diabaikan sampai selesai.",
            "Busy; shortcut ignored until finished.",
        ),
        (
            "Muat model ASR dan VAD di Pengaturan terlebih dahulu.",
            "Load ASR and VAD models in Settings first.",
        ),
        (
            "Model hanya dapat diganti saat idle.",
            "Models can only be changed while idle.",
        ),
        (
            "Hasil siap, tidak dimasukkan otomatis:",
            "Result ready, not inserted automatically:",
        ),
        ("Salin manual.", "Copy manually."),
        (
            "Project ID Google Cloud tidak valid.",
            "Invalid Google Cloud Project ID.",
        ),
        (
            "Region Chirp 3 harus us atau eu.",
            "Chirp 3 region must be us or eu.",
        ),
        ("ADC belum tersedia. Jalankan", "ADC unavailable. Run"),
        (
            "Login ADC gagal. Jalankan ulang",
            "ADC login failed. Run again:",
        ),
        (
            "Batas bulanan tercapai atau sisa kurang dari 2 menit.",
            "Monthly limit reached or less than two minutes remaining.",
        ),
        (
            "Google Cloud tidak terhubung. Periksa internet.",
            "Cannot connect to Google Cloud. Check your internet.",
        ),
        (
            "Mikrofon pilihan tidak ditemukan. Pilih ulang atau gunakan default sistem.",
            "Selected microphone not found. Select again or use the system default.",
        ),
        (
            "Konfigurasi gagal disimpan:",
            "Could not save configuration:",
        ),
        ("Belum diinisialisasi", "Not initialized"),
    ] {
        text = text.replace(id, en);
    }
    text
}

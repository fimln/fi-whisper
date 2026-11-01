#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod cloud;
mod config;
mod controller;
mod theme;
mod transcribe;
mod windows;

use config::{Config, Engine, Language};
use controller::{Cmd, Handle, Phase, Source};
use eframe::egui::{self, Align, Layout, RichText, Ui, vec2};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::Duration;
use theme::*;

/// Kondisi lintas thread untuk handler tray/hotkey.
struct Window {
    hwnd: AtomicIsize,
    quitting: AtomicBool,
    open_requested: AtomicBool,
}

struct App {
    core: Handle,
    cloud_usage: Option<(std::time::Instant, String, Result<u64, String>)>,
    logo: egui::TextureHandle,
    icons: theme::Icons,
    window: Arc<Window>,
    startup: Option<windows::StartupWindow>,
    start_in_tray: bool,
    _activation: Option<windows::Activation>,
    config: Config,
    config_path: Option<PathBuf>,
    /// Error config rusak; selama ada, config tidak ditulis agar file lama tidak tertimpa.
    config_error: String,
    /// Mesin yang dipilih di kartu Model (disimpan ke config saat Muat model).
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
    level: f32,
    level_time: std::time::Instant,
    result_revision: u64,
    result_flash: Option<std::time::Instant>,
    copy_flash: Option<std::time::Instant>,
    form_changed: Option<std::time::Instant>,
    /// Tinggi alami isi kartu per baris/kolom dari frame sebelumnya, agar kartu sebaris sama tinggi.
    rows: [[f32; 2]; 2],
    /// Tinggi kosong di bawah kartu (frame sebelumnya) yang ditambahkan ke baris bawah.
    extra: f32,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        let window = Arc::new(Window {
            hwnd: AtomicIsize::new(0),
            quitting: AtomicBool::new(false),
            open_requested: AtomicBool::new(false),
        });
        if let Ok(handle) = cc.window_handle()
            && let RawWindowHandle::Win32(h) = handle.as_raw()
        {
            window.hwnd.store(h.hwnd.get(), Ordering::SeqCst);
        }
        // shortcut: Glow eframe 0.37 menampilkan jendela sebelum swap_buffers; hapus saat upstream memperbaikinya.
        let startup = windows::StartupWindow::new(
            window.hwnd.load(Ordering::SeqCst),
            cc.egui_ctx.cumulative_frame_nr(),
        );

        let ctx = cc.egui_ctx.clone();
        let core = controller::spawn(move || ctx.request_repaint());

        let config_path = config::config_path();
        let (config, config_error) = match config_path.as_deref().map(config::load) {
            Some(Ok(Some(c))) => (c, String::new()),
            Some(Ok(None)) => (Config::default(), String::new()),
            Some(Err(e)) => (Config::default(), e),
            None => (
                Config::default(),
                "LOCALAPPDATA tidak tersedia; konfigurasi tidak disimpan.".into(),
            ),
        };

        // Handler hotkey/tray berjalan di message loop thread utama, juga saat jendela tersembunyi.
        let tx = core.tx.clone();
        let hotkeys = windows::Hotkeys::new(move || {
            let _ = tx.send(Cmd::Toggle(Source::Hotkey));
        });
        let (tx, win, ctx) = (core.tx.clone(), window.clone(), cc.egui_ctx.clone());
        let tray = windows::create_tray(move |action| match action {
            windows::TrayAction::Open => {
                win.open_requested.store(true, Ordering::SeqCst);
                windows::request_show_window(win.hwnd.load(Ordering::SeqCst));
                ctx.request_repaint();
            }
            windows::TrayAction::Cancel => {
                let _ = tx.send(Cmd::Cancel);
            }
            windows::TrayAction::Quit => {
                win.quitting.store(true, Ordering::SeqCst);
                // Tampilkan jendela agar event loop memproses penutupan.
                windows::show_window(win.hwnd.load(Ordering::SeqCst));
                ctx.request_repaint();
            }
        });

        let mut app = Self {
            _activation: {
                let (win, ctx) = (window.clone(), cc.egui_ctx.clone());
                windows::Activation::new(move || {
                    win.open_requested.store(true, Ordering::SeqCst);
                    windows::request_show_window(win.hwnd.load(Ordering::SeqCst));
                    ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
                    ctx.request_repaint();
                })
            },
            core,
            cloud_usage: None,
            logo: theme::logo_texture(&cc.egui_ctx),
            icons: theme::Icons::load(&cc.egui_ctx),
            window,
            startup,
            start_in_tray: std::env::args_os().any(|arg| arg == "--tray"),
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
            result_flash: None,
            copy_flash: None,
            form_changed: None,
            rows: [[0.0; 2]; 2],
            extra: 0.0,
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
        if !self.config_error.is_empty() {
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

    fn header(&mut self, ui: &mut Ui) {
        let english = self.config.language == Language::English;
        let t = |id, en| if english { en } else { id };
        let (phase, backend, loaded) = {
            let st = self.core.status();
            (
                st.phase,
                st.backend.clone(),
                st.loaded.as_ref().map(|l| l.0),
            )
        };
        // Tanpa title bar OS: area kosong header menjadi pegangan geser jendela. Didaftarkan
        // sebelum isi header agar tombol dan badge tetap menerima klik.
        let drag = ui.interact(
            ui.max_rect().expand2(vec2(16.0, 12.0)),
            ui.scope_id().with("drag"),
            egui::Sense::click_and_drag(),
        );
        if drag.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.add(egui::Image::new((self.logo.id(), vec2(30.0, 30.0))));
            ui.label(RichText::new("fi-whisper").font(semibold(21.0)).color(TEXT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if window_badge(ui, &self.icons.close, DANGER, t("Tutup", "Close")) {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if window_badge(
                    ui,
                    &self.icons.minimize,
                    SECONDARY,
                    t("Perkecil", "Minimize"),
                ) {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                link_badge(ui, &self.icons.home, None, LINK_HOME, HOME_URL);
                link_badge(
                    ui,
                    &self.icons.github,
                    Some(concat!("v", env!("CARGO_PKG_VERSION"))),
                    ACCENT,
                    REPO_URL,
                );
                let color = phase_color(phase);
                let id = ui.scope_id().with("phase_color");
                let components = color.to_array();
                let animated = std::array::from_fn::<_, 4, _>(|i| {
                    ui.ctx()
                        .animate_value_with_time(id.with(i), components[i] as f32, 0.15)
                });
                let background = egui::Color32::from_rgb(
                    animated[0] as u8,
                    animated[1] as u8,
                    animated[2] as u8,
                );
                status_badge(ui, phase_label(phase, english), color, background);

                {
                    let engine = loaded.unwrap_or(self.engine);
                    // Lewati badge mesin bila jendela terlalu sempit agar tidak menimpa judul.
                    let text = if engine == Engine::GoogleChirp3 {
                        engine.short().to_owned()
                    } else {
                        format!("{}, {backend}", engine.short())
                    };
                    let width = ui
                        .painter()
                        .layout_no_wrap(text.clone(), medium(12.0), SECONDARY)
                        .size()
                        .x
                        + 28.0;
                    if ui.available_width() >= width {
                        let badge_color =
                            if engine != Engine::GoogleChirp3 && backend.starts_with("CPU") {
                                WARN
                            } else {
                                SECONDARY
                            };
                        status_badge(ui, &text, badge_color, badge_color);
                    }
                }
            });
        });
    }

    fn dictation_card(&mut self, ui: &mut Ui) {
        let english = self.config.language == Language::English;
        let t = |id, en| if english { en } else { id };
        let st = self.core.status();
        let phase = st.phase;
        let recording = phase == Phase::Recording;
        let duration = st
            .recording_since
            .map_or(st.captured, |t| t.elapsed().min(audio::LIMIT));
        let now = std::time::Instant::now();
        let dt = now.duration_since(self.level_time).as_secs_f32();
        self.level_time = now;
        if let Some(level) = &st.level {
            // Peak meluruh perlahan agar meter terbaca.
            self.level = audio::take_level(level).max(self.level * (-dt / 0.3).exp());
        } else {
            self.level = 0.0;
        }
        let (note, error) = (st.note.clone(), st.error.clone());
        let ready = st.loaded.is_some();
        drop(st);

        ui.horizontal(|ui| {
            title(ui, t("Dikte", "Dictation"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                match self.shortcut() {
                    Some(s) => keycap(ui, s),
                    None => {
                        ui.label(
                            RichText::new(t("Shortcut tidak aktif", "Shortcut inactive"))
                                .small()
                                .color(WARN),
                        );
                    }
                }
            });
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "{:02}:{:02}",
                    duration.as_secs() / 60,
                    duration.as_secs() % 60
                ))
                .monospace()
                .size(36.0)
                .color(if recording { TEXT } else { SECONDARY }),
            );
            ui.label(RichText::new("/ 02:00").monospace().color(MUTED));
            if recording {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    rec_indicator(ui);
                });
            }
        });
        ui.add_space(2.0);
        meter(
            ui,
            recording,
            matches!(phase, Phase::Loading | Phase::Transcribing),
            self.level,
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let (enabled, label) = match phase {
                Phase::Recording => (true, t("Selesai", "Done")),
                Phase::Transcribing => (false, "Memproses…"),
                Phase::Loading => (false, "Memuat…"),
                _ => (ready, t("Mulai dikte", "Start dictation")),
            };
            if primary(ui, enabled, recording, label).clicked() {
                self.core.send(Cmd::Toggle(Source::Gui));
            }
            if matches!(phase, Phase::Recording | Phase::Transcribing)
                && secondary(ui, true, t("Batalkan", "Cancel")).clicked()
            {
                self.core.send(Cmd::Cancel);
            }
        });
        if !error.is_empty() {
            ui.label(
                RichText::new(message(&error, english))
                    .small()
                    .color(DANGER),
            );
        }
        if !note.is_empty() {
            ui.label(
                RichText::new(message(&note, english))
                    .small()
                    .color(SECONDARY),
            );
        }
        if !ready && matches!(phase, Phase::MissingModel | Phase::Error) {
            ui.label(
                RichText::new(t(
                    "Pilih model ASR dan VAD, lalu tekan Muat model.",
                    "Select ASR and VAD models, then press Load model.",
                ))
                .small()
                .color(SECONDARY),
            );
        }
    }

    fn result_card(&mut self, ui: &mut Ui) {
        let english = self.config.language == Language::English;
        let t = |id, en| if english { en } else { id };
        let (result, result_note, timing, revision) = {
            let st = self.core.status();
            (
                st.result.clone(),
                st.result_note.clone(),
                st.timing,
                st.result_revision,
            )
        };
        if revision != self.result_revision {
            self.result_revision = revision;
            self.result_flash = Some(std::time::Instant::now());
        }
        let flash = flash_amount(ui, self.result_flash, 0.6);
        title(ui, t("Hasil terakhir", "Last result"));
        inset(false)
            .stroke(egui::Stroke::new(
                1.0,
                egui::lerp(egui::Rgba::from(BORDER)..=egui::Rgba::from(ACCENT), flash),
            ))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical()
                    .id_salt("result")
                    .min_scrolled_height(110.0)
                    .max_height(110.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.set_min_height(110.0);
                        if result.is_empty() {
                            ui.label(
                                RichText::new(t(
                                    "Hasil dikte akan muncul di sini.",
                                    "Your dictation will appear here.",
                                ))
                                .color(MUTED),
                            );
                        } else {
                            ui.add(
                                egui::Label::new(RichText::new(&result).size(15.0).color(TEXT))
                                    .selectable(true)
                                    .wrap(),
                            );
                        }
                    });
            });
        let status = match (result.is_empty(), result_note.as_str()) {
            (false, COPIED) => Some(t(COPIED, "Copied to clipboard.")),
            (false, "") => Some(t("Selesai", "Done")),
            _ => None,
        };
        let mut copy_note = None;
        ui.horizontal(|ui| {
            let copy_flash = flash_amount(ui, self.copy_flash, 0.5);
            let response = secondary(ui, !result.is_empty(), t("Salin", "Copy"));
            if copy_flash > 0.0 {
                ui.painter().rect_stroke(
                    response.rect,
                    6.0,
                    egui::Stroke::new(1.0, SUCCESS.gamma_multiply(copy_flash)),
                    egui::StrokeKind::Inside,
                );
            }
            let copied = response.clicked();
            if copied {
                copy_note = Some(
                    match windows::copy_text(self.window.hwnd.load(Ordering::SeqCst), &result) {
                        Ok(()) => {
                            self.copy_flash = Some(std::time::Instant::now());
                            COPIED.to_owned()
                        }
                        Err(e) => e,
                    },
                );
            }
            if secondary(ui, !result.is_empty(), t("Hapus", "Clear")).clicked() {
                self.core.send(Cmd::ClearResult);
            }
            if status.is_some() || timing.is_some() {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let text_width = |ui: &Ui, s: &str, color| {
                        ui.painter()
                            .layout_no_wrap(s.to_owned(), egui::FontId::proportional(12.0), color)
                            .size()
                            .x
                    };
                    let status_reserve = match status {
                        Some(s) if result_note == COPIED => {
                            text_width(ui, s, SUCCESS) + ui.spacing().item_spacing.x
                        }
                        _ => 0.0,
                    };
                    if let Some((tm, total)) = timing {
                        let full = format!(
                            "{:.1} s audio, {:.2} s {}",
                            tm.audio.as_secs_f32(),
                            total.as_secs_f32(),
                            t("proses", "processing")
                        );
                        let avail = ui.available_width() - status_reserve;
                        let label = if avail >= text_width(ui, &full, SECONDARY) {
                            Some(full)
                        } else {
                            let short = format!(
                                "{:.1} s, {:.2} s {}",
                                tm.audio.as_secs_f32(),
                                total.as_secs_f32(),
                                t("proses", "proc")
                            );
                            (avail >= text_width(ui, &short, SECONDARY)).then_some(short)
                        };
                        if let Some(label) = label {
                            ui.label(RichText::new(label).small().color(SECONDARY))
                                .on_hover_text(format!(
                                    "VAD {:.2} s, ASR {:.2} s",
                                    tm.vad.as_secs_f32(),
                                    tm.asr.as_secs_f32()
                                ));
                        }
                    }
                    if let Some(text) = status
                        && ui.available_width() >= text_width(ui, text, SUCCESS)
                    {
                        ui.label(RichText::new(text).small().color(SUCCESS));
                    }
                });
            }
        });
        if !result_note.is_empty() && result_note != COPIED {
            ui.label(
                RichText::new(message(&result_note, english))
                    .small()
                    .color(WARN),
            );
        }
        if let Some(note) = copy_note {
            self.core.status().result_note = note;
        }
    }

    fn input_card(&mut self, ui: &mut Ui) {
        let english = self.config.language == Language::English;
        let t = |id, en| if english { en } else { id };
        let idle = !self.core.status().phase.busy();
        title(ui, t("Pengaturan", "Settings"));
        let width = ui.available_width();
        let mut changed = false;
        ui.add_enabled_ui(idle, |ui| {
            field_label(ui, t("Mikrofon", "Microphone"));
            let selected = match &self.config.microphone {
                None => {
                    let name = self
                        .default_mic
                        .as_ref()
                        .and_then(|id| self.mics.iter().find(|m| &m.id == id))
                        .map_or(t("tidak ada", "none"), |m| m.name.as_str());
                    format!("Default, {name}")
                }
                Some(id) => self.mics.iter().find(|m| &m.id == id).map_or_else(
                    || t("Perangkat tidak ditemukan", "Device not found").into(),
                    |m| m.name.clone(),
                ),
            };
            let response = combo(
                ui,
                egui::ComboBox::from_id_salt("microphone")
                    .width(width)
                    .truncate()
                    .selected_text(selected),
                |ui| {
                    changed |= interactive(ui, |ui| {
                        ui.selectable_value(
                            &mut self.config.microphone,
                            None,
                            t("Default sistem", "System default"),
                        )
                    })
                    .changed();
                    for mic in &self.mics {
                        changed |= interactive(ui, |ui| {
                            ui.selectable_value(
                                &mut self.config.microphone,
                                Some(mic.id.clone()),
                                &mic.name,
                            )
                        })
                        .changed();
                    }
                },
            );
            // Perbarui daftar mikrofon setiap kali dropdown dibuka.
            if response.clicked() {
                self.refresh_mics();
            }
            {
                field_label(ui, t("Bahasa", "Language"));
                combo(
                    ui,
                    egui::ComboBox::from_id_salt("language")
                        .width(width)
                        .selected_text(self.config.language.label()),
                    |ui| {
                        for language in Language::ALL {
                            changed |= interactive(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.config.language,
                                    language,
                                    language.label(),
                                )
                            })
                            .changed();
                        }
                    },
                );
            }
            field_label(ui, "Shortcut");
            ui.horizontal(|ui| {
                let response = text_field(
                    ui,
                    "shortcut-input",
                    &mut self.shortcut_input,
                    ui.available_width() - 104.0,
                    "Ctrl+Alt+Space",
                );
                let trimmed = self.shortcut_input.trim();
                let dirty = !trimmed.is_empty()
                    && (self.shortcut() != Some(trimmed) || !self.shortcut_error.is_empty());
                let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if action_button(
                    ui,
                    dirty,
                    dirty,
                    ui.available_width(),
                    t("Terapkan", "Apply"),
                )
                .clicked()
                    || (enter && !trimmed.is_empty())
                {
                    self.apply_shortcut();
                }
            });
        });
        if !self.mic_error.is_empty() {
            ui.label(
                RichText::new(message(&self.mic_error, english))
                    .small()
                    .color(WARN),
            );
        }
        if !self.shortcut_error.is_empty() {
            ui.label(
                RichText::new(message(&self.shortcut_error, english))
                    .small()
                    .color(WARN),
            );
        }
        if changed {
            self.refresh_mics();
            self.save_config();
            self.push_settings();
        }

        field_label(ui, t("Tombol close", "Close button"));
        let tray = self.tray.is_some();
        let old = if self.config.auto_startup {
            2
        } else {
            i32::from(self.config.close_to_tray)
        };
        let mut selected = old;
        combo(
            ui,
            egui::ComboBox::from_id_salt("close-action")
                .width(width)
                .selected_text(match selected {
                    2 => t(
                        "Sembunyikan ke tray, auto-startup",
                        "Hide to tray, start with Windows",
                    ),
                    1 => t("Sembunyikan ke tray", "Hide to tray"),
                    _ => t("Keluar dari aplikasi", "Quit application"),
                }),
            |ui| {
                ui.add_enabled_ui(tray, |ui| {
                    ui.selectable_value(&mut selected, 1, t("Sembunyikan ke tray", "Hide to tray"));
                    ui.selectable_value(
                        &mut selected,
                        2,
                        t(
                            "Sembunyikan ke tray, auto-startup",
                            "Hide to tray, start with Windows",
                        ),
                    );
                });
                ui.selectable_value(
                    &mut selected,
                    0,
                    t("Keluar dari aplikasi", "Quit application"),
                );
            },
        );
        if selected != old && self.config_error.is_empty() {
            match windows::set_auto_startup(selected == 2) {
                Ok(()) => {
                    self.config.close_to_tray = selected != 0;
                    self.config.auto_startup = selected == 2;
                    self.save_config();
                }
                Err(e) => self.tray_error = e,
            }
        }
        if !self.tray_error.is_empty() {
            ui.label(
                RichText::new(message(&self.tray_error, english))
                    .small()
                    .color(WARN),
            );
        }
    }

    fn model_card(&mut self, ui: &mut Ui) {
        let english = self.config.language == Language::English;
        let t = |id, en| if english { en } else { id };
        let (phase, diagnostics, loaded, load_time) = {
            let st = self.core.status();
            (
                st.phase,
                st.diagnostics.clone(),
                st.loaded.clone(),
                st.load_time,
            )
        };
        title(ui, "Model");
        ui.add_enabled_ui(!phase.busy(), |ui| {
            field_label(ui, t("Mesin ASR", "ASR engine"));
            let previous = self.engine;
            let response = combo(
                ui,
                egui::ComboBox::from_id_salt("engine")
                    .width(ui.available_width())
                    .selected_text(self.engine.label()),
                |ui| {
                    for engine in Engine::ALL {
                        interactive(ui, |ui| {
                            ui.selectable_value(&mut self.engine, engine, engine.label())
                        });
                    }
                },
            );
            if previous != self.engine
                && (previous == Engine::GoogleChirp3) != (self.engine == Engine::GoogleChirp3)
            {
                self.form_changed = Some(std::time::Instant::now());
            }
            let fade = 1.0 - flash_amount(ui, self.form_changed, 0.15);
            ui.multiply_opacity(0.65 + 0.35 * fade);
            if self.engine == Engine::GoogleChirp3 {
                if previous != self.engine {
                    self.config.engine = self.engine;
                    self.save_config();
                    self.activate_cloud();
                }
                self.cloud_settings(ui);
                return;
            }
            if response.clicked() || previous != self.engine {
                self.refresh_models();
            }
            if previous != self.engine {
                self.config.engine = self.engine;
                self.save_config();
                self.core.send(Cmd::Load {
                    engine: self.engine,
                    asr: self.selected_asr().unwrap_or_default(),
                    vad: self.vad.clone().unwrap_or_default(),
                });
            }
            let engine = self.engine;
            let (label, pick) = if engine.is_qwen() {
                (t("Folder model", "Model folder"), Pick::Folder)
            } else {
                (t("File model ASR", "ASR model file"), Pick::GgmlFile)
            };
            let (path, valid) = self.asr_path_mut(engine);
            model_field(ui, label, pick, engine.default_name(), path, valid, english);
            model_field(
                ui,
                "VAD (Silero)",
                Pick::GgmlFile,
                &format!(r"{}\{}", config::VAD_DIR, config::VAD_FILE),
                &mut self.vad,
                &mut self.model_valid[3],
                english,
            );
            let selected = self.selected_asr();
            let same = loaded.as_ref().is_some_and(|(e, a, v)| {
                *e == self.engine && Some(a) == selected.as_ref() && Some(v) == self.vad.as_ref()
            });
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let label = if same {
                    t("Muat ulang", "Reload")
                } else {
                    t("Muat model", "Load model")
                };
                if action_button(
                    ui,
                    selected.is_some() && self.vad.is_some(),
                    !same,
                    0.0,
                    label,
                )
                .clicked()
                    && let (Some(asr), Some(vad)) = (selected.clone(), self.vad.clone())
                {
                    self.refresh_models();
                    self.config.engine = self.engine;
                    match self.engine {
                        Engine::Whisper => self.config.asr_model = Some(asr.clone()),
                        Engine::Qwen3Asr => self.config.qwen_model = Some(asr.clone()),
                        Engine::Qwen3Asr17 => self.config.qwen17_model = Some(asr.clone()),
                        Engine::GoogleChirp3 => unreachable!(),
                    }
                    self.config.vad_model = Some(vad.clone());
                    self.save_config();
                    self.core.send(Cmd::Load {
                        engine: self.engine,
                        asr,
                        vad,
                    });
                }
                if phase == Phase::Loading {
                    ui.label(RichText::new("Memuat…").small().color(SECONDARY));
                } else if let (true, Some(t)) = (same, load_time) {
                    ui.label(
                        RichText::new(format!(
                            "{} {:.1} s",
                            if english { "Loaded in" } else { "Dimuat" },
                            t.as_secs_f32()
                        ))
                        .small()
                        .color(SECONDARY),
                    );
                }
            });
        });
        if !diagnostics.is_empty() {
            interactive(ui, |ui| {
                egui::CollapsingHeader::new(
                    RichText::new(t("Diagnostik", "Diagnostics"))
                        .small()
                        .color(SECONDARY),
                )
                .id_salt("diagnostics")
                .show(ui, |ui| {
                    for line in &diagnostics {
                        ui.label(RichText::new(line).monospace().size(11.0).color(SECONDARY));
                    }
                })
                .header_response
            });
        }
    }

    fn activate_cloud(&self) {
        self.core.send(Cmd::Google(self.config.google.clone()));
        self.core.send(Cmd::Load {
            engine: Engine::GoogleChirp3,
            asr: PathBuf::new(),
            vad: PathBuf::new(),
        });
    }

    fn cloud_settings(&mut self, ui: &mut Ui) {
        let english = self.config.language == Language::English;
        let t = |id, en| if english { en } else { id };
        field_label(ui, "Project ID");
        let mut changed = text_field(
            ui,
            "cloud-project",
            &mut self.config.google.project,
            ui.available_width(),
            "my-gcp-project-id",
        )
        .changed();
        ui.columns_const(|[l, r]: &mut [Ui; 2]| {
            l.with_layout(Layout::top_down(Align::Min), |ui| {
                field_label(ui, "Region");
                let width = ui.available_width();
                combo(
                    ui,
                    egui::ComboBox::from_id_salt("cloud-region")
                        .width(width)
                        .selected_text(&self.config.google.region),
                    |ui| {
                        for region in ["us", "eu"] {
                            changed |= interactive(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.config.google.region,
                                    region.into(),
                                    region,
                                )
                            })
                            .changed();
                        }
                    },
                );
            });
            r.with_layout(Layout::top_down(Align::Min), |ui| {
                field_label(ui, t("Batas / bulan", "Monthly limit"));
                let width = ui.available_width();
                changed |= interactive(ui, |ui| {
                    ui.add_sized(
                        [width, 34.0],
                        egui::DragValue::new(&mut self.config.google.monthly_limit_minutes)
                            .range(0..=60000)
                            .suffix(t(" menit", " min")),
                    )
                })
                .changed();
            });
        });
        if changed {
            self.save_config();
            self.core.send(Cmd::Google(self.config.google.clone()));
        }
        if self.cloud_usage.as_ref().is_none_or(|(at, project, _)| {
            at.elapsed() >= Duration::from_secs(1) || project != &self.config.google.project
        }) {
            self.cloud_usage = Some((
                std::time::Instant::now(),
                self.config.google.project.clone(),
                cloud::usage_seconds(&self.config.google.project),
            ));
        }
        ui.add_space(2.0);
        match self.cloud_usage.as_ref().unwrap().2.clone() {
            Ok(seconds) => {
                let minutes = seconds as f64 / 60.0;
                let near_limit =
                    minutes >= f64::from(self.config.google.monthly_limit_minutes) * 0.8;
                ui.label(
                    RichText::new(if english {
                        format!(
                            "This month: {minutes:.1} min | est. US${:.2}",
                            minutes * 0.016
                        )
                    } else {
                        format!(
                            "Bulan ini: {minutes:.1} menit | estimasi US${:.2}",
                            minutes * 0.016
                        )
                    })
                    .small()
                    .color(if near_limit { WARN } else { SECONDARY }),
                );
            }
            Err(e) => {
                ui.label(RichText::new(e).small().color(DANGER));
            }
        }
        interactive(ui, |ui| {
            egui::CollapsingHeader::new(RichText::new("Setup ADC").small().color(SECONDARY))
                .id_salt("setup-adc")
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(t(
                            "Jalankan di terminal, lalu mulai dikte:",
                            "Run in a terminal, then start dictation:",
                        ))
                        .small()
                        .color(SECONDARY),
                    );
                    let project = if self.config.google.project.trim().is_empty() {
                        "PROJECT_ID"
                    } else {
                        self.config.google.project.trim()
                    };
                    let cmd = format!(
                        "gcloud auth application-default login\ngcloud auth application-default set-quota-project {project}"
                    );
                    inset(false).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(
                            egui::Label::new(
                                RichText::new(cmd).monospace().size(11.5).color(TEXT),
                            )
                            .selectable(true),
                        );
                    });
                    ui.label(
                        RichText::new(t(
                            "Penghitung lokal bukan tagihan Google. Bulan mengikuti UTC. Sisa minimal 2 menit untuk memulai sesi.",
                            "Local counter, not your Google bill. Months use UTC. At least 2 minutes must remain to start a session.",
                        ))
                        .small()
                        .color(SECONDARY),
                    );
                })
                .header_response
        });
    }

    fn config_error_card(&mut self, ui: &mut Ui) {
        let english = self.config.language == Language::English;
        let t = |id, en| if english { en } else { id };
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(message(&self.config_error, english))
                    .small()
                    .color(WARN),
            );
            if secondary(
                ui,
                true,
                t("Reset konfigurasi ke default", "Reset configuration"),
            )
            .clicked()
            {
                self.config = Config::default();
                self.engine = self.config.engine;
                self.config_error.clear();
                self.save_config();
                self.shortcut_input = self.config.shortcut.clone();
                self.apply_shortcut();
                self.refresh_mics();
                self.refresh_models();
                self.push_settings();
            }
        });
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
    fn row(
        &mut self,
        ui: &mut Ui,
        index: usize,
        left: fn(&mut Self, &mut Ui),
        right: fn(&mut Self, &mut Ui),
    ) {
        let previous = self.rows[index];
        // Baris atas tetap stabil saat tinggi form pada baris bawah berubah.
        let target = if index == 0 {
            previous[0].max(previous[1])
        } else {
            previous[0].max(previous[1]) + self.extra
        };
        let mut heights = [0.0; 2];
        ui.columns_const(|[l, r]: &mut [Ui; 2]| {
            for (col, (ui, add)) in [(l, left), (r, right)].into_iter().enumerate() {
                card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    // Harus sebelum isi: set_min_height di egui dihitung dari kursor saat ini.
                    ui.set_min_height(target);
                    // Tinggi scope = tinggi isi saja, tidak terpengaruh set_min_height.
                    // Kolom egui memakai layout justified; tanpa ini teks label ikut rata kanan-kiri.
                    heights[col] = ui
                        .with_layout(Layout::top_down(Align::Min), |ui| add(self, ui))
                        .response
                        .rect
                        .height();
                });
            }
        });
        if heights != previous {
            self.rows[index] = heights;
            ui.ctx().request_discard("row height");
            ui.ctx().request_repaint();
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

const COPIED: &str = "Disalin ke clipboard.";
const REPO_URL: &str = "https://github.com/fimln/fi-whisper";
const HOME_URL: &str = "https://alfi.ai.id";

fn phase_color(phase: Phase) -> egui::Color32 {
    match phase {
        Phase::CloudIdle | Phase::Ready => SUCCESS,
        Phase::Error | Phase::MissingModel => DANGER,
        Phase::Recording => SUCCESS,
        Phase::Loading | Phase::Transcribing => ACCENT,
    }
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

/// Shortcut ditampilkan sebagai keycap.
fn keycap(ui: &mut Ui, text: &str) {
    egui::Frame::new()
        .fill(RAISED)
        .stroke(egui::Stroke::new(1.0, BORDER))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.label(RichText::new(text).monospace().size(11.5).color(SECONDARY));
        })
        .response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Keyboard, true, text));
}

/// Indikator rekaman live (merah berdenyut), terpisah dari tombol utama.
fn rec_indicator(ui: &mut Ui) {
    let t = ui.input(|i| i.time) as f32;
    let pulse = 0.55 + 0.45 * (t * 4.0).sin().abs();
    ui.label(RichText::new("REC").font(semibold(12.0)).color(DANGER));
    let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.circle_filled(rect.center(), 7.0, DANGER.gamma_multiply(0.2 * pulse));
    painter.circle_filled(rect.center(), 4.0, DANGER.gamma_multiply(pulse));
}

fn meter_fraction(level: f32) -> f32 {
    // Skala visual -60..-24 dBFS agar suara kuat memenuhi meter tanpa perlu clipping.
    let db = if level > 0.0 {
        20.0 * level.log10()
    } else {
        -60.0
    };
    ((db + 60.0) / 36.0).clamp(0.0, 1.0)
}

fn flash_amount(ui: &Ui, started: Option<std::time::Instant>, duration: f32) -> f32 {
    let value = started.map_or(0.0, |t| {
        (1.0 - t.elapsed().as_secs_f32() / duration).clamp(0.0, 1.0)
    });
    if value > 0.0 {
        ui.ctx().request_repaint_after(Duration::from_millis(33));
    }
    value
}

fn meter(ui: &mut Ui, active: bool, busy: bool, level: f32) {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), 36.0), egui::Sense::hover());
    let fraction = meter_fraction(level);
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::new(egui::Role::Meter);
        info.label = Some("Level audio".into());
        info.value = Some((fraction * 100.0).round().into());
        info
    });
    let count = ((rect.width() / 6.0) as usize).max(8);
    let step = rect.width() / count as f32;
    let exact = fraction * count as f32;
    let filled = exact.ceil() as usize;
    let t = ui.input(|i| i.time) as f32;
    for i in 0..count {
        let on = active && i < filled;
        let edge = (exact - i as f32).clamp(0.0, 1.0);
        let processing = busy && ((i as f32 - (t * 12.0) % (count as f32 + 8.0) + 8.0).abs() < 3.0);
        // Tinggi bar bervariasi mengikuti dinamika suara dan meluruh halus di batang ujung.
        let wave = 0.55 + 0.45 * ((i as f32 * 0.9 + t * 7.0).sin() * 0.5 + 0.5);
        let height = if on {
            6.0 + 26.0 * wave * (0.35 + 0.65 * fraction) * edge
        } else if processing {
            10.0
        } else {
            4.0
        };
        let bar = egui::Rect::from_center_size(
            egui::pos2(rect.left() + (i as f32 + 0.5) * step, rect.center().y),
            vec2((step - 3.0).max(1.0), height),
        );
        let color = if processing {
            ACCENT
        } else if !on {
            BORDER
        } else if i + 1 == filled {
            ACCENT_HOVER
        } else {
            ACCENT
        };
        ui.painter().rect_filled(bar, 2.0, color);
    }
}

#[cfg(test)]
mod meter_tests {
    use super::meter_fraction;

    #[test]
    fn speech_meter_range() {
        let level = |db: f32| 10.0_f32.powf(db / 20.0);
        assert_eq!(meter_fraction(0.0), 0.0);
        assert_eq!(meter_fraction(level(-66.0)), 0.0);
        assert_eq!(meter_fraction(level(-60.0)), 0.0);
        assert!((meter_fraction(level(-42.0)) - 0.5).abs() < 1e-6);
        assert!((meter_fraction(level(-33.0)) - 0.75).abs() < 1e-6);
        assert_eq!(meter_fraction(level(-24.0)), 1.0);
        assert_eq!(meter_fraction(level(-12.0)), 1.0);
        assert_eq!(meter_fraction(1.0), 1.0);
    }
}

#[derive(Clone, Copy)]
enum Pick {
    GgmlFile,
    Folder,
}

/// `default` = nama file/folder bawaan di `models\`, dipakai sebagai petunjuk bila belum ada.
fn model_field(
    ui: &mut Ui,
    label: &str,
    pick: Pick,
    default: &str,
    path: &mut Option<PathBuf>,
    valid: &mut bool,
    english: bool,
) {
    let t = |id, en| if english { en } else { id };
    field_label(ui, label);
    let exists = *valid;
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 72.0).max(60.0);
        let color = match (path.is_some(), exists) {
            (true, true) => TEXT,
            (true, false) => DANGER,
            _ => MUTED,
        };
        let response = egui::Frame::new()
            .fill(INPUT)
            .stroke(egui::Stroke::new(1.0, BORDER))
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(10, 0))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    vec2(width - 22.0, ui.spacing().interact_size.y - 2.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.set_min_width(width - 22.0);
                        let name = path
                            .as_deref()
                            .map_or_else(|| t("Belum dipilih", "Not selected").into(), file_name);
                        ui.add(egui::Label::new(RichText::new(name).color(color)).truncate());
                    },
                );
            })
            .response;
        match path.as_deref() {
            Some(p) if exists => {
                response.on_hover_text(p.display().to_string());
            }
            Some(p) => {
                response.on_hover_text(format!(
                    "{}: {}",
                    t(
                        "Model tidak lengkap atau tidak ditemukan",
                        "Model incomplete or not found"
                    ),
                    p.display()
                ));
            }
            None => {}
        }
        let pick_btn = action_button(ui, true, false, ui.available_width(), t("Pilih", "Browse"));
        let a11y_label = format!("{} {label}", t("Pilih", "Browse"));
        pick_btn.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), &a11y_label)
        });
        if pick_btn.clicked() {
            let dialog =
                rfd::FileDialog::new().set_title(format!("{} {label}", t("Pilih", "Select")));
            let selected = match pick {
                Pick::GgmlFile => dialog.add_filter("Model GGML", &["bin"]).pick_file(),
                Pick::Folder => dialog.pick_folder(),
            };
            if let Some(selected) = selected {
                *valid = match pick {
                    Pick::GgmlFile => selected.is_file(),
                    Pick::Folder => config::validate_qwen_dir(&selected).is_ok(),
                };
                *path = Some(selected);
            }
        }
    });
    if !exists {
        ui.label(
            RichText::new(if path.is_some() {
                t(
                    "Model tidak lengkap atau tidak ditemukan.",
                    "Model incomplete or not found.",
                )
                .into()
            } else {
                format!("{} models\\{default}", t("Belum ada di", "Missing from"))
            })
            .small()
            .color(WARN),
        );
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.window.open_requested.swap(false, Ordering::SeqCst) {
            let hwnd = self.window.hwnd.load(Ordering::SeqCst);
            // Seperti startup, tunggu satu frame lengkap sebelum DWM menampilkan jendela.
            if self.startup.is_none() {
                self.startup = windows::StartupWindow::new(hwnd, ctx.cumulative_frame_nr());
            }
            self.start_in_tray = false;
            windows::show_window(hwnd);
        }
        if self
            .startup
            .as_ref()
            .is_some_and(|startup| startup.ready(ctx))
        {
            if self.start_in_tray && self.tray.is_some() {
                windows::hide_window(self.window.hwnd.load(Ordering::SeqCst));
                if !self.core.status().phase.busy() {
                    windows::trim_idle_memory();
                }
            }
            self.start_in_tray = false;
            self.startup = None;
        }
        if self.window.quitting.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if ctx.input(|i| i.viewport().close_requested())
            && self.config.close_to_tray
            && self.tray.is_some()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            windows::hide_window(self.window.hwnd.load(Ordering::SeqCst));
            if !self.core.status().phase.busy() {
                windows::trim_idle_memory();
            }
        }
        if self.core.status().phase.busy() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    fn ui(&mut self, root: &mut Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("header")
            .resizable(false)
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin {
                left: 16,
                right: 16,
                top: 12,
                bottom: 10,
            }))
            .show(root, |ui| self.header(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin {
                left: 16,
                right: 16,
                top: 12,
                bottom: 16,
            }))
            .show(root, |ui| {
                let available = ui.available_height();
                // Cadangan bila jendela diperkecil atau pesan error panjang membuat isi melebihi tinggi.
                let style = ui.style().clone();
                let scroll = &mut ui.spacing_mut().scroll;
                scroll.foreground_color = true;
                scroll.floating_width = 5.0;
                scroll.bar_width = 8.0;
                scroll.dormant_handle_opacity = 0.10;
                scroll.active_handle_opacity = 0.18;
                scroll.interact_handle_opacity = 0.30;
                egui::ScrollArea::vertical()
                    .scroll_bar_visibility(
                        egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded,
                    )
                    .show(ui, |ui| {
                        ui.set_style(style);
                        ui.set_width(ui.available_width());
                        if !self.config_error.is_empty() {
                            self.config_error_card(ui);
                        }
                        self.row(ui, 0, Self::dictation_card, Self::result_card);
                        self.row(ui, 1, Self::input_card, Self::model_card);
                        // Koreksi linear dari frame ini; stabil setelah satu pass tambahan.
                        // Dibulatkan ke bawah agar pembulatan piksel tidak memunculkan scrollbar.
                        let used = ui.min_rect().height();
                        let extra = (self.extra + available - used - 1.0).max(0.0).floor();
                        if (extra - self.extra).abs() >= 1.0 {
                            self.extra = extra;
                            ui.ctx().request_discard("extra height");
                            ui.ctx().request_repaint();
                        }
                    });
            });
        root.painter().rect_stroke(
            root.ctx().content_rect(),
            0.0,
            egui::Stroke::new(1.0, BORDER),
            egui::StrokeKind::Inside,
        );
        resize_edges(root);
    }
}

/// Jendela tanpa dekorasi tidak punya bingkai resize dari OS (winit membuang area non-client),
/// jadi tepi dan sudut jendela dijadikan pegangan `BeginResize`. Didaftarkan terakhir agar
/// menang atas widget di bawahnya; sudut setelah tepi agar diagonal diprioritaskan.
fn resize_edges(ui: &mut Ui) {
    use egui::{CursorIcon as C, ResizeDirection as D, pos2};
    let r = ui.ctx().content_rect();
    let (e, k) = (5.0, 12.0);
    let edges = [
        (
            pos2(r.left(), r.top()),
            pos2(r.right(), r.top() + e),
            D::North,
            C::ResizeNorth,
        ),
        (
            pos2(r.left(), r.bottom() - e),
            pos2(r.right(), r.bottom()),
            D::South,
            C::ResizeSouth,
        ),
        (
            pos2(r.left(), r.top()),
            pos2(r.left() + e, r.bottom()),
            D::West,
            C::ResizeWest,
        ),
        (
            pos2(r.right() - e, r.top()),
            pos2(r.right(), r.bottom()),
            D::East,
            C::ResizeEast,
        ),
        (
            pos2(r.left(), r.top()),
            pos2(r.left() + k, r.top() + k),
            D::NorthWest,
            C::ResizeNorthWest,
        ),
        (
            pos2(r.right() - k, r.top()),
            pos2(r.right(), r.top() + k),
            D::NorthEast,
            C::ResizeNorthEast,
        ),
        (
            pos2(r.left(), r.bottom() - k),
            pos2(r.left() + k, r.bottom()),
            D::SouthWest,
            C::ResizeSouthWest,
        ),
        (
            pos2(r.right() - k, r.bottom() - k),
            pos2(r.right(), r.bottom()),
            D::SouthEast,
            C::ResizeSouthEast,
        ),
    ];
    for (i, (min, max, dir, cursor)) in edges.into_iter().enumerate() {
        let response = ui
            .interact(
                egui::Rect::from_min_max(min, max),
                ui.scope_id().with(("resize", i)),
                egui::Sense::drag(),
            )
            .on_hover_cursor(cursor);
        if response.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
        }
    }
}

fn main() -> eframe::Result {
    let Some(_instance) = windows::single_instance() else {
        return Ok(());
    };
    eframe::run_native(
        "fi-whisper",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([700.0, 700.0])
                .with_min_inner_size([560.0, 600.0])
                .with_decorations(false)
                .with_taskbar(!std::env::args_os().any(|arg| arg == "--tray"))
                .with_icon(theme::icon()),
            renderer: eframe::Renderer::Glow,
            centered: true,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
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

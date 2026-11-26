#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod cloud;
mod config;
mod controller;
mod focus;
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
    focus: Option<focus::Focus>,
    focus_error: String,
    cloud_usage: Option<(std::time::Instant, String, Result<u64, String>)>,
    settings_open: bool,
    view_changed: Option<std::time::Instant>,
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
    vocabulary_draft: Option<String>,
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
        let start_in_tray = std::env::args_os().any(|arg| arg == "--tray");
        // Saat masuk tray, sembunyikan frame pertama yang ditampilkan Glow.
        let startup = if start_in_tray {
            windows::StartupWindow::new(
                window.hwnd.load(Ordering::SeqCst),
                cc.egui_ctx.cumulative_frame_nr(),
            )
        } else {
            None
        };

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
        let hotkeys = windows::Hotkeys::new(move |pressed| {
            let _ = tx.send(Cmd::Hotkey(pressed));
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
            focus: None,
            focus_error: String::new(),
            cloud_usage: None,
            settings_open: false,
            view_changed: None,
            icons: theme::Icons::load(&cc.egui_ctx),
            window,
            startup,
            start_in_tray,
            engine: config.engine,
            asr: config.asr_model.clone().or_else(|| {
                (!config.engine.is_cloud())
                    .then(|| config::default_model(config::ASR_DIR, config::ASR_FILE))
                    .flatten()
            }),
            qwen: config.qwen_model.clone().or_else(|| {
                (!config.engine.is_cloud())
                    .then(|| config::default_model_dir(config::QWEN_DIR))
                    .flatten()
            }),
            qwen17: config.qwen17_model.clone().or_else(|| {
                (!config.engine.is_cloud())
                    .then(|| config::default_model_dir(config::QWEN17_DIR))
                    .flatten()
            }),
            vad: config.vad_model.clone().or_else(|| {
                (!config.engine.is_cloud())
                    .then(|| config::default_model(config::VAD_DIR, config::VAD_FILE))
                    .flatten()
            }),
            vocabulary_draft: None,
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
        if !app.engine.is_cloud() {
            app.refresh_models();
        }
        app.push_settings();
        app.configure_focus(&cc.egui_ctx);
        if app.engine.is_cloud() {
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
            Engine::GoogleChirp3 | Engine::DeepSeekAsr => None,
        }
    }

    fn asr_path_mut(&mut self, engine: Engine) -> (&mut Option<PathBuf>, &mut bool) {
        let path = match engine {
            Engine::Whisper => &mut self.asr,
            Engine::Qwen3Asr => &mut self.qwen,
            Engine::Qwen3Asr17 => &mut self.qwen17,
            Engine::GoogleChirp3 | Engine::DeepSeekAsr => {
                unreachable!("Cloud tidak memiliki path model")
            }
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
        if let Some(path) = &self.config_path {
            if self.config.deepseek.api_key.is_empty()
                && let Ok(Some(disk)) = config::load(path)
                && !disk.deepseek.api_key.is_empty()
            {
                self.config.deepseek.api_key = disk.deepseek.api_key;
            }
            if let Err(e) = config::save(path, &self.config) {
                self.config_error = format!("Konfigurasi gagal disimpan: {e}");
            }
        }
    }

    fn push_settings(&self) {
        if let Some(tray) = &self.tray {
            tray.set_english(self.config.language == Language::English);
        }
        self.core.send(Cmd::Settings {
            mic: self.config.microphone.clone(),
            language: self.config.language,
            hold_to_talk: self.config.hold_to_talk,
            vocabulary: self.config.vocabulary.as_str().to_owned(),
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

    fn set_settings_open(&mut self, open: bool) {
        if self.settings_open != open {
            self.settings_open = open;
            self.view_changed = Some(std::time::Instant::now());
            if !open {
                self.vocabulary_draft = None;
            }
        }
    }

    fn screen(&mut self, root: &mut Ui) {
        let phase = self.core.status().phase;
        // Jika dikte dimulai lewat shortcut saat di Pengaturan, kembali ke kartu Dikte agar status terlihat.
        if self.settings_open && dictation_expanded(phase) {
            self.set_settings_open(false);
        }
        if self.settings_open
            && self.vocabulary_draft.is_none()
            && !root.ctx().egui_wants_keyboard_input()
            && root.input(|i| i.key_pressed(egui::Key::Escape))
        {
            self.set_settings_open(false);
        }

        let header = egui::Panel::top("header")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(16, 11)),
            )
            .show(root, |ui| {
                ui.set_min_height(34.0);
                self.header(ui);
            });
        let rect = header.response.rect;
        root.painter().hline(
            rect.x_range(),
            rect.bottom(),
            egui::Stroke::new(1.0, BORDER),
        );
        // Margin berada dalam area gulir agar garis kartu tidak terpotong batas klip saat pembulatan piksel.
        let body = egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::ZERO))
            .show(root, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(("body", self.settings_open))
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .inner_margin(egui::Margin::same(16))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                let fade = 1.0 - flash_amount(ui, self.view_changed, 0.18);
                                ui.multiply_opacity(0.35 + 0.65 * fade);
                                if !self.config_error.is_empty() {
                                    self.config_error_card(ui);
                                }
                                if self.settings_open {
                                    self.row(ui, 1, Self::input_card, Self::model_card);
                                } else {
                                    self.main_row(ui);
                                }
                            });
                    })
            });
        let viewport = root.ctx().content_rect();
        if !root
            .input(|i| i.viewport().maximized == Some(true) || i.viewport().minimized == Some(true))
        {
            let scroll = body.inner;
            // Ukur seluruh isi meski terpotong, lalu bulatkan sekali ke piksel fisik.
            let scale = root.ctx().pixels_per_point();
            let height = scroll.inner_rect.top() - viewport.top() + scroll.content_size.y;
            let height = (height * scale).round() / scale;
            if ((height - viewport.height()) * scale).abs() >= 0.75 {
                let work = windows::monitor_work_area(self.window.hwnd.load(Ordering::SeqCst));
                let limit = work.map_or(height, |area| (area.bottom - area.top) as f32 / scale);
                let height = height.min(limit);
                if ((height - viewport.height()) * scale).abs() >= 0.75 {
                    if let (Some(area), Some(outer)) =
                        (work, root.input(|i| i.viewport().outer_rect))
                        && let Some(new_top) = clamped_window_top(
                            outer.top(),
                            height,
                            area.top as f32 / scale,
                            area.bottom as f32 / scale,
                        )
                    {
                        root.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                                outer.left(),
                                new_top,
                            )));
                    }
                    root.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(
                            viewport.width(),
                            height,
                        )));
                }
            }
        }
        root.painter().rect_stroke(
            root.ctx().content_rect(),
            0.0,
            egui::Stroke::new(1.0, BORDER),
            egui::StrokeKind::Inside,
        );
    }

    fn header(&mut self, ui: &mut Ui) {
        let english = self.config.language == Language::English;
        let t = |id, en| if english { en } else { id };
        let (phase, cpu, loaded) = {
            let st = self.core.status();
            (
                st.phase,
                st.backend.starts_with("CPU"),
                st.loaded.as_ref().map(|l| l.0),
            )
        };
        drag_header(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 11.0;
            let (rect, _) = ui.allocate_exact_size(vec2(30.0, 30.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 7.0, ACCENT);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "fi",
                semibold(18.0),
                ON_ACCENT,
            );
            ui.label(RichText::new("fi-whisper").font(semibold(23.0)).color(TEXT));
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
                let animated_status = animate_color(
                    ui.ctx(),
                    ui.scope_id().with("phase_color"),
                    color,
                    STATE_ANIM,
                );
                let engine = loaded.unwrap_or(self.engine);
                let (model, tint) = if cpu && !engine.is_cloud() {
                    (format!("{}, CPU", engine.short()), WARN)
                } else {
                    (engine.short().to_owned(), SECONDARY)
                };
                let animated_tint =
                    animate_color(ui.ctx(), ui.scope_id().with("model_tint"), tint, STATE_ANIM);
                // Sisakan lebar pil model sebelum menggambar status dalam layout kanan-ke-kiri.
                let model_width = ui
                    .painter()
                    .layout_no_wrap(model.clone(), medium(12.5), tint)
                    .size()
                    .x
                    + 2.0 * BADGE_PADDING as f32;
                let status_width =
                    (ui.available_width() - model_width - ui.spacing().item_spacing.x)
                        .floor()
                        .max(0.0);
                ui.scope(|ui| {
                    ui.set_max_width(status_width);
                    status_badge(
                        ui,
                        phase_label(phase, english),
                        animated_status,
                        animated_status,
                    );
                });
                status_badge(ui, &model, animated_tint, animated_tint);
            });
        });
    }

    fn dictation_card(&mut self, ui: &mut Ui, button_width: f32) {
        let english = self.config.language == Language::English;
        let t = |id, en| if english { en } else { id };
        let st = self.core.status();
        let phase = st.phase;
        let notice = dictation_notice(phase, &st.error, &st.note, english);
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
        let ready = st.loaded.is_some();
        drop(st);

        let rec_t = animate_bool_ease(
            ui.ctx(),
            egui::Id::unique("rec-active"),
            recording,
            STATE_ANIM,
        );

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
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(
                RichText::new(format!(
                    "{:02}:{:02}",
                    duration.as_secs() / 60,
                    duration.as_secs() % 60
                ))
                .monospace()
                .size(42.0)
                .color(mix(SECONDARY, TEXT, rec_t)),
            );
            ui.label(RichText::new("/ 02:00").monospace().size(14.0).color(MUTED));
            if rec_t > 0.0 {
                ui.scope(|ui| {
                    ui.multiply_opacity(rec_t);
                    rec_indicator(ui);
                });
            }
        });
        dictation_feedback(ui, phase, self.level, notice.as_deref());
        ui.horizontal(|ui| {
            let settings_label = t("Pengaturan", "Settings");
            let start = t("Mulai dikte", "Start dictation");
            let done = t("Selesai", "Done");
            let processing = t("Memproses…", "Processing…");
            let loading = t("Memuat…", "Loading…");
            let (enabled, label) = match phase {
                Phase::Recording => (true, done),
                Phase::Transcribing => (false, processing),
                Phase::Loading | Phase::Starting => (false, loading),
                _ => (ready, start),
            };
            let fill = mix(ACCENT, ACCENT_HOVER, rec_t);
            if dictation_button(ui, enabled, Some(fill), button_width, label).clicked() {
                self.core.send(Cmd::Toggle(Source::Gui));
            }
            if !matches!(phase, Phase::Recording | Phase::Transcribing)
                && dictation_button(ui, true, None, button_width, settings_label).clicked()
            {
                self.set_settings_open(true);
            }
            if matches!(phase, Phase::Recording | Phase::Transcribing)
                && dictation_button(ui, true, None, button_width, t("Batalkan", "Cancel")).clicked()
            {
                self.core.send(Cmd::Cancel);
            }
        });
    }

    fn vocabulary_dialog(&mut self, ctx: &egui::Context) {
        let english = self.config.language == Language::English;
        let t = |id, en| if english { en } else { id };
        let Some(draft) = &mut self.vocabulary_draft else {
            return;
        };
        let mut save = false;
        let dialog = egui::Modal::new(egui::Id::unique("vocabulary-dialog")).show(ctx, |ui| {
            ui.set_width(310.0);
            title(ui, t("Kosakata", "Vocabulary"));
            let limit = config::VOCABULARY_LIMIT;
            ui.label(if english {
                format!("Enter names or terms for Whisper and Qwen, one per line (max. {limit} characters).")
            } else {
                format!("Tulis nama atau istilah untuk Whisper dan Qwen, satu per baris (maks. {limit} karakter).")
            });
            egui::ScrollArea::vertical().max_height(175.0).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(draft).desired_width(f32::INFINITY).desired_rows(5).char_limit(config::VOCABULARY_LIMIT));
            });
            ui.horizontal(|ui| {
                if secondary(ui, true, t("Simpan", "Save")).clicked() {
                    save = true;
                    ui.close();
                }
                if secondary(ui, true, t("Batal", "Cancel")).clicked() { ui.close(); }
            });
        });
        if save {
            self.config.vocabulary = self.vocabulary_draft.take().unwrap_or_default().into();
            self.save_config();
            self.push_settings();
        } else if dialog.should_close() {
            self.vocabulary_draft = None;
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
        let result_height = 104.0;
        title(ui, t("Hasil terakhir", "Last result"));
        ui.add_space(4.0);
        inset(false)
            .stroke(egui::Stroke::new(
                1.0,
                egui::lerp(egui::Rgba::from(BORDER)..=egui::Rgba::from(ACCENT), flash),
            ))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical()
                    .id_salt("result")
                    .min_scrolled_height(result_height)
                    .max_height(result_height)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.set_min_height(result_height);
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
        ui.add_space(4.0);
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
                    const LABEL_SIZE: f32 = 10.0;
                    let text_width = |ui: &Ui, s: &str, color| {
                        ui.painter()
                            .layout_no_wrap(
                                s.to_owned(),
                                egui::FontId::proportional(LABEL_SIZE),
                                color,
                            )
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
                            ui.label(RichText::new(label).size(LABEL_SIZE).color(SECONDARY))
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
                        ui.label(RichText::new(text).size(LABEL_SIZE).color(SUCCESS));
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
        ui.horizontal(|ui| {
            ui.set_min_height(26.0);
            title(ui, t("Pengaturan", "Settings"));
        });
        self.focus_settings(ui);
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
                &self.icons.chevron,
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
                    &self.icons.chevron,
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
        ui.add_enabled_ui(idle, |ui| {
            changed |= settings_checkbox(
                ui,
                &self.icons.check,
                &mut self.config.hold_to_talk,
                t("Tahan shortcut untuk bicara", "Hold shortcut to talk"),
            )
            .changed();
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
            &self.icons.chevron,
            |ui| {
                ui.add_enabled_ui(tray, |ui| {
                    interactive(ui, |ui| {
                        ui.selectable_value(
                            &mut selected,
                            1,
                            t("Sembunyikan ke tray", "Hide to tray"),
                        )
                    });
                    interactive(ui, |ui| {
                        ui.selectable_value(
                            &mut selected,
                            2,
                            t(
                                "Sembunyikan ke tray, auto-startup",
                                "Hide to tray, start with Windows",
                            ),
                        )
                    });
                });
                interactive(ui, |ui| {
                    ui.selectable_value(
                        &mut selected,
                        0,
                        t("Keluar dari aplikasi", "Quit application"),
                    )
                });
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

    fn configure_focus(&mut self, ctx: &egui::Context) {
        self.focus = None;
        self.focus_error.clear();
        if self.config.focus_enabled {
            let ctx = ctx.clone();
            match focus::Focus::new(self.window.hwnd.load(Ordering::SeqCst), move || {
                ctx.request_repaint();
            }) {
                Ok(focus) => self.focus = Some(focus),
                Err(error) => self.focus_error = error.to_string(),
            }
        }
    }

    fn focus_settings(&mut self, ui: &mut Ui) {
        let english = self.config.language == Language::English;
        let t = |id, en| if english { en } else { id };
        if settings_checkbox(
            ui,
            &self.icons.check,
            &mut self.config.focus_enabled,
            t("Aktifkan fi-focus", "Enable fi-focus"),
        )
        .changed()
        {
            self.configure_focus(ui.ctx());
            self.save_config();
        }
        if let Some(focus) = &self.focus {
            let status = focus.status();
            let playing = status.playback == focus::Playback::Playing;
            field_label(ui, t("Jenis audio", "Audio type"));
            ui.horizontal(|ui| {
                combo(
                    ui,
                    egui::ComboBox::from_id_salt("focus-noise")
                        .width(ui.available_width() - 84.0)
                        .selected_text(status.noise.label()),
                    &self.icons.chevron,
                    |ui| {
                        for noise in focus::Noise::ALL {
                            if interactive(ui, |ui| {
                                ui.selectable_label(status.noise == noise, noise.label())
                            })
                            .clicked()
                            {
                                focus.send(focus::Command::Select(noise));
                            }
                        }
                    },
                );
                if action_button(
                    ui,
                    true,
                    false,
                    ui.available_width(),
                    if playing {
                        t("Jeda", "Pause")
                    } else {
                        t("Putar", "Play")
                    },
                )
                .clicked()
                {
                    focus.send(if playing {
                        focus::Command::Pause
                    } else {
                        focus::Command::Play
                    });
                }
            });
            let label = if playing {
                format!(
                    "{} · {}",
                    t("Sedang diputar", "Now playing"),
                    status.noise.label()
                )
            } else if status.playback == focus::Playback::Paused {
                t("Dijeda", "Paused").into()
            } else {
                t("Tidak ada pemutaran", "Nothing playing").into()
            };
            ui.label(RichText::new(label).small().color(MUTED));
            let audio_error = status.audio_error.map(|error| {
                use focus::OutputError::*;
                match error {
                    NoDevice => t(
                        "Tidak ada perangkat keluaran audio",
                        "No audio output device",
                    )
                    .into(),
                    Configuration => t(
                        "Gagal membaca konfigurasi perangkat",
                        "Could not read device configuration",
                    )
                    .into(),
                    UnsupportedFormat(format) => format!(
                        "{} ({format})",
                        t("Format audio tidak didukung", "Unsupported output format")
                    ),
                    Open => t(
                        "Gagal membuka perangkat audio",
                        "Could not open audio device",
                    )
                    .into(),
                    Play => t("Gagal memulai pemutaran", "Could not start playback").into(),
                    Unavailable => t(
                        "Perangkat audio tidak tersedia",
                        "Audio output device unavailable",
                    )
                    .into(),
                }
            });
            for (label, error) in [
                (t("Keluaran audio", "Audio output"), audio_error),
                (
                    t("Kontrol media Windows", "Windows media controls"),
                    status.media_error,
                ),
            ] {
                if let Some(error) = error {
                    ui.label(
                        RichText::new(format!("{label}: {error}"))
                            .small()
                            .color(WARN),
                    );
                }
            }
        }
        if !self.focus_error.is_empty() {
            ui.label(RichText::new(&self.focus_error).small().color(WARN));
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
        ui.horizontal(|ui| {
            ui.set_min_height(26.0);
            title(ui, "Model");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let back = interactive(ui, |ui| {
                    ui.add(
                        egui::Button::new(
                            RichText::new(t("Kembali", "Back")).size(12.5).color(TEXT),
                        )
                        .min_size(vec2(0.0, 26.0)),
                    )
                });
                if back.clicked() {
                    self.set_settings_open(false);
                }
            });
        });
        ui.add_enabled_ui(!phase.busy(), |ui| {
            field_label(ui, t("Mesin ASR", "ASR engine"));
            let previous = self.engine;
            let response = combo(
                ui,
                egui::ComboBox::from_id_salt("engine")
                    .width(ui.available_width())
                    .selected_text(self.engine.label()),
                &self.icons.chevron,
                |ui| {
                    for engine in Engine::ALL {
                        interactive(ui, |ui| {
                            ui.selectable_value(&mut self.engine, engine, engine.label())
                        });
                    }
                },
            );
            if previous != self.engine && (previous.is_cloud() || self.engine.is_cloud()) {
                self.form_changed = Some(std::time::Instant::now());
            }
            let fade = 1.0 - flash_amount(ui, self.form_changed, 0.15);
            ui.multiply_opacity(0.65 + 0.35 * fade);
            if self.engine.is_cloud() {
                if previous != self.engine {
                    self.config.engine = self.engine;
                    self.cloud_usage = None;
                    self.save_config();
                    self.activate_cloud();
                }
                if self.engine == Engine::DeepSeekAsr {
                    self.deepseek_settings(ui);
                } else {
                    self.cloud_settings(ui);
                }
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
                let button_width = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
                let label = if same {
                    t("Muat ulang", "Reload")
                } else {
                    t("Muat model", "Load model")
                };
                if action_button(
                    ui,
                    selected.is_some() && self.vad.is_some(),
                    !same,
                    button_width,
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
                        Engine::GoogleChirp3 | Engine::DeepSeekAsr => unreachable!(),
                    }
                    self.config.vad_model = Some(vad.clone());
                    self.save_config();
                    self.core.send(Cmd::Load {
                        engine: self.engine,
                        asr,
                        vad,
                    });
                }
                if action_button(ui, true, false, button_width, t("Kosakata", "Vocabulary"))
                    .clicked()
                {
                    self.vocabulary_draft = Some(self.config.vocabulary.as_str().to_owned());
                }
            });
            if phase == Phase::Loading {
                ui.label(
                    RichText::new(t("Memuat…", "Loading…"))
                        .small()
                        .color(SECONDARY),
                );
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
                        ui.add(
                            egui::Label::new(
                                RichText::new(line).monospace().size(11.0).color(SECONDARY),
                            )
                            .wrap(),
                        );
                    }
                })
                .header_response
            });
        }
    }

    fn activate_cloud(&self) {
        self.core.send(Cmd::Google(self.config.google.clone()));
        self.core.send(Cmd::DeepSeek(self.config.deepseek.clone()));
        self.core.send(Cmd::Load {
            engine: self.engine,
            asr: PathBuf::new(),
            vad: PathBuf::new(),
        });
    }

    fn cloud_usage_label(&mut self, ui: &mut Ui, key: &str, limit: u32, rate: f64) {
        let english = self.config.language == Language::English;
        if self
            .cloud_usage
            .as_ref()
            .is_none_or(|(at, k, _)| at.elapsed() >= Duration::from_secs(1) || k != key)
        {
            self.cloud_usage = Some((
                std::time::Instant::now(),
                key.to_owned(),
                cloud::usage_seconds(key),
            ));
        }
        ui.add_space(2.0);
        match self.cloud_usage.as_ref().unwrap().2.clone() {
            Ok(seconds) => {
                let minutes = seconds as f64 / 60.0;
                let near_limit = minutes >= f64::from(limit) * 0.8;
                ui.label(
                    RichText::new(if english {
                        format!(
                            "This month: {minutes:.1} min | est. US${:.2}",
                            minutes * rate
                        )
                    } else {
                        format!(
                            "Bulan ini: {minutes:.1} menit | estimasi US${:.2}",
                            minutes * rate
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
    }

    fn deepseek_settings(&mut self, ui: &mut Ui) {
        let english = self.config.language == Language::English;
        let t = |id, en| if english { en } else { id };
        field_label(ui, t("Batas / bulan", "Monthly limit"));
        let width = ui.available_width();
        let changed = interactive(ui, |ui| {
            ui.add_sized(
                [width, 34.0],
                egui::DragValue::new(&mut self.config.deepseek.monthly_limit_minutes)
                    .range(0..=60000)
                    .suffix(t(" menit", " min")),
            )
        })
        .changed();
        if changed {
            self.save_config();
            self.core.send(Cmd::DeepSeek(self.config.deepseek.clone()));
        }
        let limit = self.config.deepseek.monthly_limit_minutes;
        self.cloud_usage_label(ui, cloud::DEEPSEEK_USAGE_KEY, limit, 0.116 / 60.0);
        interactive(ui, |ui| {
            egui::CollapsingHeader::new(RichText::new("Setup API Key").small().color(SECONDARY))
                .id_salt("setup-api-key")
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(t(
                            "Jalankan di terminal, lalu mulai ulang aplikasi:",
                            "Run in a terminal, then restart the app:",
                        ))
                        .small()
                        .color(SECONDARY),
                    );
                    inset(false).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(
                            egui::Label::new(
                                RichText::new("setx DEEPSEEK_API_KEY \"API_KEY\"")
                                    .monospace()
                                    .size(11.5)
                                    .color(TEXT),
                            )
                            .selectable(true),
                        );
                    });
                    ui.label(
                        RichText::new(t(
                            "Atau isi deepseek.api_key di config.json. Penghitung lokal bukan tagihan DeepSeek. Bulan mengikuti UTC. Sisa minimal 2 menit untuk memulai sesi.",
                            "Or set deepseek.api_key in config.json. Local counter, not your DeepSeek bill. Months use UTC. At least 2 minutes must remain to start a session.",
                        ))
                        .small()
                        .color(SECONDARY),
                    );
                })
                .header_response
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
                    &self.icons.chevron,
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
        let project = self.config.google.project.clone();
        let limit = self.config.google.monthly_limit_minutes;
        self.cloud_usage_label(ui, &project, limit, 0.016);
        interactive(ui, |ui| {
            egui::CollapsingHeader::new(RichText::new("Setup ADC").small().color(SECONDARY))
                .icon({
                    let texture = self.icons.caret.id();
                    move |ui, openness, response| {
                        egui::Image::new((texture, vec2(10.0,10.0))).tint(SECONDARY)
                            .rotate(openness * std::f32::consts::FRAC_PI_2, egui::pos2(0.5,0.5).to_vec2())
                            .paint_at(ui, egui::Rect::from_center_size(response.rect.center(),vec2(10.0,10.0)));
                    }
                }).id_salt("setup-adc")
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
                !self.core.status().phase.busy(),
                t("Reset konfigurasi ke default", "Reset configuration"),
            )
            .clicked()
            {
                self.config = Config::default();
                self.configure_focus(ui.ctx());
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
        let status = self.core.status();
        if status.phase.busy() {
            return;
        }
        let result = hotkeys.set(&self.shortcut_input);
        drop(status);
        match result {
            Ok(()) => {
                self.shortcut_error.clear();
                self.config.shortcut = self.shortcut_input.trim().to_owned();
                self.save_config();
            }
            // Shortcut lama tetap terdaftar.
            Err(e) => self.shortcut_error = e,
        }
    }

    /// Baris utama: kartu Dikte melebar penuh saat merekam/memproses, lalu kembali dua kolom saat idle.
    fn main_row(&mut self, ui: &mut Ui) {
        let phase = self.core.status().phase;
        let expand = animate_bool_ease(
            ui.ctx(),
            egui::Id::unique("dictation-expand"),
            dictation_expanded(phase),
            CARD_ANIM,
        );
        let total_width = ui.available_width();
        let gap = ui.spacing().item_spacing.x;
        let layout = main_row_layout(total_width, gap, expand);
        let previous = self.rows[0];
        let target = previous[0].max(previous[1]).max(209.0);
        let mut heights = previous;
        let origin = ui.cursor().min;
        let max_height = ui.available_height().max(target + 36.0);

        let mut left_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("dictation-card")
                .max_rect(egui::Rect::from_min_size(
                    origin,
                    vec2(layout.dict_width, max_height),
                ))
                .layout(Layout::top_down(Align::Min)),
        );
        let left_frame = card().show(&mut left_ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(target);
            heights[0] = ui
                .with_layout(Layout::top_down(Align::Min), |ui| {
                    self.dictation_card(ui, layout.button_width);
                })
                .response
                .rect
                .height();
        });
        let mut row_height = left_frame.response.rect.height();

        let right_left = origin.x + layout.right_x_offset;
        let visible_right = (origin.x + total_width - right_left).max(0.0);
        if layout.result_alpha > 0.0 && visible_right > 1.0 {
            let mut right_ui = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("result-card")
                    .max_rect(egui::Rect::from_min_size(
                        egui::pos2(right_left, origin.y),
                        vec2(layout.col_width, max_height),
                    ))
                    .layout(Layout::top_down(Align::Min)),
            );
            // Kunci lebar layout kartu hasil dan potong dengan clip_rect agar teks tidak membungkus ulang saat meluncur.
            let clip = ui.clip_rect().intersect(egui::Rect::from_min_max(
                egui::pos2(right_left - 1.0, ui.clip_rect().top()),
                egui::pos2(origin.x + total_width + 1.0, ui.clip_rect().bottom()),
            ));
            right_ui.set_clip_rect(clip);
            right_ui.multiply_opacity(layout.result_alpha);
            let right_frame = card().show(&mut right_ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(target);
                let measured = ui
                    .with_layout(Layout::top_down(Align::Min), |ui| self.result_card(ui))
                    .response
                    .rect
                    .height();
                if expand == 0.0 {
                    heights[1] = measured;
                }
            });
            row_height = row_height.max(right_frame.response.rect.height());
        }

        ui.advance_cursor_after_rect(egui::Rect::from_min_size(
            origin,
            vec2(total_width, row_height),
        ));
        if heights != previous {
            self.rows[0] = heights;
            ui.ctx().request_discard("row height");
            ui.ctx().request_repaint();
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
        if index == 1 && ui.available_width() < 520.0 {
            for add in [left, right] {
                card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    add(self, ui);
                });
            }
            return;
        }
        let previous = self.rows[index];
        let target = previous[0]
            .max(previous[1])
            .max(if index == 0 { 209.0 } else { 316.0 });
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

fn dictation_expanded(phase: Phase) -> bool {
    matches!(
        phase,
        Phase::Starting | Phase::Recording | Phase::Transcribing
    )
}

struct MainRowLayout {
    col_width: f32,
    dict_width: f32,
    button_width: f32,
    right_x_offset: f32,
    result_alpha: f32,
}

fn main_row_layout(total_width: f32, gap: f32, expand: f32) -> MainRowLayout {
    let expand = expand.clamp(0.0, 1.0);
    let col_width = ((total_width - gap) / 2.0).max(0.0);
    let dict_width = egui::lerp(col_width..=total_width, expand);
    // 36 pt = margin kiri+kanan `card()` (18 pt × 2). Lebar tombol dikunci ke ukuran dua kolom
    // agar tombol tetap kompak di kiri saat kartu melebar penuh.
    let button_width = ((col_width - 36.0 - gap) / 2.0).max(0.0);
    let right_x_offset = dict_width + gap;
    let result_alpha = (1.0 - expand * 1.35).clamp(0.0, 1.0);
    MainRowLayout {
        col_width,
        dict_width,
        button_width,
        right_x_offset,
        result_alpha,
    }
}

fn settings_checkbox(
    ui: &mut Ui,
    check: &egui::TextureHandle,
    checked: &mut bool,
    label: &str,
) -> egui::Response {
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 22.0;
        ui.spacing_mut().icon_width = 16.0;
        ui.spacing_mut().icon_spacing = 8.0;

        let response = ui.checkbox(checked, label);
        let hover = animate_bool_ease(
            ui.ctx(),
            response.id.with("hover"),
            response.enabled() && response.hovered(),
            HOVER_ANIM,
        );
        let checked_t =
            animate_bool_ease(ui.ctx(), response.id.with("checked"), *checked, STATE_ANIM);

        let rect = egui::Rect::from_center_size(
            egui::pos2(response.rect.left() + 8.0, response.rect.center().y),
            vec2(16.0, 16.0),
        );
        let fill = mix(
            mix(INPUT, RAISED, 0.45 * hover),
            ACCENT_TINT.gamma_multiply(1.0 + 0.5 * hover),
            checked_t,
        );
        let border = mix(
            mix(MUTED, SECONDARY, hover),
            mix(ACCENT_BORDER, ACCENT, 0.65 + 0.35 * hover),
            checked_t,
        );
        ui.painter().rect(
            rect,
            4.0,
            fill,
            egui::Stroke::new(1.0, border),
            egui::StrokeKind::Inside,
        );
        if checked_t > 0.0 {
            let size = egui::lerp(8.0..=12.0, checked_t);
            egui::Image::new((check.id(), vec2(size, size)))
                .tint(ACCENT_HOVER.gamma_multiply(checked_t))
                .paint_at(
                    ui,
                    egui::Rect::from_center_size(rect.center(), vec2(size, size)),
                );
        }
        response
    })
    .inner
}

fn clamped_window_top(top: f32, height: f32, work_top: f32, work_bottom: f32) -> Option<f32> {
    if top + height > work_bottom {
        Some((work_bottom - height).max(work_top))
    } else {
        None
    }
}

fn drag_header(ui: &mut Ui) {
    let drag = ui.interact(
        ui.max_rect().expand2(vec2(16.0, 11.0)),
        ui.scope_id().with("drag"),
        egui::Sense::click_and_drag(),
    );
    if drag.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
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
        Phase::Loading | Phase::Starting | Phase::Transcribing => ACCENT,
    }
}

fn phase_label(phase: Phase, english: bool) -> &'static str {
    let t = |id, en| if english { en } else { id };
    match phase {
        Phase::CloudIdle | Phase::Ready => t("Siap", "Ready"),
        Phase::Recording => t("Merekam", "Recording"),
        Phase::Loading => t("Memuat", "Loading"),
        Phase::Starting => t("Menyiapkan mikrofon", "Starting microphone"),
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
        .inner_margin(egui::Margin::symmetric(13, 8))
        .show(ui, |ui| {
            ui.label(RichText::new(text).monospace().size(12.5).color(SECONDARY));
        })
        .response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Keyboard, true, text));
}

/// Indikator rekaman live (merah berdenyut), terpisah dari tombol utama.
fn rec_indicator(ui: &mut Ui) {
    let t = ui.input(|i| i.time) as f32;
    let pulse = 0.55 + 0.45 * (t * 4.0).sin().abs();
    let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.circle_filled(rect.center(), 7.0, DANGER.gamma_multiply(0.2 * pulse));
    painter.circle_filled(rect.center(), 4.0, DANGER.gamma_multiply(pulse));
    ui.label(RichText::new("REC").font(semibold(12.0)).color(DANGER));
    ui.ctx().request_repaint();
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
        let progress = (t.elapsed().as_secs_f32() / duration).clamp(0.0, 1.0);
        1.0 - egui::emath::easing::cubic_out(progress)
    });
    if value > 0.0 {
        ui.ctx().request_repaint();
    }
    value
}

fn dictation_notice(phase: Phase, error: &str, note: &str, english: bool) -> Option<String> {
    let text = match phase {
        Phase::Error if error.is_empty() => "Dikte gagal. Coba lagi.",
        Phase::Error => error,
        Phase::MissingModel => controller::MISSING_MODEL_NOTICE,
        Phase::Ready | Phase::CloudIdle if note == controller::NO_SPEECH_NOTICE => note,
        _ => return None,
    };
    Some(message(text, english))
}

fn dictation_feedback(ui: &mut Ui, phase: Phase, level: f32, notice: Option<&str>) {
    // Slot ini mencakup meter 4 pt dan ruang 18 pt di kedua sisinya.
    let (slot, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), 40.0), egui::Sense::hover());
    if let Some(text) = notice {
        let mut job = egui::text::LayoutJob::simple(
            text.to_owned(),
            egui::FontId::proportional(13.0),
            WARN,
            slot.width(),
        );
        job.wrap.max_rows = 2;
        let galley = ui.painter().layout_job(job);
        let pos = egui::pos2(slot.left(), slot.center().y - galley.size().y * 0.5);
        let elided = galley.elided;
        ui.painter().with_clip_rect(slot).galley(pos, galley, WARN);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Status, true, text));
        if elided {
            response.on_hover_text(text);
        }
        return;
    }
    let rect = egui::Rect::from_center_size(slot.center(), vec2(slot.width(), 4.0));
    let busy = matches!(
        phase,
        Phase::Loading | Phase::Starting | Phase::Transcribing
    );
    let fraction = meter_fraction(level);
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::new(egui::Role::Meter);
        info.label = Some("Level audio".into());
        info.value = Some((fraction * 100.0).round().into());
        info
    });
    ui.painter().rect_filled(rect, 3.0, BORDER);
    let fill = if busy {
        let offset = ((ui.input(|i| i.time) as f32 * 2.0).sin() + 1.0) * 0.4;
        egui::Rect::from_min_size(
            rect.min + vec2(rect.width() * offset, 0.0),
            vec2(rect.width() * 0.2, rect.height()),
        )
    } else {
        egui::Rect::from_min_size(rect.min, vec2(rect.width() * fraction, rect.height()))
    };
    if busy || (phase == Phase::Recording && fraction > 0.0) {
        ui.painter().rect_filled(fill, 3.0, ACCENT);
    }
    if busy {
        ui.ctx().request_repaint();
    }
}

#[cfg(test)]
mod dictation_feedback_tests;

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

    #[test]
    fn layout_constraints_and_card_expansion() {
        use super::{Phase, Pick, dictation_expanded, main_row_layout, model_field};
        use crate::theme::{self, action_button, card};
        use eframe::egui::{self, RichText, vec2};
        use std::path::PathBuf;

        for phase in [Phase::Starting, Phase::Recording, Phase::Transcribing] {
            assert!(dictation_expanded(phase), "{phase:?} harus melebar");
        }
        for phase in [
            Phase::MissingModel,
            Phase::CloudIdle,
            Phase::Loading,
            Phase::Ready,
            Phase::Error,
        ] {
            assert!(!dictation_expanded(phase), "{phase:?} harus dua kolom");
        }

        let idle = main_row_layout(588.0, 8.0, 0.0);
        let mid = main_row_layout(588.0, 8.0, 0.5);
        let full = main_row_layout(588.0, 8.0, 1.0);
        assert!((idle.dict_width - 290.0).abs() < 1e-4 && (full.dict_width - 588.0).abs() < 1e-4);
        assert!(mid.dict_width > idle.dict_width && mid.dict_width < full.dict_width);
        assert_eq!(mid.col_width, idle.col_width);
        assert_eq!(idle.result_alpha, 1.0);
        assert!(mid.result_alpha > 0.0 && mid.result_alpha < 1.0);
        assert_eq!(full.result_alpha, 0.0);
        assert_eq!(idle.button_width, full.button_width);

        let ctx = egui::Context::default();
        theme::install(&ctx);
        let col_width = 290.0;
        for english in [false, true] {
            let mut asr = Some(PathBuf::from("Qwen3-ASR-1.7B-hf"));
            let mut vad = Some(PathBuf::from("ggml-silero-v6.2.0.bin"));
            let (mut asr_ok, mut vad_ok) = (true, true);
            let mut card_width = 0.0;
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.allocate_ui(vec2(col_width, 600.0), |ui| {
                    card_width = card()
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            model_field(
                                ui,
                                if english {
                                    "Model folder"
                                } else {
                                    "Folder model"
                                },
                                Pick::Folder,
                                "Qwen3-ASR-1.7B-hf",
                                &mut asr,
                                &mut asr_ok,
                                english,
                            );
                            model_field(
                                ui,
                                "VAD (Silero)",
                                Pick::GgmlFile,
                                "ggml-silero-v6.2.0.bin",
                                &mut vad,
                                &mut vad_ok,
                                english,
                            );
                            ui.horizontal(|ui| {
                                let w = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
                                action_button(
                                    ui,
                                    true,
                                    false,
                                    w,
                                    if english { "Load model" } else { "Muat ulang" },
                                );
                                action_button(
                                    ui,
                                    true,
                                    false,
                                    w,
                                    if english { "Vocabulary" } else { "Kosakata" },
                                );
                            });
                            ui.label(
                                RichText::new(if english {
                                    "Loaded in 1.7 s"
                                } else {
                                    "Dimuat 1.7 s"
                                })
                                .small(),
                            );
                        })
                        .response
                        .rect
                        .width();
                });
            });
            assert!(
                card_width <= col_width + 0.5,
                "english={english}: lebar kartu {card_width} melebihi kolom {col_width}"
            );
        }
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
        let width = (ui.available_width() - 90.0).max(60.0);
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
            // Tunggu satu frame lengkap sebelum DWM menampilkan kembali jendela.
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
                self.settings_open = false;
                self.vocabulary_draft = None;
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
            self.settings_open = false;
            self.vocabulary_draft = None;
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
        self.screen(root);
        self.vocabulary_dialog(root.ctx());
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
                .with_inner_size([620.0, 338.0])
                .with_resizable(false)
                .with_maximize_button(false)
                .with_decorations(false)
                // Tray memakai SW_HIDE; taskbar tetap aktif saat jendela dibuka kembali.
                .with_taskbar(true)
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
    for &(id, en) in ui_messages() {
        text = text.replace(id, en);
    }
    text
}

fn ui_messages() -> &'static [(&'static str, &'static str)] {
    &[
        (
            "Tidak ada jendela tujuan; hasil hanya tampil di sini.",
            "No target window; results appear only here.",
        ),
        (
            "Lepaskan shortcut sebelum menggantinya.",
            "Release the shortcut before changing it.",
        ),
        ("Memasukkan teks…", "Inserting text…"),
        (
            controller::NO_SPEECH_NOTICE,
            "No speech detected. Try dictating again.",
        ),
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
            controller::MISSING_MODEL_NOTICE,
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
            "API key DeepSeek belum tersedia. Buka Setup API Key.",
            "DeepSeek API key unavailable. Open Setup API Key.",
        ),
        ("API key DeepSeek tidak valid.", "Invalid DeepSeek API key."),
        (
            "API key DeepSeek ditolak. Periksa DEEPSEEK_API_KEY.",
            "DeepSeek API key rejected. Check DEEPSEEK_API_KEY.",
        ),
        (
            "Saldo DeepSeek tidak mencukupi.",
            "Insufficient DeepSeek balance.",
        ),
        (
            "Format audio ditolak DeepSeek.",
            "Audio format rejected by DeepSeek.",
        ),
        (
            "Batas waktu DeepSeek terlampaui.",
            "DeepSeek request timed out.",
        ),
        (
            "Ukuran audio melebihi batas 8 MiB DeepSeek.",
            "Audio exceeds DeepSeek 8 MiB limit.",
        ),
        (
            "Terlalu banyak permintaan DeepSeek yang sedang berjalan.",
            "Too many DeepSeek requests in progress.",
        ),
        (
            "Layanan DeepSeek sedang sibuk. Coba lagi nanti.",
            "DeepSeek service is busy. Try again later.",
        ),
        (
            "DeepSeek tidak terhubung. Periksa internet.",
            "Cannot connect to DeepSeek. Check your internet.",
        ),
        (
            "Respons DeepSeek tidak valid.",
            "Invalid response from DeepSeek.",
        ),
        (
            "Transkripsi DeepSeek gagal. Periksa koneksi dan API key.",
            "DeepSeek transcription failed. Check your connection and API key.",
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
        (
            cloud::INVALID_PROJECT_ID,
            "Check your Google Cloud Project ID in Settings.",
        ),
        ("Dikte gagal. Coba lagi.", "Dictation failed. Try again."),
        (
            "Stream berakhir sebelum rekaman selesai.",
            "The stream ended before recording finished.",
        ),
        (
            "Mesin lokal melewati batas waktu. Tunggu mesin berhenti atau mulai ulang aplikasi.",
            "The local engine timed out. Wait for it to stop or restart the app.",
        ),
        (
            "Mesin lokal sudah berhenti.",
            "The local engine has stopped.",
        ),
        ("Mesin ASR berhenti", "The ASR engine stopped"),
        (
            "Muat ulang model di Pengaturan.",
            "Reload the model in Settings.",
        ),
        ("Model belum dimuat.", "The model has not been loaded."),
        ("Path model tidak valid:", "Invalid model path:"),
        (
            "File model VAD tidak ditemukan:",
            "VAD model file not found:",
        ),
        (
            "File model ASR tidak ditemukan:",
            "ASR model file not found:",
        ),
        (
            "Cloud tidak memakai loader lokal.",
            "Cloud engines cannot load local models.",
        ),
        ("Model VAD gagal dimuat", "Could not load the VAD model"),
        (
            "Gunakan model Silero GGML untuk whisper.cpp.",
            "Use a Silero GGML model for whisper.cpp.",
        ),
        ("Model ASR gagal dimuat", "Could not load the ASR model"),
        (
            "Pastikan file GGML whisper.cpp yang benar.",
            "Check the whisper.cpp GGML file.",
        ),
        ("State ASR gagal dibuat", "Could not initialize ASR"),
        (
            "memori GPU/RAM mungkin tidak cukup.",
            "GPU or system memory may be insufficient.",
        ),
        (
            "Folder model Qwen3-ASR tidak ditemukan:",
            "Qwen3-ASR model folder not found:",
        ),
        (
            "Folder Qwen3-ASR tidak lengkap:",
            "Incomplete Qwen3-ASR folder:",
        ),
        (
            "Model Qwen3-ASR gagal dimuat",
            "Could not load the Qwen3-ASR model",
        ),
        ("VAD gagal:", "Voice detection failed:"),
        ("Transkripsi gagal:", "Transcription failed:"),
        (
            "Thread audio gagal dibuat:",
            "Could not start the audio thread:",
        ),
        (
            "Mikrofon tidak merespons saat dibuka.",
            "The microphone did not respond.",
        ),
        (
            "Mikrofon tidak mengirim ekor rekaman tepat waktu; audio dibuang.",
            "The microphone timed out at the end of recording. Audio discarded.",
        ),
        (
            "Mikrofon berhenti mengirim audio; rekaman dibatalkan.",
            "The microphone stopped sending audio. Recording cancelled.",
        ),
        (
            "Timestamp mikrofon tidak valid; rekaman dibatalkan.",
            "Invalid microphone timestamp. Recording cancelled.",
        ),
        (
            "Buffer audio penuh; rekaman dihentikan agar tidak ada audio yang hilang diam-diam.",
            "The audio buffer filled up. Recording stopped to prevent audio loss.",
        ),
        (
            "Mikrofon pilihan tidak ditemukan. Pilih ulang di Pengaturan.",
            "Microphone not found. Select it again in Settings.",
        ),
        (
            "Tidak ada mikrofon default di sistem.",
            "No default microphone. Select an input in Settings.",
        ),
        (
            "Konfigurasi mikrofon tidak dapat dibaca",
            "Could not read the microphone configuration",
        ),
        (
            "Mikrofon melaporkan 0 channel.",
            "The microphone reported zero channels.",
        ),
        (
            "Format sampel mikrofon tidak didukung:",
            "Unsupported microphone sample format:",
        ),
        (
            "Mikrofon tidak dapat dimulai",
            "Could not start the microphone",
        ),
        (
            "Stream mikrofon gagal dibuat",
            "Could not open the microphone stream",
        ),
        ("Mikrofon bermasalah", "Microphone error"),
        (
            "Akses mikrofon ditolak. Izinkan aplikasi desktop memakai mikrofon di Pengaturan Privasi Windows.",
            "Microphone access denied. Allow desktop apps in Windows privacy settings.",
        ),
        (
            "Mikrofon tidak tersedia atau terlepas. Pilih ulang di Pengaturan.",
            "Microphone unavailable or disconnected. Select it again in Settings.",
        ),
        ("Resampler gagal dibuat:", "Could not create the resampler:"),
        (
            "Buffer resampling tidak valid:",
            "Invalid resampling buffer:",
        ),
        ("Resampling gagal:", "Audio resampling failed:"),
        (
            "Resampler streaming gagal dibuat.",
            "Could not create the streaming resampler.",
        ),
        ("Buffer PCM tidak valid.", "Invalid PCM buffer."),
        ("Resampling streaming gagal.", "Stream resampling failed."),
        (
            "Buffer cloud penuh atau koneksi terputus; rekaman dihentikan.",
            "Cloud buffer full or connection lost. Recording stopped.",
        ),
        ("Sesi cloud dibatalkan.", "Cloud session cancelled."),
        ("Batas waktu cloud terlampaui.", "Cloud session timed out."),
        (
            "Runtime cloud gagal dibuat.",
            "Could not start the cloud runtime.",
        ),
        (
            "Thread cloud gagal dibuat; reservasi penggunaan tetap dihitung.",
            "Could not start the cloud thread. Reserved usage still counts.",
        ),
        ("Lokasi ADC tidak tersedia.", "ADC location unavailable."),
        ("Pembacaan ADC gagal.", "Could not read ADC."),
        ("File ADC tidak valid.", "Invalid ADC file."),
        (
            "Gunakan ADC pengguna dari gcloud auth application-default login.",
            "Run gcloud auth application-default login to set up user ADC.",
        ),
        ("ADC tidak lengkap.", "Incomplete ADC credentials."),
        (
            "TLS autentikasi gagal dibuat.",
            "Could not initialize TLS for authentication.",
        ),
        (
            "Tidak dapat menghubungi Google untuk autentikasi.",
            "Could not reach Google for authentication.",
        ),
        (
            "Respons autentikasi tidak valid.",
            "Invalid authentication response.",
        ),
        ("Token ADC tidak tersedia.", "ADC token unavailable."),
        ("Endpoint tidak valid.", "Invalid endpoint."),
        ("TLS cloud gagal dibuat.", "Could not initialize cloud TLS."),
        ("Stream cloud tertutup.", "The cloud stream closed."),
        ("Token tidak valid.", "Invalid token."),
        ("Project ID tidak valid.", "Invalid project ID."),
        ("Recognizer tidak valid.", "Invalid recognizer."),
        (
            "Pengiriman audio terputus.",
            "Audio transmission interrupted.",
        ),
        (
            "Hasil final cloud melewati batas waktu.",
            "Timed out waiting for the final cloud result.",
        ),
        (
            "Stream cloud berhenti sebelum rekaman selesai.",
            "The cloud stream closed before recording finished.",
        ),
        (
            "Login diperlukan. Perbarui ADC.",
            "Sign in again to refresh ADC.",
        ),
        (
            "Periksa izin Speech Client, API, dan billing project.",
            "Check Speech Client permissions, API access, and project billing.",
        ),
        (
            "Kuota atau kapasitas Google Cloud habis.",
            "Google Cloud quota or capacity exhausted.",
        ),
        (
            "Konfigurasi audio, region, atau model ditolak Google.",
            "Google rejected the audio, region, or model configuration.",
        ),
        (
            "Batas waktu Google Cloud terlampaui.",
            "Google Cloud request timed out.",
        ),
        (
            "Transkripsi cloud gagal. Periksa koneksi dan konfigurasi project.",
            "Cloud transcription failed. Check your connection and project settings.",
        ),
        (
            "Lokasi penggunaan tidak tersedia.",
            "Usage counter location unavailable.",
        ),
        (
            "Penghitung penggunaan rusak; cloud diblokir.",
            "Usage counter corrupted. Cloud dictation blocked.",
        ),
        (
            "Penghitung penggunaan tidak dapat dibaca.",
            "Could not read the usage counter.",
        ),
        (
            "Penghitung penggunaan gagal disimpan; cloud diblokir.",
            "Could not save the usage counter. Cloud dictation blocked.",
        ),
    ]
}

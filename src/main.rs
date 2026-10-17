#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod config;
mod controller;
mod theme;
mod transcribe;
mod windows;

use config::{Config, Language};
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
}

struct App {
    core: Handle,
    window: Arc<Window>,
    settings: bool,
    config: Config,
    config_path: Option<PathBuf>,
    /// Error config rusak; selama ada, config tidak ditulis agar file lama tidak tertimpa.
    config_error: String,
    asr: Option<PathBuf>,
    vad: Option<PathBuf>,
    mics: Vec<audio::Mic>,
    default_mic: Option<String>,
    mic_error: String,
    shortcut_input: String,
    shortcut_error: String,
    hotkeys: Option<windows::Hotkeys>,
    tray: Option<windows::Tray>,
    tray_error: String,
    close_prompt: bool,
    level: f32,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install(&cc.egui_ctx);
        let window = Arc::new(Window {
            hwnd: AtomicIsize::new(0),
            quitting: AtomicBool::new(false),
        });
        if let Ok(handle) = cc.window_handle()
            && let RawWindowHandle::Win32(h) = handle.as_raw()
        {
            window.hwnd.store(h.hwnd.get(), Ordering::SeqCst);
        }

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
                windows::show_window(win.hwnd.load(Ordering::SeqCst));
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
            core,
            window,
            settings: false,
            asr: config
                .asr_model
                .clone()
                .or_else(|| config::default_model(config::ASR_FILE)),
            vad: config
                .vad_model
                .clone()
                .or_else(|| config::default_model(config::VAD_FILE)),
            shortcut_input: config.shortcut.clone(),
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
            close_prompt: false,
            level: 0.0,
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
        app.push_settings();
        if let (Some(asr), Some(vad)) = (app.asr.clone(), app.vad.clone()) {
            app.core.send(Cmd::Load { asr, vad });
        }
        app
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

    fn header(&mut self, ui: &mut Ui, phase: Phase) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("fi-whisper").font(semibold(25.0)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let color = match phase {
                    Phase::Error | Phase::Recording => DANGER,
                    Phase::MissingModel => WARN,
                    _ => ACCENT,
                };
                badge(ui, phase.label(), color);
            });
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.settings, false, "Dikte");
            ui.selectable_value(&mut self.settings, true, "Pengaturan");
        });
    }

    fn dictation(&mut self, ui: &mut Ui) {
        let st = self.core.status();
        let phase = st.phase;
        let recording = phase == Phase::Recording;
        let duration = st
            .recording_since
            .map_or(st.captured, |t| t.elapsed().min(audio::LIMIT));
        if let Some(level) = &st.level {
            // Peak meluruh perlahan agar meter terbaca.
            self.level = audio::take_level(level).max(self.level * 0.85);
        } else {
            self.level = 0.0;
        }
        let (note, error) = (st.note.clone(), st.error.clone());
        let (backend, timing) = (st.backend.clone(), st.timing);
        let ready = st.loaded.is_some();
        drop(st);

        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            title(ui, "Dikte");
            let shortcut = self
                .hotkeys
                .as_ref()
                .and_then(|h| h.active.as_ref())
                .map(|(_, s)| s.clone());
            ui.label(
                RichText::new(match &shortcut {
                    Some(s) => format!(
                        "Tekan {s} di aplikasi lain untuk dikte. Tombol di bawah hanya untuk uji."
                    ),
                    None => "Shortcut global tidak aktif. Tombol di bawah hanya untuk uji.".into(),
                })
                .color(MUTED),
            );
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
                    .color(if recording { ACCENT } else { TEXT }),
                );
                ui.label(RichText::new("/ 02:00").monospace().color(MUTED));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(if recording {
                            "Mikrofon aktif"
                        } else {
                            "Mikrofon tidak aktif"
                        })
                        .small()
                        .color(MUTED),
                    );
                });
            });
            meter(ui, recording, self.level);
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                let (enabled, label) = match phase {
                    Phase::Recording => (true, "Selesai"),
                    Phase::Transcribing => (false, "Memproses…"),
                    Phase::Loading => (false, "Memuat…"),
                    _ => (ready, "Mulai"),
                };
                if primary(ui, enabled, label).clicked() {
                    self.core.send(Cmd::Toggle(Source::Gui));
                }
                if matches!(phase, Phase::Recording | Phase::Transcribing)
                    && ui.button("Batalkan").clicked()
                {
                    self.core.send(Cmd::Cancel);
                }
                if matches!(phase, Phase::Loading | Phase::Transcribing) {
                    ui.spinner();
                }
            });
            ui.add_space(4.0);
            if !error.is_empty() {
                ui.label(RichText::new(&error).small().color(DANGER));
            }
            let hint = match phase {
                Phase::MissingModel | Phase::Error if !ready => {
                    "Pilih dan muat model di Pengaturan."
                }
                Phase::Recording => {
                    "Tekan Selesai, shortcut, atau Batalkan. Berhenti otomatis pada 2 menit."
                }
                _ => "",
            };
            for text in [note.as_str(), hint] {
                if !text.is_empty() {
                    ui.label(RichText::new(text).small().color(MUTED));
                }
            }
        });
        ui.add_space(8.0);
        self.result_ui(ui);
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            badge(
                ui,
                &format!("Backend: {backend}"),
                if ready { ACCENT } else { MUTED },
            );
            let text = timing.map_or("Proses terakhir: —".into(), |(t, total)| {
                format!(
                    "Audio {:.1} dtk · VAD {:.2} · ASR {:.2} · total {:.2} dtk",
                    t.audio.as_secs_f32(),
                    t.vad.as_secs_f32(),
                    t.asr.as_secs_f32(),
                    total.as_secs_f32()
                )
            });
            ui.label(RichText::new(text).small().color(MUTED));
        });
    }

    fn result_ui(&mut self, ui: &mut Ui) {
        let (result, result_note) = {
            let st = self.core.status();
            (st.result.clone(), st.result_note.clone())
        };
        let mut copy_note = None;
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                title(ui, "Hasil terakhir");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add_enabled(!result.is_empty(), egui::Button::new("Hapus"))
                        .clicked()
                    {
                        self.core.send(Cmd::ClearResult);
                    }
                    if ui
                        .add_enabled(!result.is_empty(), egui::Button::new("Salin"))
                        .clicked()
                    {
                        copy_note = Some(
                            match windows::copy_text(
                                self.window.hwnd.load(Ordering::SeqCst),
                                &result,
                            ) {
                                Ok(()) => {
                                    "Teks disalin ke clipboard (dikecualikan dari riwayat Windows)."
                                        .to_owned()
                                }
                                Err(e) => e,
                            },
                        );
                    }
                });
            });
            ui.add_space(4.0);
            egui::Frame::new()
                .fill(INPUT)
                .corner_radius(8)
                .inner_margin(14)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(70.0);
                    if result.is_empty() {
                        ui.label(
                            RichText::new("Hasil dikte terakhir akan muncul di sini.").color(MUTED),
                        );
                    } else {
                        ui.add(egui::Label::new(&result).selectable(true).wrap());
                    }
                });
            if !result_note.is_empty() {
                ui.label(RichText::new(&result_note).small().color(ACCENT));
            }
            ui.label(
                RichText::new(
                    "Hasil hanya disimpan di RAM sampai diganti, dihapus, atau aplikasi keluar.",
                )
                .small()
                .color(MUTED),
            );
        });
        if let Some(note) = copy_note {
            self.core.status().result_note = note;
        }
    }

    fn settings_ui(&mut self, ui: &mut Ui) {
        let (phase, backend, diagnostics, loaded, load_time) = {
            let st = self.core.status();
            (
                st.phase,
                st.backend.clone(),
                st.diagnostics.clone(),
                st.loaded.clone(),
                st.load_time,
            )
        };
        let idle = !phase.busy();
        if !self.config_error.is_empty() {
            card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(&self.config_error).small().color(WARN));
                if ui.button("Reset konfigurasi ke default").clicked() {
                    self.config = Config::default();
                    self.config_error.clear();
                    self.save_config();
                    self.shortcut_input = self.config.shortcut.clone();
                    self.push_settings();
                }
            });
            ui.add_space(8.0);
        }
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            title(ui, "Input & bahasa");
            ui.add_space(4.0);
            field_label(ui, "Mikrofon");
            let mut changed = false;
            ui.add_enabled_ui(idle, |ui| {
                ui.horizontal(|ui| {
                    let selected = match &self.config.microphone {
                        None => {
                            let name = self
                                .default_mic
                                .as_ref()
                                .and_then(|id| self.mics.iter().find(|m| &m.id == id))
                                .map_or("tidak ada", |m| m.name.as_str());
                            format!("Default sistem · {name}")
                        }
                        Some(id) => self
                            .mics
                            .iter()
                            .find(|m| &m.id == id)
                            .map_or_else(|| "Perangkat tidak ditemukan".into(), |m| m.name.clone()),
                    };
                    egui::ComboBox::from_id_salt("microphone")
                        .width((ui.available_width() - 110.0).max(200.0))
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            changed |= ui
                                .selectable_value(
                                    &mut self.config.microphone,
                                    None,
                                    "Default sistem",
                                )
                                .changed();
                            for mic in &self.mics {
                                changed |= ui
                                    .selectable_value(
                                        &mut self.config.microphone,
                                        Some(mic.id.clone()),
                                        &mic.name,
                                    )
                                    .changed();
                            }
                        });
                    if ui.button("Muat ulang").clicked() {
                        self.refresh_mics();
                    }
                });
            });
            if !self.mic_error.is_empty() {
                ui.label(RichText::new(&self.mic_error).small().color(WARN));
            }
            field_label(ui, "Bahasa transkripsi");
            ui.add_enabled_ui(idle, |ui| {
                egui::ComboBox::from_id_salt("language")
                    .width(220.0)
                    .selected_text(self.config.language.label())
                    .show_ui(ui, |ui| {
                        for language in Language::ALL {
                            changed |= ui
                                .selectable_value(
                                    &mut self.config.language,
                                    language,
                                    language.label(),
                                )
                                .changed();
                        }
                    });
            });
            if changed {
                self.refresh_mics();
                self.save_config();
                self.push_settings();
            }
            field_label(ui, "Shortcut mulai/selesai");
            ui.add_enabled_ui(idle, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.shortcut_input)
                            .desired_width(250.0)
                            .hint_text("Ctrl+Alt+Space"),
                    );
                    if ui.button("Terapkan").clicked() {
                        self.apply_shortcut();
                    }
                });
            });
            let active = self
                .hotkeys
                .as_ref()
                .and_then(|h| h.active.as_ref())
                .map(|(_, s)| s.as_str());
            ui.label(
                RichText::new(match active {
                    Some(s) => format!("Aktif: {s}"),
                    None => "Shortcut global tidak aktif.".into(),
                })
                .small()
                .color(MUTED),
            );
            if !self.shortcut_error.is_empty() {
                ui.label(RichText::new(&self.shortcut_error).small().color(WARN));
            }
        });
        ui.add_space(8.0);
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            title(ui, "Model lokal");
            ui.label(
                RichText::new("Pilih file GGML whisper.cpp dari komputer Anda.")
                    .small()
                    .color(MUTED),
            );
            ui.add_enabled_ui(idle, |ui| {
                model_field(
                    ui,
                    "Model ASR",
                    "Whisper large-v3-turbo · ggml-large-v3-turbo.bin",
                    &mut self.asr,
                );
                model_field(
                    ui,
                    "Model VAD",
                    "Silero VAD · ggml-silero-v6.2.0.bin",
                    &mut self.vad,
                );
                ui.add_space(4.0);
                let same = loaded.as_ref().is_some_and(|(a, v)| {
                    Some(a) == self.asr.as_ref() && Some(v) == self.vad.as_ref()
                });
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            self.asr.is_some() && self.vad.is_some(),
                            egui::Button::new(if same {
                                "Muat ulang model"
                            } else {
                                "Muat model"
                            }),
                        )
                        .clicked()
                        && let (Some(asr), Some(vad)) = (self.asr.clone(), self.vad.clone())
                    {
                        self.config.asr_model = Some(asr.clone());
                        self.config.vad_model = Some(vad.clone());
                        self.save_config();
                        self.core.send(Cmd::Load { asr, vad });
                    }
                    if phase == Phase::Loading {
                        ui.spinner();
                    }
                });
            });
            let status = match (&loaded, load_time) {
                (Some((a, v)), Some(t)) => format!(
                    "Aktif: {} + {} · dimuat {:.1} dtk",
                    file_name(a),
                    file_name(v),
                    t.as_secs_f32()
                ),
                _ if phase == Phase::Loading => "Memuat model…".into(),
                _ => "Belum ada model aktif.".into(),
            };
            ui.label(RichText::new(status).small().color(MUTED));
            ui.label(
                RichText::new(format!("Backend aktual: {backend}"))
                    .small()
                    .color(MUTED),
            );
            if !diagnostics.is_empty() {
                egui::CollapsingHeader::new("Diagnostik backend")
                    .id_salt("diagnostics")
                    .show(ui, |ui| {
                        for line in &diagnostics {
                            ui.label(RichText::new(line).monospace().size(11.5).color(MUTED));
                        }
                    });
            }
        });
        ui.add_space(8.0);
        let path = self
            .config_path
            .as_ref()
            .map_or("—".into(), |p| p.display().to_string());
        ui.label(
            RichText::new(format!("Konfigurasi: {path}"))
                .small()
                .color(MUTED),
        );
        ui.label(
            RichText::new(if self.tray.is_some() {
                "Menutup jendela menyembunyikan aplikasi ke tray. Pilih Keluar di menu tray untuk berhenti."
            } else {
                "Tray tidak tersedia; menutup jendela akan keluar dari aplikasi."
            })
            .small()
            .color(MUTED),
        );
        if !self.tray_error.is_empty() {
            ui.label(RichText::new(&self.tray_error).small().color(WARN));
        }
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

    fn hide(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    fn close_prompt_ui(&mut self, ctx: &egui::Context) {
        if !self.close_prompt {
            return;
        }
        let mut choice = None;
        egui::Modal::new(egui::Id::unique("close-hint")).show(ctx, |ui| {
            ui.set_width(380.0);
            title(ui, "fi-whisper tetap berjalan");
            ui.label("Jendela akan disembunyikan ke tray. Shortcut tetap aktif. Pilih Keluar di menu tray untuk menghentikan aplikasi.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if primary(ui, true, "Sembunyikan").clicked() {
                    choice = Some(false);
                }
                if ui.button("Keluar").clicked() {
                    choice = Some(true);
                }
            });
        });
        if let Some(quit) = choice {
            self.close_prompt = false;
            self.config.close_hint_shown = true;
            self.save_config();
            if quit {
                self.window.quitting.store(true, Ordering::SeqCst);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                self.hide(ctx);
            }
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn meter(ui: &mut Ui, active: bool, level: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), egui::Sense::hover());
    let count = 64;
    let step = rect.width() / count as f32;
    // Skala log sederhana: -60 dBFS..0 dBFS.
    let db = if level > 0.0 {
        20.0 * level.log10()
    } else {
        -60.0
    };
    let filled = (((db + 60.0) / 60.0).clamp(0.0, 1.0) * count as f32) as usize;
    for i in 0..count {
        let on = active && i < filled;
        let bar = egui::Rect::from_center_size(
            egui::pos2(rect.left() + (i as f32 + 0.5) * step, rect.center().y),
            vec2((step - 3.0).max(1.0), if on { 28.0 } else { 6.0 }),
        );
        ui.painter()
            .rect_filled(bar, 2.0, if on { ACCENT } else { BORDER });
    }
}

fn model_field(ui: &mut Ui, label: &str, description: &str, path: &mut Option<PathBuf>) {
    field_label(ui, label);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - 110.0).max(150.0));
            ui.label(
                path.as_deref()
                    .map_or_else(|| "Pilih file model".into(), file_name),
            );
            let detail = path
                .as_ref()
                .map_or_else(|| description.to_owned(), |p| p.display().to_string());
            ui.add(egui::Label::new(RichText::new(detail).small().color(MUTED)).wrap());
        });
        if ui
            .button(format!(
                "Pilih {}",
                label.strip_prefix("Model ").unwrap_or(label)
            ))
            .clicked()
            && let Some(selected) = rfd::FileDialog::new()
                .set_title(label)
                .add_filter("Model GGML", &["bin"])
                .pick_file()
        {
            *path = Some(selected);
        }
    });
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.window.quitting.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if ctx.input(|i| i.viewport().close_requested()) && self.tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.config.close_hint_shown {
                self.hide(ctx);
            } else {
                self.close_prompt = true;
            }
        }
        if self.core.status().phase.busy() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    fn ui(&mut self, root: &mut Ui, _frame: &mut eframe::Frame) {
        let phase = self.core.status().phase;
        egui::Panel::top("header")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(24, 18)),
            )
            .show(root, |ui| self.header(ui, phase));
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(24, 14)),
            )
            .show(root, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(if self.settings {
                        "settings"
                    } else {
                        "dictation"
                    })
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if self.settings {
                            self.settings_ui(ui);
                        } else {
                            self.dictation(ui);
                        }
                    });
            });
        let ctx = root.ctx().clone();
        self.close_prompt_ui(&ctx);
    }
}

fn main() -> eframe::Result {
    let Some(_instance) = windows::single_instance() else {
        windows::message("fi-whisper sudah berjalan. Buka melalui ikon tray.");
        return Ok(());
    };
    eframe::run_native(
        "fi-whisper",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([860.0, 820.0])
                .with_min_inner_size([620.0, 540.0]),
            renderer: eframe::Renderer::Glow,
            centered: true,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

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
    level: f32,
    /// Tinggi alami isi kartu per baris/kolom dari frame sebelumnya, agar kartu sebaris sama tinggi.
    rows: [[f32; 2]; 2],
    /// Tinggi kosong di bawah kartu (frame sebelumnya) yang dibagikan ke baris.
    extra: f32,
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
            level: 0.0,
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

    fn shortcut(&self) -> Option<&str> {
        self.hotkeys
            .as_ref()
            .and_then(|h| h.active.as_ref())
            .map(|(_, s)| s.as_str())
    }

    fn header(&mut self, ui: &mut Ui) {
        let (phase, backend, ready) = {
            let st = self.core.status();
            (st.phase, st.backend.clone(), st.loaded.is_some())
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new("fi-whisper").font(semibold(22.0)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let color = match phase {
                    Phase::Error | Phase::Recording => DANGER,
                    Phase::MissingModel => WARN,
                    _ => ACCENT,
                };
                badge(ui, phase.label(), color);
                if ready {
                    badge(ui, &backend, MUTED);
                }
            });
        });
    }

    fn dictation_card(&mut self, ui: &mut Ui) {
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
        let ready = st.loaded.is_some();
        drop(st);

        ui.horizontal(|ui| {
            title(ui, "Dikte");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let (text, color) = match self.shortcut() {
                    Some(s) => (s.to_owned(), MUTED),
                    None => ("Shortcut tidak aktif".to_owned(), WARN),
                };
                ui.label(RichText::new(text).small().color(color));
            });
        });
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "{:02}:{:02}",
                    duration.as_secs() / 60,
                    duration.as_secs() % 60
                ))
                .monospace()
                .size(34.0)
                .color(if recording { ACCENT } else { TEXT }),
            );
            ui.label(RichText::new("/ 02:00").monospace().color(MUTED));
        });
        meter(ui, recording, self.level);
        ui.horizontal(|ui| {
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
        });
        if !error.is_empty() {
            ui.label(RichText::new(&error).small().color(DANGER));
        }
        if !note.is_empty() {
            ui.label(RichText::new(&note).small().color(MUTED));
        }
        if !ready && matches!(phase, Phase::MissingModel | Phase::Error) {
            ui.label(
                RichText::new("Pilih model ASR dan VAD, lalu tekan Muat model.")
                    .small()
                    .color(MUTED),
            );
        }
    }

    fn result_card(&mut self, ui: &mut Ui) {
        let (result, result_note, timing) = {
            let st = self.core.status();
            (st.result.clone(), st.result_note.clone(), st.timing)
        };
        title(ui, "Hasil terakhir");
        egui::Frame::new()
            .fill(INPUT)
            .corner_radius(8)
            .inner_margin(10)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical()
                    .id_salt("result")
                    .min_scrolled_height(120.0)
                    .max_height(120.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.set_min_height(120.0);
                        if result.is_empty() {
                            ui.label(RichText::new("Belum ada hasil.").color(MUTED));
                        } else {
                            ui.add(egui::Label::new(&result).selectable(true).wrap());
                        }
                    });
            });
        let mut copy_note = None;
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!result.is_empty(), egui::Button::new("Salin"))
                .clicked()
            {
                copy_note = Some(
                    match windows::copy_text(self.window.hwnd.load(Ordering::SeqCst), &result) {
                        Ok(()) => "Disalin ke clipboard.".to_owned(),
                        Err(e) => e,
                    },
                );
            }
            if ui
                .add_enabled(!result.is_empty(), egui::Button::new("Hapus"))
                .clicked()
            {
                self.core.send(Cmd::ClearResult);
            }
        });
        if let Some((t, total)) = timing {
            ui.label(
                RichText::new(format!(
                    "Audio {:.1} dtk · proses {:.2} dtk",
                    t.audio.as_secs_f32(),
                    total.as_secs_f32()
                ))
                .small()
                .color(MUTED),
            )
            .on_hover_text(format!(
                "VAD {:.2} dtk · ASR {:.2} dtk",
                t.vad.as_secs_f32(),
                t.asr.as_secs_f32()
            ));
        }
        if !result_note.is_empty() {
            ui.label(RichText::new(&result_note).small().color(ACCENT));
        }
        if let Some(note) = copy_note {
            self.core.status().result_note = note;
        }
    }

    fn input_card(&mut self, ui: &mut Ui) {
        let idle = !self.core.status().phase.busy();
        title(ui, "Pengaturan");
        let width = ui.available_width();
        let mut changed = false;
        ui.add_enabled_ui(idle, |ui| {
            field_label(ui, "Mikrofon");
            let selected = match &self.config.microphone {
                None => {
                    let name = self
                        .default_mic
                        .as_ref()
                        .and_then(|id| self.mics.iter().find(|m| &m.id == id))
                        .map_or("tidak ada", |m| m.name.as_str());
                    format!("Default · {name}")
                }
                Some(id) => self
                    .mics
                    .iter()
                    .find(|m| &m.id == id)
                    .map_or_else(|| "Perangkat tidak ditemukan".into(), |m| m.name.clone()),
            };
            let combo = egui::ComboBox::from_id_salt("microphone")
                .width(width)
                .truncate()
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    changed |= ui
                        .selectable_value(&mut self.config.microphone, None, "Default sistem")
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
            // Daftar perangkat diperbarui setiap kali dropdown dibuka.
            if combo.response.clicked() {
                self.refresh_mics();
            }
            field_label(ui, "Bahasa");
            egui::ComboBox::from_id_salt("language")
                .width(width)
                .selected_text(self.config.language.label())
                .show_ui(ui, |ui| {
                    for language in Language::ALL {
                        changed |= ui
                            .selectable_value(&mut self.config.language, language, language.label())
                            .changed();
                    }
                });
            field_label(ui, "Shortcut");
            ui.horizontal(|ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.shortcut_input)
                        .desired_width(ui.available_width() - 104.0)
                        .hint_text("Ctrl+Alt+Space"),
                );
                let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("Terapkan").clicked() || enter {
                    self.apply_shortcut();
                }
            });
        });
        if !self.mic_error.is_empty() {
            ui.label(RichText::new(&self.mic_error).small().color(WARN));
        }
        if !self.shortcut_error.is_empty() {
            ui.label(RichText::new(&self.shortcut_error).small().color(WARN));
        }
        if changed {
            self.refresh_mics();
            self.save_config();
            self.push_settings();
        }

        field_label(ui, "Tombol close");
        let tray = self.tray.is_some();
        let mut close_changed = false;
        egui::ComboBox::from_id_salt("close-action")
            .width(width)
            .selected_text(if self.config.close_to_tray && tray {
                "Sembunyikan ke tray"
            } else {
                "Keluar dari aplikasi"
            })
            .show_ui(ui, |ui| {
                ui.add_enabled_ui(tray, |ui| {
                    close_changed |= ui
                        .selectable_value(
                            &mut self.config.close_to_tray,
                            true,
                            "Sembunyikan ke tray",
                        )
                        .changed();
                });
                close_changed |= ui
                    .selectable_value(
                        &mut self.config.close_to_tray,
                        false,
                        "Keluar dari aplikasi",
                    )
                    .changed();
            });
        if close_changed {
            self.save_config();
        }
        if !self.tray_error.is_empty() {
            ui.label(RichText::new(&self.tray_error).small().color(WARN));
        }
    }

    fn model_card(&mut self, ui: &mut Ui) {
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
            model_field(ui, "ASR", &mut self.asr);
            model_field(ui, "VAD", &mut self.vad);
            let same = loaded
                .as_ref()
                .is_some_and(|(a, v)| Some(a) == self.asr.as_ref() && Some(v) == self.vad.as_ref());
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.asr.is_some() && self.vad.is_some(),
                        egui::Button::new(if same { "Muat ulang" } else { "Muat model" }),
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
                } else if let (true, Some(t)) = (same, load_time) {
                    ui.label(
                        RichText::new(format!("Dimuat {:.1} dtk", t.as_secs_f32()))
                            .small()
                            .color(MUTED),
                    );
                }
            });
        });
        if !diagnostics.is_empty() {
            egui::CollapsingHeader::new(RichText::new("Diagnostik").small().color(MUTED))
                .id_salt("diagnostics")
                .show(ui, |ui| {
                    for line in &diagnostics {
                        ui.label(RichText::new(line).monospace().size(11.0).color(MUTED));
                    }
                });
        }
    }

    fn config_error_card(&mut self, ui: &mut Ui) {
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
        // Sisa tinggi jendela dibagi rata ke kedua baris agar kartu mengisi sampai bawah.
        let target = previous[0].max(previous[1]) + self.extra / 2.0;
        let mut heights = [0.0; 2];
        ui.columns_const(|[l, r]: &mut [Ui; 2]| {
            for (col, (ui, add)) in [(l, left), (r, right)].into_iter().enumerate() {
                card().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    // Harus sebelum isi: set_min_height di egui dihitung dari kursor saat ini.
                    ui.set_min_height(target);
                    // Tinggi scope = tinggi isi saja, tidak terpengaruh set_min_height.
                    heights[col] = ui.scope(|ui| add(self, ui)).response.rect.height();
                });
            }
        });
        if heights != previous {
            self.rows[index] = heights;
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

fn meter(ui: &mut Ui, active: bool, level: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), egui::Sense::hover());
    let count = ((rect.width() / 6.0) as usize).max(8);
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
            vec2((step - 3.0).max(1.0), if on { 24.0 } else { 6.0 }),
        );
        ui.painter()
            .rect_filled(bar, 2.0, if on { ACCENT } else { BORDER });
    }
}

fn model_field(ui: &mut Ui, label: &str, path: &mut Option<PathBuf>) {
    field_label(ui, label);
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 64.0).max(60.0);
        ui.allocate_ui_with_layout(
            vec2(width, ui.spacing().interact_size.y),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_min_width(width);
                let name = path
                    .as_deref()
                    .map_or_else(|| "Belum dipilih".into(), file_name);
                let response = ui.add(
                    egui::Label::new(RichText::new(name).color(if path.is_some() {
                        TEXT
                    } else {
                        MUTED
                    }))
                    .truncate(),
                );
                if let Some(p) = path.as_deref() {
                    response.on_hover_text(p.display().to_string());
                }
            },
        );
        if ui.button("Pilih").clicked()
            && let Some(selected) = rfd::FileDialog::new()
                .set_title(format!("Model {label}"))
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
        if ctx.input(|i| i.viewport().close_requested())
            && self.config.close_to_tray
            && self.tray.is_some()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            windows::hide_window(self.window.hwnd.load(Ordering::SeqCst));
        }
        if self.core.status().phase.busy() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    fn ui(&mut self, root: &mut Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("header")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(16, 12)),
            )
            .show(root, |ui| self.header(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(16))
            .show(root, |ui| {
                let available = ui.available_height();
                // Cadangan bila jendela diperkecil atau pesan error panjang membuat isi melebihi tinggi.
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if !self.config_error.is_empty() {
                        self.config_error_card(ui);
                    }
                    self.row(ui, 0, Self::dictation_card, Self::result_card);
                    self.row(ui, 1, Self::input_card, Self::model_card);
                    // Koreksi linear dari frame ini; stabil setelah satu frame tambahan.
                    // Dibulatkan ke bawah agar pembulatan piksel tidak memunculkan scrollbar.
                    let used = ui.min_rect().height();
                    let extra = (self.extra + available - used - 1.0).max(0.0).floor();
                    if (extra - self.extra).abs() >= 1.0 {
                        self.extra = extra;
                        ui.ctx().request_repaint();
                    }
                });
            });
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
                .with_inner_size([620.0, 730.0])
                .with_min_inner_size([560.0, 600.0]),
            renderer: eframe::Renderer::Glow,
            centered: true,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
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
}

struct App {
    core: Handle,
    logo: egui::TextureHandle,
    icons: theme::Icons,
    window: Arc<Window>,
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
            logo: theme::logo_texture(&cc.egui_ctx),
            icons: theme::Icons::load(&cc.egui_ctx),
            window,
            engine: config.engine,
            asr: config
                .asr_model
                .clone()
                .or_else(|| config::default_model(config::ASR_DIR, config::ASR_FILE)),
            qwen: config
                .qwen_model
                .clone()
                .or_else(|| config::default_model_dir(config::QWEN_DIR)),
            qwen17: config
                .qwen17_model
                .clone()
                .or_else(|| config::default_model_dir(config::QWEN17_DIR)),
            vad: config
                .vad_model
                .clone()
                .or_else(|| config::default_model(config::VAD_DIR, config::VAD_FILE)),
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
        if let (Some(asr), Some(vad)) = (app.selected_asr(), app.vad.clone()) {
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
        }
    }

    fn asr_path_mut(&mut self, engine: Engine) -> &mut Option<PathBuf> {
        match engine {
            Engine::Whisper => &mut self.asr,
            Engine::Qwen3Asr => &mut self.qwen,
            Engine::Qwen3Asr17 => &mut self.qwen17,
        }
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
        let (phase, backend, loaded) = {
            let st = self.core.status();
            (
                st.phase,
                st.backend.clone(),
                st.loaded.as_ref().map(|l| l.0),
            )
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.add(egui::Image::new((self.logo.id(), vec2(30.0, 30.0))));
            ui.label(RichText::new("fi-whisper").font(semibold(21.0)).color(TEXT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                link_badge(ui, &self.icons.home, None, LINK_HOME, HOME_URL);
                link_badge(
                    ui,
                    &self.icons.github,
                    Some(concat!("v", env!("CARGO_PKG_VERSION"))),
                    ACCENT,
                    REPO_URL,
                );
                badge(ui, phase.label(), phase_color(phase));
                if let Some(engine) = loaded {
                    // Lewati badge mesin bila jendela terlalu sempit agar tidak menimpa judul.
                    let text = format!("{} · {backend}", engine.short());
                    let width = ui
                        .painter()
                        .layout_no_wrap(text.clone(), medium(12.0), SECONDARY)
                        .size()
                        .x
                        + 28.0;
                    if ui.available_width() >= width {
                        badge(ui, &text, SECONDARY);
                    }
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
                match self.shortcut() {
                    Some(s) => keycap(ui, s),
                    None => {
                        ui.label(RichText::new("Shortcut tidak aktif").small().color(WARN));
                    }
                }
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
        meter(ui, recording, self.level);
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let (enabled, label) = match phase {
                Phase::Recording => (true, "Selesai"),
                Phase::Transcribing => (false, "Memproses…"),
                Phase::Loading => (false, "Memuat model…"),
                _ => (ready, "Mulai dikte"),
            };
            if primary(ui, enabled, recording, label).clicked() {
                self.core.send(Cmd::Toggle(Source::Gui));
            }
            if matches!(phase, Phase::Recording | Phase::Transcribing)
                && secondary(ui, true, "Batalkan").clicked()
            {
                self.core.send(Cmd::Cancel);
            }
        });
        if !error.is_empty() {
            ui.label(RichText::new(&error).small().color(DANGER));
        }
        if !note.is_empty() {
            ui.label(RichText::new(&note).small().color(SECONDARY));
        }
        if !ready && matches!(phase, Phase::MissingModel | Phase::Error) {
            ui.label(
                RichText::new("Pilih model ASR dan VAD, lalu tekan Muat model.")
                    .small()
                    .color(SECONDARY),
            );
        }
    }

    fn result_card(&mut self, ui: &mut Ui) {
        let (result, result_note, timing) = {
            let st = self.core.status();
            (st.result.clone(), st.result_note.clone(), st.timing)
        };
        title(ui, "Hasil terakhir");
        inset(false).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical()
                .id_salt("result")
                .min_scrolled_height(120.0)
                .max_height(120.0)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(120.0);
                    if result.is_empty() {
                        ui.label(RichText::new("Hasil dikte akan muncul di sini.").color(MUTED));
                    } else {
                        ui.add(
                            egui::Label::new(RichText::new(&result).size(15.0).color(TEXT))
                                .selectable(true)
                                .wrap(),
                        );
                    }
                });
        });
        let mut copy_note = None;
        ui.horizontal(|ui| {
            if secondary(ui, !result.is_empty(), "Salin").clicked() {
                copy_note = Some(
                    match windows::copy_text(self.window.hwnd.load(Ordering::SeqCst), &result) {
                        Ok(()) => COPIED.to_owned(),
                        Err(e) => e,
                    },
                );
            }
            if secondary(ui, !result.is_empty(), "Hapus").clicked() {
                self.core.send(Cmd::ClearResult);
            }
        });
        if !result_note.is_empty() && result_note != COPIED {
            ui.label(RichText::new(&result_note).small().color(WARN));
        }
        // Baris sendiri (wrap) agar tidak bertabrakan dengan judul/tombol saat kolom sempit.
        let status = match (result.is_empty(), result_note.as_str()) {
            (false, COPIED) => Some(COPIED),
            (false, "") => Some("Selesai"),
            _ => None,
        };
        if status.is_some() || timing.is_some() {
            ui.horizontal_wrapped(|ui| {
                if let Some(text) = status {
                    ui.label(RichText::new(text).small().color(SUCCESS));
                }
                if let Some((t, total)) = timing {
                    ui.label(
                        RichText::new(format!(
                            "{:.1} dtk audio · {:.2} dtk proses",
                            t.audio.as_secs_f32(),
                            total.as_secs_f32()
                        ))
                        .small()
                        .color(SECONDARY),
                    )
                    .on_hover_text(format!(
                        "VAD {:.2} dtk · ASR {:.2} dtk",
                        t.vad.as_secs_f32(),
                        t.asr.as_secs_f32()
                    ));
                }
            });
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
                let id = egui::Id::unique("shortcut-input");
                let focused = ui.memory(|m| m.has_focus(id));
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.shortcut_input)
                        .id(id)
                        .desired_width(ui.available_width() - 104.0)
                        .hint_text(RichText::new("Ctrl+Alt+Space").color(MUTED))
                        .frame(
                            egui::Frame::new()
                                .fill(INPUT)
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    if focused { ACCENT_BORDER } else { BORDER },
                                ))
                                .corner_radius(8)
                                .inner_margin(egui::Margin::symmetric(11, 8)),
                        ),
                );
                let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if secondary(ui, true, "Terapkan").clicked() || enter {
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
        // Folder Qwen yang belum dipilih dicari ulang agar model yang baru diunduh langsung terdeteksi.
        for (path, dir) in [
            (&mut self.qwen, config::QWEN_DIR),
            (&mut self.qwen17, config::QWEN17_DIR),
        ] {
            if path.is_none() {
                *path = config::default_model_dir(dir);
            }
        }
        ui.add_enabled_ui(!phase.busy(), |ui| {
            field_label(ui, "Mesin ASR");
            egui::ComboBox::from_id_salt("engine")
                .width(ui.available_width())
                .selected_text(self.engine.label())
                .show_ui(ui, |ui| {
                    for engine in Engine::ALL {
                        ui.selectable_value(&mut self.engine, engine, engine.label());
                    }
                });
            let engine = self.engine;
            let (label, pick) = if engine.is_qwen() {
                ("Folder model", Pick::Folder)
            } else {
                ("File model ASR", Pick::GgmlFile)
            };
            model_field(
                ui,
                label,
                pick,
                engine.default_name(),
                self.asr_path_mut(engine),
            );
            model_field(
                ui,
                "VAD (Silero)",
                Pick::GgmlFile,
                &format!(r"{}\{}", config::VAD_DIR, config::VAD_FILE),
                &mut self.vad,
            );
            let selected = self.selected_asr();
            let same = loaded.as_ref().is_some_and(|(e, a, v)| {
                *e == self.engine && Some(a) == selected.as_ref() && Some(v) == self.vad.as_ref()
            });
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let button = if same {
                    egui::Button::new("Muat ulang")
                } else {
                    // Pilihan berbeda dari model aktif: tonjolkan aksi yang perlu dilakukan.
                    egui::Button::new(RichText::new("Muat model").color(ACCENT_HOVER))
                        .fill(ACCENT_TINT)
                        .stroke(egui::Stroke::new(1.0, ACCENT_BORDER))
                };
                if ui
                    .add_enabled(selected.is_some() && self.vad.is_some(), button)
                    .clicked()
                    && let (Some(asr), Some(vad)) = (selected.clone(), self.vad.clone())
                {
                    self.config.engine = self.engine;
                    match self.engine {
                        Engine::Whisper => self.config.asr_model = Some(asr.clone()),
                        Engine::Qwen3Asr => self.config.qwen_model = Some(asr.clone()),
                        Engine::Qwen3Asr17 => self.config.qwen17_model = Some(asr.clone()),
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
                    ui.spinner();
                    ui.label(RichText::new("Memuat…").small().color(SECONDARY));
                } else if let (true, Some(t)) = (same, load_time) {
                    ui.label(
                        RichText::new(format!("Dimuat {:.1} dtk", t.as_secs_f32()))
                            .small()
                            .color(SECONDARY),
                    );
                }
            });
        });
        if !diagnostics.is_empty() {
            egui::CollapsingHeader::new(RichText::new("Diagnostik").small().color(SECONDARY))
                .id_salt("diagnostics")
                .show(ui, |ui| {
                    for line in &diagnostics {
                        ui.label(RichText::new(line).monospace().size(11.0).color(SECONDARY));
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

const COPIED: &str = "Disalin ke clipboard.";
const REPO_URL: &str = "https://github.com/fimln/fi-whisper";
const HOME_URL: &str = "https://alfi.ai.id";

fn phase_color(phase: Phase) -> egui::Color32 {
    match phase {
        Phase::Ready => SUCCESS,
        Phase::Recording | Phase::Error => DANGER,
        Phase::Loading | Phase::Transcribing => ACCENT,
        Phase::MissingModel => WARN,
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
        });
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

fn meter(ui: &mut Ui, active: bool, level: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), egui::Sense::hover());
    let count = ((rect.width() / 6.0) as usize).max(8);
    let step = rect.width() / count as f32;
    // Skala log sederhana: -60 dBFS..0 dBFS.
    let db = if level > 0.0 {
        20.0 * level.log10()
    } else {
        -60.0
    };
    let filled = (((db + 60.0) / 60.0).clamp(0.0, 1.0) * count as f32) as usize;
    let t = ui.input(|i| i.time) as f32;
    for i in 0..count {
        let on = active && i < filled;
        // Tinggi bar bervariasi agar terbaca sebagai gelombang suara, bukan progress bar.
        let wave = 0.55 + 0.45 * ((i as f32 * 0.9 + t * 7.0).sin() * 0.5 + 0.5);
        let height = if on { 8.0 + 24.0 * wave } else { 4.0 };
        let bar = egui::Rect::from_center_size(
            egui::pos2(rect.left() + (i as f32 + 0.5) * step, rect.center().y),
            vec2((step - 3.0).max(1.0), height),
        );
        let color = if !on {
            BORDER
        } else if i + 1 == filled {
            ACCENT_HOVER
        } else {
            ACCENT
        };
        ui.painter().rect_filled(bar, 2.0, color);
    }
}

#[derive(Clone, Copy)]
enum Pick {
    GgmlFile,
    Folder,
}

/// `default` = nama file/folder bawaan di `models\`, dipakai sebagai petunjuk bila belum ada.
fn model_field(ui: &mut Ui, label: &str, pick: Pick, default: &str, path: &mut Option<PathBuf>) {
    field_label(ui, label);
    let exists = path.as_deref().is_some_and(|p| match pick {
        Pick::GgmlFile => p.is_file(),
        Pick::Folder => p.join("config.json").is_file(),
    });
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
                            .map_or_else(|| "Belum dipilih".into(), file_name);
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
                response.on_hover_text(format!("Tidak ditemukan: {}", p.display()));
            }
            None => {}
        }
        if secondary(ui, true, "Pilih").clicked() {
            let dialog = rfd::FileDialog::new().set_title(format!("Pilih {label}"));
            let selected = match pick {
                Pick::GgmlFile => dialog.add_filter("Model GGML", &["bin"]).pick_file(),
                Pick::Folder => dialog.pick_folder(),
            };
            if let Some(selected) = selected {
                *path = Some(selected);
            }
        }
    });
    if !exists {
        ui.label(
            RichText::new(format!("Belum ada di models\\{default}"))
                .small()
                .color(WARN),
        );
    }
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
                .with_min_inner_size([560.0, 600.0])
                .with_icon(theme::icon()),
            renderer: eframe::Renderer::Glow,
            centered: true,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

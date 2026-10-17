#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod theme;

use eframe::egui::{self, Align, Layout, RichText, Ui, vec2};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use theme::*;

const LIMIT: Duration = Duration::from_secs(120);
const SAMPLE: &str = "Ini adalah contoh hasil dikte bahasa Indonesia. Hasil terakhir tetap tersedia di sini untuk dipilih, disalin, atau dihapus.";

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    MissingModel,
    Loading,
    Ready,
    Recording,
    Transcribing,
    Error,
}

impl State {
    fn label(self) -> &'static str {
        match self {
            Self::MissingModel => "Belum siap",
            Self::Loading => "Memuat demo",
            Self::Ready => "Siap · demo",
            Self::Recording => "Merekam · demo",
            Self::Transcribing => "Memproses · demo",
            Self::Error => "Periksa pengaturan",
        }
    }

    fn busy(self) -> bool {
        matches!(self, Self::Loading | Self::Recording | Self::Transcribing)
    }
}

struct App {
    settings: bool,
    demo: bool,
    state: State,
    since: Instant,
    captured: Duration,
    result: String,
    result_note: String,
    last_processing: Option<Duration>,
    language: String,
    shortcut: String,
    asr: Option<PathBuf>,
    vad: Option<PathBuf>,
    error: String,
}

impl Default for App {
    fn default() -> Self {
        Self {
            settings: false,
            demo: false,
            state: State::MissingModel,
            since: Instant::now(),
            captured: Duration::ZERO,
            result: String::new(),
            result_note: String::new(),
            last_processing: None,
            language: "Indonesia (id)".into(),
            shortcut: "Ctrl+Alt+Space".into(),
            asr: None,
            vad: None,
            error: String::new(),
        }
    }
}

impl App {
    fn transition(&mut self, state: State, now: Instant) {
        self.state = state;
        self.since = now;
    }

    fn set_demo(&mut self, enabled: bool, now: Instant) {
        self.demo = enabled;
        self.error.clear();
        self.captured = Duration::ZERO;
        self.transition(
            if enabled {
                State::Loading
            } else {
                State::MissingModel
            },
            now,
        );
    }

    fn toggle_recording(&mut self, now: Instant) {
        match self.state {
            State::Ready if self.demo => {
                self.captured = Duration::ZERO;
                self.transition(State::Recording, now);
            }
            State::Recording => {
                self.captured = now.saturating_duration_since(self.since).min(LIMIT);
                self.transition(State::Transcribing, now);
            }
            _ => {}
        }
    }

    fn cancel(&mut self, now: Instant) {
        if self.state.busy() {
            self.captured = Duration::ZERO;
            self.transition(
                if self.demo {
                    State::Ready
                } else {
                    State::MissingModel
                },
                now,
            );
        }
    }

    fn tick(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.since);
        match self.state {
            State::Loading if elapsed >= Duration::from_millis(650) => {
                self.transition(State::Ready, now);
            }
            State::Recording if elapsed >= LIMIT => self.toggle_recording(now),
            State::Transcribing if elapsed >= Duration::from_millis(1200) => {
                // shortcut: hasil demo memakai teks tetap; ganti dengan hasil worker saat backend dibuat.
                self.result = SAMPLE.into();
                self.result_note = "Contoh demo · tidak dikirim ke aplikasi lain".into();
                self.last_processing = Some(elapsed);
                self.transition(State::Ready, now);
            }
            _ => {}
        }
    }

    fn header(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new("fi-whisper").font(semibold(25.0)));
                ui.label(
                    RichText::new("Dikte lokal, dalam kendali Anda.")
                        .small()
                        .color(MUTED),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                badge(ui, self.state.label(), self.status_color());
            });
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.settings, false, "Dikte");
            ui.selectable_value(&mut self.settings, true, "Pengaturan");
        });
    }

    fn status_color(&self) -> egui::Color32 {
        match self.state {
            State::Error | State::Recording => DANGER,
            State::MissingModel => WARN,
            _ => ACCENT,
        }
    }

    fn demo_banner(&mut self, ui: &mut Ui) {
        egui::Frame::new()
            .fill(RAISED)
            .corner_radius(10)
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().interact_size.y = 26.0;
                ui.horizontal_wrapped(|ui| {
                    badge(ui, "PRATINJAU UI", ACCENT);
                    ui.label(
                        RichText::new("Backend belum terhubung. Mikrofon tidak diakses.")
                            .small()
                            .color(MUTED),
                    );
                });
                let mut enabled = self.demo;
                if ui
                    .checkbox(&mut enabled, "Aktifkan demo interaksi")
                    .changed()
                {
                    self.set_demo(enabled, Instant::now());
                }
            });
    }

    fn dictation(&mut self, ui: &mut Ui) {
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            title(ui, "Dikte");
            ui.label(
                RichText::new("Mulai bicara, lalu selesaikan untuk melihat hasil.").color(MUTED),
            );
            ui.add_space(4.0);
            let duration = if self.state == State::Recording {
                self.since.elapsed().min(LIMIT)
            } else {
                self.captured
            };
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{:02}:{:02}",
                        duration.as_secs() / 60,
                        duration.as_secs() % 60
                    ))
                    .monospace()
                    .size(36.0)
                    .color(if self.state == State::Recording {
                        ACCENT
                    } else {
                        TEXT
                    }),
                );
                ui.label(RichText::new("/ 02:00").monospace().color(MUTED));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(if self.state == State::Recording {
                            "Sinyal simulasi"
                        } else {
                            "Mikrofon tidak aktif"
                        })
                        .small()
                        .color(MUTED),
                    );
                });
            });
            self.meter(ui);
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                let (enabled, label) = match self.state {
                    State::Recording => (true, "Selesai · demo"),
                    State::Transcribing => (false, "Memproses…"),
                    State::Loading => (false, "Memuat…"),
                    _ => (self.demo && self.state == State::Ready, "Mulai · demo"),
                };
                if primary(ui, enabled, label).clicked() {
                    self.toggle_recording(Instant::now());
                }
                if self.state.busy() && ui.button("Batalkan").clicked() {
                    self.cancel(Instant::now());
                }
                if matches!(self.state, State::Loading | State::Transcribing) {
                    ui.spinner();
                }
                ui.label(
                    RichText::new(format!("{} · belum terdaftar", self.shortcut))
                        .small()
                        .color(MUTED),
                );
            });
            ui.add_space(4.0);
            let note = match self.state {
                State::MissingModel => {
                    "Pilih model di Pengaturan. Aktifkan demo untuk mencoba alur UI."
                }
                State::Loading => "Menyiapkan pratinjau, tanpa memuat model atau memakai GPU.",
                State::Ready => "Demo siap. Tombol GUI hanya menampilkan hasil di panel bawah.",
                State::Recording => "Simulasi rekaman. Berhenti otomatis pada 120 detik.",
                State::Transcribing => {
                    "Simulasi pemrosesan. Hasil contoh akan muncul setelah selesai."
                }
                State::Error => &self.error,
            };
            ui.label(
                RichText::new(note)
                    .small()
                    .color(if self.state == State::Error {
                        DANGER
                    } else {
                        MUTED
                    }),
            );
        });
        ui.add_space(8.0);
        self.result_ui(ui);
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            badge(ui, "Backend: belum diinisialisasi", MUTED);
            let timing = self
                .last_processing
                .map_or("Proses terakhir: —".into(), |d| {
                    format!("Proses demo: {:.2} dtk", d.as_secs_f32())
                });
            ui.label(RichText::new(timing).small().color(MUTED));
        });
    }

    fn meter(&self, ui: &mut Ui) {
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), 32.0), egui::Sense::hover());
        let count = 64;
        let step = rect.width() / count as f32;
        for i in 0..count {
            let level = if self.state == State::Recording {
                ((self.since.elapsed().as_secs_f32() * 4.0 + i as f32 * 0.7)
                    .sin()
                    .abs()
                    * 0.8
                    + 0.2)
                    * 32.0
            } else {
                4.0
            };
            let bar = egui::Rect::from_center_size(
                egui::pos2(rect.left() + (i as f32 + 0.5) * step, rect.center().y),
                vec2((step - 3.0).max(1.0), level),
            );
            ui.painter().rect_filled(
                bar,
                2.0,
                if self.state == State::Recording {
                    ACCENT
                } else {
                    BORDER
                },
            );
        }
    }

    fn result_ui(&mut self, ui: &mut Ui) {
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                title(ui, "Hasil terakhir");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add_enabled(!self.result.is_empty(), egui::Button::new("Hapus"))
                        .clicked()
                    {
                        self.result.clear();
                        self.result_note.clear();
                    }
                    if ui
                        .add_enabled(!self.result.is_empty(), egui::Button::new("Salin"))
                        .clicked()
                    {
                        ui.ctx().copy_text(self.result.clone());
                        self.result_note = "Contoh demo disalin ke clipboard".into();
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
                    if self.result.is_empty() {
                        ui.label(RichText::new("Hasil dikte akan muncul di sini.").color(MUTED));
                        ui.label(
                            RichText::new(
                                "Belum ada hasil. Coba alur demo untuk melihat pratinjau.",
                            )
                            .small()
                            .color(MUTED),
                        );
                    } else {
                        ui.add(egui::Label::new(&self.result).selectable(true).wrap());
                    }
                });
            if !self.result_note.is_empty() {
                ui.label(RichText::new(&self.result_note).small().color(ACCENT));
            }
            ui.label(
                RichText::new("Hasil hanya di RAM hingga diganti, dihapus, atau aplikasi ditutup.")
                    .small()
                    .color(MUTED),
            );
        });
    }

    fn settings_ui(&mut self, ui: &mut Ui) {
        let idle = !self.state.busy();
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            title(ui, "Input & bahasa");
            ui.label(
                RichText::new("Pilihan berlaku selama sesi pratinjau ini.")
                    .small()
                    .color(MUTED),
            );
            ui.add_space(8.0);
            field_label(ui, "Mikrofon");
            ui.add_enabled_ui(false, |ui| {
                egui::ComboBox::from_id_salt("microphone")
                    .width(ui.available_width() - 16.0)
                    .selected_text("Default sistem · perangkat belum diperiksa")
                    .show_ui(ui, |ui| {
                        ui.label("Menunggu backend audio");
                    });
            });
            ui.add_enabled_ui(idle, |ui| {
                field_label(ui, "Bahasa transkripsi");
                egui::ComboBox::from_id_salt("language")
                    .width(220.0)
                    .selected_text(&self.language)
                    .show_ui(ui, |ui| {
                        for language in ["Indonesia (id)", "Auto", "English (en)"] {
                            ui.selectable_value(&mut self.language, language.into(), language);
                        }
                    });
                field_label(ui, "Shortcut toggle");
                ui.add(
                    egui::TextEdit::singleline(&mut self.shortcut)
                        .desired_width(250.0)
                        .hint_text("Ctrl+Alt+Space"),
                );
                ui.label(
                    RichText::new("Belum didaftarkan secara global; konflik belum diperiksa.")
                        .small()
                        .color(MUTED),
                );
            });
        });
        ui.add_space(8.0);
        card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            title(ui, "Model lokal");
            ui.label(
                RichText::new("Pilih file yang sudah tersedia. Tidak ada unduhan otomatis.")
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
                    "Silero yang kompatibel dengan runtime whisper-rs",
                    &mut self.vad,
                );
                ui.add_space(4.0);
                if ui.button("Periksa pilihan").clicked() {
                    self.error = if self.asr.is_none() || self.vad.is_none() {
                        "Pilih file model ASR dan VAD terlebih dahulu.".into()
                    } else {
                        "File dipilih. Validasi format dan pemuatan model menunggu backend.".into()
                    };
                    if self.asr.is_none() || self.vad.is_none() {
                        self.transition(State::Error, Instant::now());
                    }
                }
            });
            if !self.error.is_empty() {
                ui.label(RichText::new(&self.error).color(WARN).small());
                if self.state == State::Error && ui.button("Kembali ke pratinjau").clicked() {
                    self.error.clear();
                    self.transition(
                        if self.demo {
                            State::Ready
                        } else {
                            State::MissingModel
                        },
                        Instant::now(),
                    );
                }
            }
            ui.label(
                RichText::new("Status model: belum dimuat · Backend aktual: belum diinisialisasi")
                    .small()
                    .color(MUTED),
            );
        });
        ui.add_space(8.0);
        ui.label(RichText::new("Konfigurasi belum disimpan ke disk. Tray dan shortcut global tersedia pada sesi backend; menutup jendela saat ini akan keluar.").small().color(MUTED));
    }
}

fn model_field(ui: &mut Ui, label: &str, description: &str, path: &mut Option<PathBuf>) {
    field_label(ui, label);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - 110.0).max(150.0));
            let name = path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned());
            ui.label(name.unwrap_or_else(|| "Belum dipilih".into()));
            if let Some(path) = path {
                ui.add(
                    egui::Label::new(
                        RichText::new(path.display().to_string())
                            .small()
                            .color(MUTED),
                    )
                    .wrap(),
                );
            } else {
                ui.label(RichText::new(description).small().color(MUTED));
            }
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
        self.tick(Instant::now());
        if self.state.busy() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    fn ui(&mut self, root: &mut Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("header")
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(24, 18)),
            )
            .show(root, |ui| self.header(ui));
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
                        self.demo_banner(ui);
                        ui.add_space(12.0);
                        if self.settings {
                            self.settings_ui(ui);
                        } else {
                            self.dictation(ui);
                        }
                    });
            });
    }
}

fn main() -> eframe::Result {
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
        Box::new(|cc| {
            theme::install(&cc.egui_ctx);
            Ok(Box::new(App::default()))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_flow_and_cancel_preserve_last_result() {
        let now = Instant::now();
        let mut app = App::default();
        app.toggle_recording(now);
        assert_eq!(app.state, State::MissingModel);
        app.set_demo(true, now);
        assert_eq!(app.state, State::Loading);
        app.tick(now + Duration::from_secs(1));
        assert_eq!(app.state, State::Ready);
        app.toggle_recording(now + Duration::from_secs(2));
        app.toggle_recording(now + Duration::from_secs(7));
        assert_eq!(app.state, State::Transcribing);
        assert_eq!(app.captured, Duration::from_secs(5));
        app.tick(now + Duration::from_secs(9));
        assert_eq!(app.result, SAMPLE);
        app.toggle_recording(now + Duration::from_secs(10));
        app.toggle_recording(now + Duration::from_secs(11));
        app.cancel(now + Duration::from_secs(12));
        app.tick(now + Duration::from_secs(20));
        assert_eq!(app.state, State::Ready);
        assert_eq!(app.result, SAMPLE);
        assert_eq!(app.last_processing, Some(Duration::from_secs(2)));
    }

    #[test]
    fn recording_limit_and_disabling_demo_cancel_pending_work() {
        let now = Instant::now();
        let mut app = App::default();
        app.set_demo(true, now);
        app.tick(now + Duration::from_secs(1));
        app.toggle_recording(now + Duration::from_secs(2));
        app.tick(now + Duration::from_secs(123));
        assert_eq!(app.state, State::Transcribing);
        assert_eq!(app.captured, LIMIT);
        app.set_demo(false, now + Duration::from_secs(124));
        app.tick(now + Duration::from_secs(130));
        assert_eq!(app.state, State::MissingModel);
        assert!(app.result.is_empty());
        assert!(app.last_processing.is_none());
    }
}

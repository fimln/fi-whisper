use crate::theme::*;
use crate::{
    APP_HANDLE, App, Cmd, Engine, Language, Phase, Source, config, message, phase_label, windows,
};
use rune::{Cx, El, TextEdit, TextStyle, col, el, row, text, text_input};
use rune_kit::{Select, SelectAppearance};
use std::sync::atomic::Ordering;

fn choose(
    s: &App,
    key: &str,
    current: Option<usize>,
    options: &[String],
    enabled: bool,
    layers: &mut Vec<El>,
    f: impl Fn(&mut App, usize) + Clone + 'static,
) -> El {
    let key = key.to_owned();
    let toggle_key = key.clone();
    let trigger = Select::new(&key, options)
        .current(current.and_then(|i| options.get(i)).map(String::as_str))
        .label(&key)
        .width(280.0)
        .appearance(SelectAppearance::Field)
        .open(s.menu.as_deref() == Some(key.as_str()))
        .disabled(!enabled)
        .on_toggle(move |s: &mut App| {
            if toggle_key == "microphone" && !s.preview {
                s.refresh_mics();
            }
            s.menu = if s.menu.as_ref() == Some(&toggle_key) {
                None
            } else {
                Some(toggle_key.clone())
            };
        })
        .trigger(&kit())
        .w_full()
        .h(34.0);
    if enabled && s.menu.as_deref() == Some(key.as_str()) {
        let k = kit();
        let anchor = format!("{key}-trigger");
        let mut rows = col().key("rows").gap(2.0).padding(4.0).w_full();
        for (i, option) in options.iter().enumerate() {
            let choose = f.clone();
            rows = rows.child(
                row()
                    .key(format!("option{i}"))
                    .role("option")
                    .label(option.clone())
                    .focusable(true)
                    .focus_selected(current == Some(i))
                    .position_in_set(i + 1, options.len())
                    .w_full()
                    .min_h(32.0)
                    .px(12.0)
                    .py(7.0)
                    .gap(8.0)
                    .center_y()
                    .radius(4.0)
                    .hover(k.c.hover)
                    .focus_highlight(k.c.hover)
                    .child(
                        text(option.clone())
                            .key("label")
                            .font(k.ty.ui_compact)
                            .color(k.c.text_base)
                            .wrap_anywhere()
                            .grow(1.0)
                            .shrink()
                            .min_w(0.0),
                    )
                    .child(
                        rune::icon(k.glyph.check_small, 14.0)
                            .color(k.c.icon_base)
                            .opacity(if current == Some(i) { 1.0 } else { 0.0 }),
                    )
                    .on_press(move |s: &mut App| {
                        choose(s, i);
                        s.menu = None;
                    }),
            );
        }
        layers.push(
            col()
                .key(format!("{key}-content"))
                .label(key.clone())
                .w(420.0)
                .max_h(260.0)
                .radius(6.0)
                .fill(surface())
                .border(1.0, border())
                .shadow(6.0, 20.0, c(0x000000).alpha(0.4))
                .modal(true, |s: &mut App| s.menu = None)
                .focus_return(&anchor)
                .anchored_fit(anchor, rune::Side::Bottom, rune::Align::Start, 2.0)
                .instant_changes()
                .child(
                    col()
                        .key("listbox")
                        .role("listbox")
                        .label(key)
                        .focus_group()
                        .w_full()
                        .shrink()
                        .min_h(0.0)
                        .scroll(rune::ScrollSpec {
                            stick_bottom: false,
                        })
                        .child(rows),
                ),
        );
    }
    trigger
}
fn field(key: &str, edit: &TextEdit, placeholder: &str) -> El {
    text_input(edit, TextStyle::new(14.0, 400.0))
        .single_line()
        .key(key)
        .label(key)
        .placeholder(placeholder, muted())
        .h(34.0)
        .w_full()
        .px(10.0)
        .py(8.0)
        .radius(6.0)
        .fill(bg())
        .border(1.0, border())
        .color(fg())
        .selection_color(accent().alpha(0.25))
}
fn note(s: &App, v: &str, error: bool) -> El {
    text(message(v, s.config.language == Language::English))
        .wrap()
        .font(TextStyle::new(12.0, 400.0))
        .color(if error { c(0xef4444) } else { secondary() })
}
fn header(s: &App, width: f32) -> El {
    let st = s.core.status();
    let color = match st.phase {
        Phase::Ready | Phase::CloudIdle | Phase::Recording => c(0x10b981),
        Phase::MissingModel | Phase::Error => c(0xef4444),
        _ => accent(),
    };
    let engine = st.loaded.as_ref().map(|x| x.0);
    let badge = engine.map(|e| {
        if e == Engine::GoogleChirp3 {
            e.short().to_owned()
        } else {
            format!("{}, {}", e.short(), st.backend)
        }
    });
    let mut h = row()
        .key("header")
        .h(46.0)
        .center_y()
        .gap(10.0)
        .window_drag()
        .child(el().size(30.0, 30.0).pixels(&s.logo))
        .child(
            text("fi-whisper")
                .font(TextStyle::new(20.0, 600.0))
                .color(fg()),
        )
        .child(el().grow(1.0));
    if let Some(b) = badge.filter(|_| width >= 700.0) {
        h = h.child(
            el().px(9.0)
                .key("model-badge")
                .py(4.0)
                .radius(12.0)
                .fill(secondary().alpha(0.08))
                .child(label(b).color(if st.backend.starts_with("CPU") {
                    accent()
                } else {
                    secondary()
                })),
        );
    }
    h.child(
        text(phase_label(
            st.phase,
            s.config.language == Language::English,
        ))
        .key("status")
        .font(TextStyle::new(12.0, 500.0))
        .color(color),
    )
    .child(label(concat!("v", env!("CARGO_PKG_VERSION"))).key("version"))
    .child(
        icon_button("github", "GitHub", &s.icons.github)
            .href("https://github.com/fimln/fi-whisper"),
    )
    .child(icon_button("home", "alfi.ai.id", &s.icons.home).href("https://alfi.ai.id"))
    .child(
        icon_button("minimize", s.t("Perkecil", "Minimize"), &s.icons.minimize).on_press(
            |_: &mut App| {
                if let Some(h) = APP_HANDLE.get() {
                    h.minimize();
                }
            },
        ),
    )
    .child(
        icon_button("close", s.t("Tutup", "Close"), &s.icons.close).on_press(|_: &mut App| {
            if let Some(h) = APP_HANDLE.get() {
                h.close();
            }
        }),
    )
}
fn dictation(s: &App, reduced: bool) -> El {
    let st = s.core.status();
    let phase = st.phase;
    let recording = phase == Phase::Recording;
    let duration = st
        .recording_since
        .map_or(st.captured, |x| x.elapsed().min(crate::audio::LIMIT));
    let (enabled, caption) = match phase {
        Phase::Recording => (true, s.t("Selesai", "Done")),
        Phase::Transcribing => (false, "Memproses…"),
        Phase::Loading => (false, "Memuat…"),
        _ => (st.loaded.is_some(), s.t("Mulai dikte", "Start dictation")),
    };
    let fraction = meter_fraction(s.level);
    let busy = matches!(phase, Phase::Loading | Phase::Transcribing);
    let mut bars = row()
        .key("meter")
        .role("meter")
        .label("Level audio")
        .h(36.0)
        .center_y()
        .gap(3.0);
    for i in 0..32 {
        let exact = fraction * 32.0;
        let on = recording && (i as f32) < exact;
        let edge = (exact - i as f32).clamp(0.0, 1.0);
        let now = if reduced { 0.0 } else { s.now as f32 };
        let processing = busy && ((i as f32 - now * 12.0 % 40.0 + 8.0).abs() < 3.0);
        let wave = 0.55 + 0.45 * ((i as f32 * 0.9 + now * 7.0).sin() * 0.5 + 0.5);
        let height = if on {
            6.0 + 26.0 * wave * (0.35 + 0.65 * fraction) * edge
        } else if processing {
            10.0
        } else {
            4.0
        };
        bars = bars.child(
            el().key(format!("bar-{i}"))
                .grow(1.0)
                .h(height)
                .radius(2.0)
                .fill(if on || processing { accent() } else { border() })
                .motion(rune::tok::motion::SNAPPY),
        );
    }
    let mut buttons =
        row().gap(8.0).child(
            button("dictate", caption, true, enabled).on_press(|s: &mut App| {
                s.core.send(Cmd::Toggle(Source::Gui));
            }),
        );
    if matches!(phase, Phase::Recording | Phase::Transcribing) {
        buttons = buttons.child(
            button("cancel", s.t("Batalkan", "Cancel"), false, true)
                .on_press(|s: &mut App| s.core.send(Cmd::Cancel)),
        );
    }
    let mut card = card("dictation")
        .child(
            row()
                .justify_between()
                .center_y()
                .child(title(s.t("Dikte", "Dictation")))
                .child(label(
                    s.shortcut()
                        .unwrap_or(s.t("Shortcut tidak aktif", "Shortcut inactive")),
                )),
        )
        .child(
            row()
                .center_y()
                .gap(6.0)
                .child(
                    text(format!(
                        "{:02}:{:02}",
                        duration.as_secs() / 60,
                        duration.as_secs() % 60
                    ))
                    .font(TextStyle::new(36.0, 500.0).mono())
                    .color(if recording { fg() } else { secondary() }),
                )
                .child(label("/ 02:00")),
        )
        .child(bars)
        .child(buttons);
    if !st.error.is_empty() {
        card = card.child(note(s, &st.error, true));
    }
    if !st.note.is_empty() {
        card = card.child(note(s, &st.note, false));
    }
    if st.loaded.is_none() && matches!(phase, Phase::MissingModel | Phase::Error) {
        card = card.child(
            label(s.t(
                "Pilih model ASR dan VAD, lalu tekan Muat model.",
                "Select ASR and VAD models, then press Load model.",
            ))
            .key("missing-model-note"),
        );
    }
    card
}
fn result(s: &App) -> El {
    let st = s.core.status();
    let empty = st.result.is_empty();
    let body = if empty {
        text(s.t(
            "Hasil dikte akan muncul di sini.",
            "Your dictation will appear here.",
        ))
        .color(muted())
    } else {
        text(&st.result)
            .color(fg())
            .selectable(true)
            .selection_color(accent().alpha(0.25))
    };
    let mut out = card("result")
        .child(title(s.t("Hasil terakhir", "Last result")))
        .child(
            col()
                .key("result-text")
                .h(130.0)
                .grow(1.0)
                .w_full()
                .padding(10.0)
                .radius(6.0)
                .fill(bg())
                .border(1.0, border())
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                })
                .child(
                    body.font(TextStyle::new(15.0, 400.0))
                        .wrap()
                        .w_full()
                        .key("result-body"),
                ),
        )
        .child(
            row()
                .gap(8.0)
                .child(
                    button("copy", s.t("Salin", "Copy"), false, !empty).on_press(|s: &mut App| {
                        let result = s.core.status().result.clone();
                        match windows::copy_text(s.window.hwnd.load(Ordering::SeqCst), &result) {
                            Ok(()) => {
                                s.copy_flash = s.now;
                                s.core.status().result_note = "Disalin ke clipboard.".into();
                            }
                            Err(e) => s.tray_error = e,
                        }
                    }),
                )
                .child(
                    button("clear", s.t("Hapus", "Clear"), false, !empty)
                        .on_press(|s: &mut App| s.core.send(Cmd::ClearResult)),
                ),
        );
    if !st.result_note.is_empty() {
        out = out.child(note(
            s,
            if st.result_note == "Disalin ke clipboard." {
                s.t("Disalin ke clipboard.", "Copied to clipboard.")
            } else {
                &st.result_note
            },
            false,
        ));
    }
    if let Some((timing, total)) = st.timing {
        out = out.child(label(format!(
            "{:.1} s · {:.1} s",
            timing.audio.as_secs_f32(),
            total.as_secs_f32()
        )));
    }
    out
}
fn settings(s: &App, layers: &mut Vec<El>) -> El {
    let idle = !s.core.status().phase.busy();
    let mut microphones = vec![s.t("Default sistem", "System default").to_owned()];
    microphones.extend(s.mics.iter().map(|m| m.name.clone()));
    let selected = s
        .config
        .microphone
        .as_ref()
        .and_then(|id| s.mics.iter().position(|m| &m.id == id))
        .map_or(0, |i| i + 1);
    let mic = choose(
        s,
        "microphone",
        Some(selected),
        &microphones,
        idle,
        layers,
        |s, i| {
            if s.core.status().phase.busy() {
                return;
            }
            s.config.microphone = i
                .checked_sub(1)
                .and_then(|i| s.mics.get(i))
                .map(|m| m.id.clone());
            s.save_config();
            s.push_settings();
        },
    );
    let languages = Language::ALL.map(|x| x.label().to_owned());
    let language = choose(
        s,
        "language",
        Language::ALL.iter().position(|x| *x == s.config.language),
        &languages,
        idle,
        layers,
        |s, i| {
            if s.core.status().phase.busy() {
                return;
            }
            if let Some(&x) = Language::ALL.get(i) {
                s.config.language = x;
                s.save_config();
                s.push_settings();
            }
        },
    );
    let close_options = [
        s.t("Keluar dari aplikasi", "Quit application"),
        s.t("Sembunyikan ke tray", "Hide to tray"),
        s.t(
            "Sembunyikan ke tray, auto-startup",
            "Hide to tray, start with Windows",
        ),
    ]
    .map(str::to_owned);
    let selected = if s.config.auto_startup {
        2
    } else {
        usize::from(s.config.close_to_tray)
    };
    let close = choose(
        s,
        "close-action",
        Some(selected),
        &close_options,
        true,
        layers,
        move |s, index| {
            if index > 0 && s.tray.is_none() {
                return;
            }
            match windows::set_auto_startup(index == 2) {
                Ok(()) => {
                    s.config.close_to_tray = index != 0;
                    s.config.auto_startup = index == 2;
                    s.save_config();
                }
                Err(e) => s.tray_error = e,
            }
        },
    );
    let mut out = card("settings")
        .child(title(s.t("Pengaturan", "Settings")))
        .child(label(s.t("Mikrofon", "Microphone")))
        .child(mic)
        .child(label(s.t("Bahasa", "Language")))
        .child(language)
        .child(label("Shortcut"))
        .child(
            row()
                .gap(8.0)
                .child(
                    field("shortcut-input", &s.shortcut_edit, "Ctrl+Alt+Space")
                        .grow(1.0)
                        .disabled(!idle)
                        .on_edit(|s: &mut App| &mut s.shortcut_edit)
                        .on_submit(|s: &mut App, _| {
                            s.shortcut_input = s.shortcut_edit.text.clone();
                            s.apply_shortcut();
                        }),
                )
                .child(
                    button("apply", s.t("Terapkan", "Apply"), false, idle).on_press(
                        |s: &mut App| {
                            s.shortcut_input = s.shortcut_edit.text.clone();
                            s.apply_shortcut();
                        },
                    ),
                ),
        )
        .child(label(s.t("Tombol close", "Close button")))
        .child(close);
    for error in [&s.mic_error, &s.shortcut_error, &s.tray_error] {
        if !error.is_empty() {
            out = out.child(note(s, error, true));
        }
    }
    out
}
fn pick_model(s: &mut App, vad: bool) {
    let folder = !vad && s.engine.is_qwen();
    let caption = if vad {
        "VAD (Silero)"
    } else if folder {
        s.t("Folder model", "Model folder")
    } else {
        s.t("File model ASR", "ASR model file")
    };
    let dialog = rfd::FileDialog::new().set_title(format!("{} {caption}", s.t("Pilih", "Select")));
    let path = if folder {
        dialog.pick_folder()
    } else {
        dialog.add_filter("Model GGML", &["bin"]).pick_file()
    };
    if let Some(path) = path {
        if vad {
            s.vad = Some(path);
        } else {
            let engine = s.engine;
            *s.asr_path_mut(engine).0 = Some(path);
        }
        s.refresh_models();
    }
}
fn cloud(s: &App, layers: &mut Vec<El>, idle: bool) -> El {
    let regions = ["us".to_owned(), "eu".to_owned()];
    let region = choose(
        s,
        "cloud-region",
        regions.iter().position(|r| *r == s.config.google.region),
        &regions,
        idle,
        layers,
        |s, i| {
            if s.core.status().phase.busy() {
                return;
            }
            s.config.google.region = ["us", "eu"][i].into();
            s.save_config();
            s.core.send(Cmd::Google(s.config.google.clone()));
        },
    );
    let mut out = col()
        .gap(8.0)
        .child(label("Project ID"))
        .child(
            field("cloud-project", &s.project_edit, "my-gcp-project-id")
                .on_edit(|s: &mut App| &mut s.project_edit)
                .on_input_change(|s: &mut App, _| {
                    s.config.google.project = s.project_edit.text.clone();
                    s.save_config();
                    s.core.send(Cmd::Google(s.config.google.clone()));
                }),
        )
        .child(
            row()
                .gap(8.0)
                .child(
                    col()
                        .grow(1.0)
                        .gap(6.0)
                        .child(label("Region"))
                        .child(region),
                )
                .child(
                    col()
                        .grow(1.0)
                        .gap(6.0)
                        .child(label(s.t("Batas / bulan", "Monthly limit")))
                        .child(
                            row()
                                .center_y()
                                .gap(6.0)
                                .child(
                                    field("cloud-limit", &s.limit_edit, "600")
                                        .grow(1.0)
                                        .shrink()
                                        .min_w(0.0)
                                        .on_edit(|s: &mut App| &mut s.limit_edit)
                                        .on_input_change(|s: &mut App, _| {
                                            if let Ok(v) = s.limit_edit.text.parse::<u32>() {
                                                s.config.google.monthly_limit_minutes =
                                                    v.min(60000);
                                                s.save_config();
                                                s.core.send(Cmd::Google(s.config.google.clone()));
                                            }
                                        }),
                                )
                                .child(label(s.t("menit", "min"))),
                        ),
                ),
        );
    if let Some((_, _, usage)) = &s.cloud_usage {
        out = out.child(match usage {
            Ok(seconds) => {
                let minutes = *seconds as f64 / 60.0;
                label(if s.config.language == Language::English {
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
                .key("cloud-usage")
                .color(
                    if minutes >= s.config.google.monthly_limit_minutes as f64 * 0.8 {
                        accent()
                    } else {
                        secondary()
                    },
                )
            }
            Err(e) => note(s, e, true),
        });
    }
    out = out.child(
        button("setup-adc", "Setup ADC", false, true)
            .on_press(|s: &mut App| s.adc_open = !s.adc_open),
    );
    if s.adc_open {
        let project = if s.config.google.project.trim().is_empty() {
            "PROJECT_ID"
        } else {
            s.config.google.project.trim()
        };
        let commands = format!(
            "gcloud auth application-default login\ngcloud auth application-default set-quota-project {project}"
        );
        let help = s.t(
            "Penghitung lokal bukan tagihan Google. Bulan mengikuti UTC. Sisa minimal 2 menit untuk memulai sesi.",
            "Local counter, not your Google bill. Months use UTC. At least 2 minutes must remain to start a session.",
        );
        out = out
            .child(label(s.t(
                "Jalankan di terminal, lalu mulai dikte:",
                "Run in a terminal, then start dictation:",
            )))
            .child(
                text(commands)
                    .key("adc-command")
                    .font(TextStyle::new(11.5, 400.0).mono())
                    .color(fg())
                    .selectable(true)
                    .wrap_anywhere()
                    .w_full(),
            )
            .child(label(help));
    }
    out
}
fn model_field(
    s: &App,
    caption: &str,
    path: Option<&std::path::Path>,
    valid: bool,
    default: &str,
    vad: bool,
    enabled: bool,
) -> El {
    let key = if vad { "vad-file" } else { "asr-file" };
    let name = path
        .and_then(|p| p.file_name())
        .and_then(|p| p.to_str())
        .unwrap_or(s.t("Belum dipilih", "Not selected"));
    let color = if path.is_none() {
        muted()
    } else if valid {
        fg()
    } else {
        c(0xef4444)
    };
    let mut field = col().gap(6.0).child(label(caption)).child(
        row()
            .gap(8.0)
            .child(
                el().key(key)
                    .grow(1.0)
                    .min_w(0.0)
                    .h(34.0)
                    .px(10.0)
                    .center_y()
                    .radius(8.0)
                    .fill(bg())
                    .border(1.0, border())
                    .child(
                        text(name)
                            .font(TextStyle::new(14.0, 400.0))
                            .color(color)
                            .truncate(),
                    ),
            )
            .child(
                button(
                    &format!("{key}-browse"),
                    s.t("Pilih", "Browse"),
                    false,
                    enabled,
                )
                .label(format!("{} {caption}", s.t("Pilih", "Browse")))
                .on_press(move |s: &mut App| pick_model(s, vad)),
            ),
    );
    if !valid {
        field = field.child(
            label(if path.is_some() {
                s.t(
                    "Model tidak lengkap atau tidak ditemukan.",
                    "Model incomplete or not found.",
                )
                .to_owned()
            } else {
                format!("{} models\\{default}", s.t("Belum ada di", "Missing from"))
            })
            .color(accent()),
        );
    }
    field
}
fn model(s: &App, layers: &mut Vec<El>) -> El {
    let st = s.core.status();
    let idle = !st.phase.busy();
    let options = Engine::ALL.map(|e| e.label().to_owned());
    let engine = choose(
        s,
        "engine",
        Engine::ALL.iter().position(|e| *e == s.engine),
        &options,
        idle,
        layers,
        |s, i| {
            if let Some(&e) = Engine::ALL.get(i) {
                s.select_engine(e);
            }
        },
    );
    let mut out = card("model")
        .child(title("Model"))
        .child(label(s.t("Mesin ASR", "ASR engine")))
        .child(engine);
    if s.engine == Engine::GoogleChirp3 {
        return out.child(cloud(s, layers, idle).disabled(!idle));
    }
    let selected = s.selected_asr();
    let asr_label = if s.engine.is_qwen() {
        s.t("Folder model", "Model folder")
    } else {
        s.t("File model ASR", "ASR model file")
    };
    out = out
        .child(model_field(
            s,
            asr_label,
            selected.as_deref(),
            s.model_valid[s.engine as usize],
            s.engine.default_name(),
            false,
            idle,
        ))
        .child(model_field(
            s,
            "VAD (Silero)",
            s.vad.as_deref(),
            s.model_valid[3],
            &format!(r"{}\{}", config::VAD_DIR, config::VAD_FILE),
            true,
            idle,
        ));
    let same = st.loaded.as_ref().is_some_and(|(e, a, v)| {
        *e == s.engine && Some(a) == selected.as_ref() && Some(v) == s.vad.as_ref()
    });
    out = out.child(
        button(
            "load",
            if same {
                s.t("Muat ulang", "Reload")
            } else {
                s.t("Muat model", "Load model")
            },
            !same,
            idle && selected.is_some() && s.vad.is_some(),
        )
        .on_press(|s: &mut App| s.load_model()),
    );
    if let (true, Some(t)) = (same, st.load_time) {
        out = out.child(label(format!(
            "{} {:.1} s",
            s.t("Dimuat", "Loaded in"),
            t.as_secs_f32()
        )));
    }
    if !st.diagnostics.is_empty() {
        out = out.child(
            button("diagnostics", s.t("Diagnostik", "Diagnostics"), false, true)
                .on_press(|s: &mut App| s.diagnostics_open = !s.diagnostics_open),
        );
        if s.diagnostics_open {
            for line in &st.diagnostics {
                out = out.child(note(s, line, false));
            }
        }
    }
    out
}
pub fn view(s: &App, cx: &mut Cx<App>) -> El {
    let reduced = cx.is_reduced_motion();
    s.reduced_motion.set(reduced);
    cx.focus_ring(1.0, 0.0, accent());
    let mut layers = Vec::new();
    let settings = settings(s, &mut layers);
    let model = model(s, &mut layers);
    let mut body = col()
        .w_full()
        .min_h(cx.layout_size("root").map_or(680.0, |size| size[1]) - 94.0)
        .pb(16.0)
        .gap(12.0)
        .child(
            row()
                .grow(1.0)
                .gap(12.0)
                .child(dictation(s, reduced))
                .child(result(s)),
        )
        .child(row().gap(12.0).child(settings).child(model));
    if !s.config_error.is_empty() {
        body = col()
            .gap(12.0)
            .child(
                card("config-error")
                    .child(note(s, &s.config_error, true))
                    .child(
                        button(
                            "reset-config",
                            s.t("Reset konfigurasi ke default", "Reset configuration"),
                            false,
                            true,
                        )
                        .on_press(|s: &mut App| {
                            s.config = crate::Config::default();
                            s.config_error.clear();
                            s.engine = s.config.engine;
                            s.shortcut_edit = TextEdit::new(&s.config.shortcut);
                            s.project_edit = TextEdit::new(&s.config.google.project);
                            s.limit_edit =
                                TextEdit::new(s.config.google.monthly_limit_minutes.to_string());
                            s.shortcut_input = s.config.shortcut.clone();
                            s.apply_shortcut();
                            s.refresh_mics();
                            s.refresh_models();
                            s.push_settings();
                            s.save_config();
                        }),
                    ),
            )
            .child(body);
    }
    let mut root = col()
        .key("root")
        .w_full()
        .h_full()
        .padding(16.0)
        .pb(20.0)
        .gap(12.0)
        .radius(12.0)
        .border(1.0, accent().alpha(0.12))
        .fill(bg())
        .radial_gradient(
            [0.2, 0.0],
            550.0,
            [(0.0, accent().alpha(0.055)), (1.0, accent().alpha(0.0))],
        )
        .child(header(
            s,
            cx.layout_size("root").map_or(560.0, |size| size[0]),
        ))
        .child(
            col()
                .key("content")
                .grow(1.0)
                .min_h(0.0)
                .w_full()
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                })
                .child(body),
        );
    for (key, left, top, right, bottom, sides, corners) in [
        (
            "corner-tl",
            Some(1.0),
            Some(1.0),
            None,
            None,
            [1.0, 0.0, 0.0, 1.0],
            [10.0, 0.0, 0.0, 0.0],
        ),
        (
            "corner-tr",
            None,
            Some(1.0),
            Some(1.0),
            None,
            [1.0, 1.0, 0.0, 0.0],
            [0.0, 10.0, 0.0, 0.0],
        ),
        (
            "corner-bl",
            Some(1.0),
            None,
            None,
            Some(1.0),
            [0.0, 0.0, 1.0, 1.0],
            [0.0, 0.0, 0.0, 10.0],
        ),
        (
            "corner-br",
            None,
            None,
            Some(1.0),
            Some(1.0),
            [0.0, 1.0, 1.0, 0.0],
            [0.0, 0.0, 10.0, 0.0],
        ),
    ] {
        root = root.child(
            el().key(key)
                .pin(left, top, right, bottom)
                .size(24.0, 24.0)
                .corners(corners)
                .border_sides(sides, accent().alpha(0.3))
                .a11y_hidden(),
        );
    }
    for layer in layers {
        root = root.child(layer);
    }
    root
}

pub fn meter_fraction(level: f32) -> f32 {
    let db = if level > 0.0 {
        20.0 * level.log10()
    } else {
        -60.0
    };
    ((db + 60.0) / 36.0).clamp(0.0, 1.0)
}

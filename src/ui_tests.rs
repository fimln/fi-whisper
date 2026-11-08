use crate::{App, Engine, Language, ui};
use rune::{Env, Runtime};
fn runtime(size: (f32, f32)) -> Runtime<App> {
    let state = App::create(true);
    let mut rt = Runtime::new(
        state,
        ui::view,
        size,
        Env {
            system_dark: true,
            scale: 1.0,
        },
    );
    rt.frame(0.0);
    rt
}
#[test]
fn four_cards_keep_two_columns_at_supported_sizes() {
    for size in [(700.0, 700.0), (560.0, 600.0), (1000.0, 800.0)] {
        let rt = runtime(size);
        let rect = |key| rt.presented(key).unwrap().0;
        let d = rect("dictation");
        let r = rect("result");
        let s = rect("settings");
        let m = rect("model");
        assert!(d[2] > 150.0 && r[2] > 150.0, "cards must have usable width");
        assert!(d[0] + d[2] <= r[0] + 1.0, "top cards overlap");
        assert!(s[0] + s[2] <= m[0] + 1.0, "bottom cards overlap");
        assert!(d[1] + d[3] <= s[1] + 1.0, "rows overlap");
        assert!(r[0] + r[2] <= size.0, "cards exceed window");
        let n = rect("missing-model-note");
        assert!(n[0] + n[2] <= d[0] + d[2], "help text exceeds its card");
        let b = rect("result-body");
        assert!(b[0] + b[2] <= r[0] + r[2], "result text exceeds its card");
    }
}
#[test]
fn dropdown_opens_and_cloud_form_has_no_local_model_fields() {
    let mut rt = runtime((700.0, 700.0));
    let (x, y) = rt.find("engine-trigger").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state.menu.as_deref(), Some("engine"));
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let (x, y) = rt.find("option3").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(rt.state.engine, Engine::GoogleChirp3);
    assert!(rt.find("cloud-project").is_some());
    assert!(rt.find("asr-file").is_none());
    assert!(rt.find("vad-file").is_none());
    let (x, y) = rt.find("setup-adc").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert!(rt.state.adc_open);
    let (command, _) = rt.presented("adc-command").unwrap();
    let (model, _) = rt.presented("model").unwrap();
    assert!(command[0] + command[2] <= model[0] + model[2]);
}
#[test]
fn preview_does_not_register_native_resources_or_write_config() {
    let mut s = App::create(true);
    assert!(s.tray.is_none());
    assert!(s.hotkeys.is_none());
    assert!(s._activation.is_none());
    s.config_path = Some(std::path::PathBuf::from(
        "missing-parent/preview-config.json",
    ));
    s.save_config();
    assert!(s.config_error.is_empty());
    s.config.language = Language::English;
    assert_eq!(s.t("Dikte", "Dictation"), "Dictation");
}

#[test]
fn speech_meter_range() {
    let level = |db: f32| 10.0_f32.powf(db / 20.0);
    assert_eq!(ui::meter_fraction(0.0), 0.0);
    assert_eq!(ui::meter_fraction(level(-66.0)), 0.0);
    assert_eq!(ui::meter_fraction(level(-60.0)), 0.0);
    assert!((ui::meter_fraction(level(-42.0)) - 0.5).abs() < 1e-6);
    assert!((ui::meter_fraction(level(-33.0)) - 0.75).abs() < 1e-6);
    assert_eq!(ui::meter_fraction(level(-24.0)), 1.0);
    assert_eq!(ui::meter_fraction(level(-12.0)), 1.0);
    assert_eq!(ui::meter_fraction(1.0), 1.0);
}

#[test]
fn engine_switch_invalidates_cloud_before_loading_local_and_rejects_busy_switches() {
    let mut s = App::create(true);
    s.engine = Engine::GoogleChirp3;
    {
        let mut st = s.core.status();
        st.phase = crate::Phase::CloudIdle;
        st.loaded = Some((Engine::GoogleChirp3, "".into(), "".into()));
    }
    s.select_engine(Engine::Whisper);
    assert_eq!(s.engine, Engine::Whisper);
    assert_eq!(s.config.engine, Engine::Whisper);
    assert!(s.core.status().loaded.is_none());
    s.core.status().phase = crate::Phase::Recording;
    s.select_engine(Engine::GoogleChirp3);
    assert_eq!(s.engine, Engine::Whisper);
}

#[test]
fn microphones_with_identical_names_remain_individually_selectable() {
    let mut rt = runtime((700.0, 680.0));
    rt.update(|s, _| {
        s.mics = vec![
            crate::audio::Mic {
                id: "first".into(),
                name: "Microphone".into(),
            },
            crate::audio::Mic {
                id: "second".into(),
                name: "Microphone".into(),
            },
        ];
    });
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let (x, y) = rt.find("microphone-trigger").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let (x, y) = rt.find("option2").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(rt.state.config.microphone.as_deref(), Some("second"));
}

#[test]
fn already_open_region_popup_cannot_change_settings_after_recording_starts() {
    let mut rt = runtime((700.0, 680.0));
    rt.update(|s, _| {
        s.engine = Engine::GoogleChirp3;
    });
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let (x, y) = rt.find("cloud-region-trigger").unwrap();
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let (x, y) = rt.find("option1").unwrap();
    // Change the controller state without rebuilding the mounted popup first.
    rt.state.core.status().phase = crate::Phase::Recording;
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    assert_eq!(rt.state.config.google.region, "us");
    assert!(rt.find("cloud-region-content").is_none());
}

#[test]
fn reduced_motion_keeps_meter_still_and_polls_busy_state_less_often() {
    let mut rt = runtime((700.0, 680.0));
    rt.set_system_reduced_motion(true);
    rt.update(|s, _| {
        s.core.status().phase = crate::Phase::Recording;
        s.level = 0.5;
        s.now = 1.0;
    });
    rt.frame(0.0);
    let bars = |rt: &Runtime<App>| {
        (0..32)
            .map(|i| rt.presented(&format!("bar-{i}")).unwrap().0)
            .collect::<Vec<_>>()
    };
    let before = bars(&rt);
    rt.update(|s, _| s.now = 2.0);
    rt.frame(0.0);
    assert_eq!(before, bars(&rt));
    rt.update(|s, tx| {
        crate::poll(s, 10.0, tx);
    });
    rt.frame(0.0);
    assert!((rt.wake_deadline().unwrap() - rt.t - 0.25).abs() < 0.001);
}

#[test]
fn cloud_cards_fill_window_height_after_resize() {
    for size in [(700.0, 680.0), (1000.0, 900.0), (560.0, 600.0)] {
        let mut rt = runtime(size);
        rt.update(|s, _| {
            s.engine = Engine::GoogleChirp3;
            let mut st = s.core.status();
            st.phase = crate::Phase::CloudIdle;
            st.loaded = Some((Engine::GoogleChirp3, "".into(), "".into()));
        });
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        rt.update(|_, tx| tx.scroll_to_bottom_instant("content"));
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        let (settings, _) = rt.presented("settings").unwrap();
        assert!(
            (size.1 - settings[1] - settings[3] - 36.0).abs() < 2.0,
            "excess bottom space at {size:?}: {settings:?}"
        );
        assert!(rt.presented("corner-br").is_some());
        let (header, _) = rt.presented("header").unwrap();
        assert_eq!(header[3], 46.0);
    }
}

#[test]
fn compact_header_keeps_close_button_inside_window_with_backend_badge() {
    let mut rt = runtime((560.0, 600.0));
    rt.update(|s, _| {
        let mut st = s.core.status();
        st.loaded = Some((Engine::Whisper, "model.bin".into(), "vad.bin".into()));
        st.backend = "CPU".into();
    });
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let (r, _) = rt.presented("close").unwrap();
    assert!(r[0] + r[2] <= 560.0);
}

#[test]
fn header_uses_requested_order() {
    let mut rt = runtime((700.0, 700.0));
    rt.update(|s, _| {
        let mut st = s.core.status();
        st.loaded = Some((Engine::GoogleChirp3, "".into(), "".into()));
        st.phase = crate::Phase::CloudIdle;
    });
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let mut right = 0.0;
    for key in [
        "model-badge",
        "status",
        "version",
        "github",
        "home",
        "minimize",
        "close",
    ] {
        let (r, _) = rt.presented(key).unwrap();
        assert!(r[0] >= right, "header order: {key}");
        right = r[0] + r[2];
    }
    assert!(right <= 700.0);
}

#[test]
fn dropdown_options_have_room_for_full_labels() {
    for size in [(560.0, 600.0), (700.0, 700.0)] {
        let mut rt = runtime(size);
        rt.update(|_, tx| tx.scroll_to_bottom_instant("content"));
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        let (x, y) = rt.find("close-action-trigger").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.menu.as_deref(), Some("close-action"));
        let (menu, _) = rt.presented("close-action-content").unwrap();
        assert!(menu[2] >= 400.0);
        assert!(menu[0] >= 0.0 && menu[0] + menu[2] <= size.0);
        let (option, _) = rt.presented("option2").unwrap();
        assert!(option[2] >= 390.0);
    }
}

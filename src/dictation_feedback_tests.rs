use super::*;

#[test]
fn notice_follows_session_state() {
    let invalid_project = cloud::validate(&config::GoogleConfig::default()).unwrap_err();
    let error = invalid_project.as_str();
    let silence = controller::NO_SPEECH_NOTICE;
    for english in [false, true] {
        assert_eq!(
            dictation_notice(Phase::Error, error, silence, english).unwrap(),
            if english {
                "Check your Google Cloud Project ID in Settings."
            } else {
                "Periksa Project ID Google Cloud di Pengaturan."
            }
        );
        for phase in [Phase::Ready, Phase::CloudIdle] {
            assert_eq!(
                dictation_notice(phase, error, silence, english).as_deref(),
                Some(if english {
                    "No speech detected. Try dictating again."
                } else {
                    silence
                })
            );
            assert!(dictation_notice(phase, error, "", english).is_none());
        }
        assert_eq!(
            dictation_notice(Phase::MissingModel, "", "", english).as_deref(),
            Some(if english {
                "Load ASR and VAD models in Settings first."
            } else {
                controller::MISSING_MODEL_NOTICE
            })
        );
        for phase in [
            Phase::Loading,
            Phase::Starting,
            Phase::Recording,
            Phase::Transcribing,
        ] {
            assert!(dictation_notice(phase, error, silence, english).is_none());
        }
    }
}

#[test]
fn error_text_stays_inside_the_original_meter_space() {
    use egui::epaint::Shape;
    let mut errors: Vec<_> = ui_messages().iter().map(|&(id, _)| id.to_owned()).collect();
    // Backend dapat menambahkan path, pesan driver, atau nomor format pada awalan ini.
    for &(prefix, _) in ui_messages() {
        if prefix.ends_with(':')
            || [
                "Konfigurasi mikrofon tidak dapat dibaca",
                "Mikrofon tidak dapat dimulai",
                "Stream mikrofon gagal dibuat",
                "Mikrofon bermasalah",
            ]
            .contains(&prefix)
        {
            errors.push(format!("{prefix} {}", "backend-detail/".repeat(40)));
        }
    }
    errors.extend([
        String::new(),
        "ADC belum tersedia. Jalankan gcloud auth application-default login.".into(),
        "Login ADC gagal. Jalankan ulang gcloud auth application-default login.".into(),
        "Mesin lokal sudah berhenti. Muat ulang model di Pengaturan.".into(),
        "Model VAD gagal dimuat (invalid header). Gunakan model Silero GGML untuk whisper.cpp."
            .into(),
        "Model ASR gagal dimuat (invalid header). Pastikan file GGML whisper.cpp yang benar."
            .into(),
        "State ASR gagal dibuat (allocation failed); memori GPU/RAM mungkin tidak cukup.".into(),
        "Model Qwen3-ASR gagal dimuat (tensor shape mismatch).".into(),
        "Folder Qwen3-ASR tidak lengkap: Shard model-00002 tidak ditemukan (models/qwen).".into(),
        format!("File model ASR tidak ditemukan: {}", "模型🦀".repeat(128)),
        format!(
            "Mesin ASR berhenti ({}). Muat ulang model di Pengaturan.",
            "backend\n".repeat(128)
        ),
        "X".repeat(4096),
    ]);
    for scale in [1.0, 1.25, 1.5, 2.0] {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        for width in [252.0, 180.0] {
            let render = |phase: Phase, notice: Option<&str>, original: bool| {
                let mut geometry = [egui::Rect::NOTHING; 3];
                let mut input = egui::RawInput::default();
                input
                    .viewports
                    .get_mut(&egui::ViewportId::ROOT)
                    .unwrap()
                    .native_pixels_per_point = Some(scale);
                let mut output = ctx.run_ui(input, |ui| {
                    geometry[0] = card()
                        .show(ui, |ui| {
                            ui.set_width(width);
                            ui.label("00:00");
                            let top = ui.cursor().top();
                            geometry[1] = egui::Rect::from_min_size(
                                egui::pos2(ui.min_rect().left(), top),
                                vec2(width, 40.0),
                            );
                            if original {
                                ui.add_space(18.0);
                                ui.allocate_exact_size(vec2(width, 4.0), egui::Sense::hover());
                                ui.add_space(18.0);
                            } else {
                                dictation_feedback(ui, phase, 0.5, notice);
                            }
                            geometry[2] =
                                dictation_button(ui, true, Some(ACCENT), 120.0, "Mulai dikte").rect;
                        })
                        .response
                        .rect;
                });
                output.textures_delta.clear();
                if let Some(text) = notice {
                    let painted = output
                        .shapes
                        .iter()
                        .find_map(|s| match &s.shape {
                            Shape::Text(t) if t.galley.job.text == text => Some((s.clip_rect, t)),
                            _ => None,
                        })
                        .expect("pesan harus benar-benar digambar");
                    let (clip, t) = painted;
                    assert!((1..=2).contains(&t.galley.rows.len()));
                    assert!(geometry[1].expand(0.1).contains_rect(clip));
                    assert!(
                        clip.expand(0.1)
                            .contains_rect(t.galley.rect.translate(t.pos.to_vec2())),
                        "scale={scale}, width={width}, text={text}"
                    );
                }
                geometry
            };
            let original = render(Phase::Ready, None, true);
            for phase in [
                Phase::Ready,
                Phase::Loading,
                Phase::Recording,
                Phase::Transcribing,
            ] {
                assert_eq!(render(phase, None, false), original);
            }
            for english in [false, true] {
                for error in &errors {
                    let text = dictation_notice(Phase::Error, error, "", english).unwrap();
                    assert_eq!(render(Phase::Error, Some(&text), false), original);
                }
            }
        }
    }
}

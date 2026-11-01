//! Google STT V2. Audio, transkrip, dan token hanya berada di RAM.
use crate::config::{self, GoogleConfig, Language};
use googleapis_tonic_google_cloud_speech_v2::google::cloud::speech::v2 as api;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;

const AUDIO_BUFFER_CHUNKS: usize = 200;
static TOKEN_CACHE: Mutex<Option<CachedToken>> = Mutex::new(None);

struct CachedToken {
    adc: Vec<u8>,
    access: String,
    usable_until: Instant,
}
impl CachedToken {
    fn get(&self, adc: &[u8], now: Instant) -> Option<String> {
        (self.adc == adc && now < self.usable_until).then(|| self.access.clone())
    }
}

pub struct Session {
    pub audio: Option<mpsc::Sender<Vec<u8>>>,
    cancel: watch::Sender<bool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Session {
    pub fn start(
        settings: GoogleConfig,
        language: Language,
        done: impl FnOnce(Result<String, String>) + Send + 'static,
    ) -> Result<Self, String> {
        validate(&settings)?;
        let reservation = reserve(&settings)?;
        let (audio, rx) = mpsc::channel(AUDIO_BUFFER_CHUNKS); // 20 detik: startup token + koneksi sampai 16 detik.
        let (cancel, mut cancelled) = watch::channel(false);
        let sent = Arc::new(AtomicU64::new(0));
        let thread = std::thread::Builder::new().name("fi-whisper-cloud".into()).spawn(move || {
            let result = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => {
                    let result = runtime.block_on(async {
                    tokio::select! {
                        biased;
                        _ = cancelled.changed() => Err("Sesi cloud dibatalkan.".into()),
                        result = tokio::time::timeout(Duration::from_secs(165), recognize(&settings, language, rx, sent.clone())) =>
                            result.unwrap_or_else(|_| Err("Batas waktu cloud terlampaui.".into())),
                    }
                    });
                    // Pembacaan ADC yang sedang berjalan tidak bisa dibatalkan oleh Tokio.
                    // Jangan menahan penutupan sesi sambil menunggu I/O tersebut.
                    runtime.shutdown_background();
                    result
                }
                Err(_) => Err("Runtime cloud gagal dibuat.".into()),
            };
            let usage = settle(reservation, sent.load(Ordering::Relaxed).div_ceil(32000));
            done(usage.and(result));
        }).map_err(|_| "Thread cloud gagal dibuat; reservasi penggunaan tetap dihitung.".to_owned())?;
        Ok(Self {
            audio: Some(audio),
            cancel,
            thread: Some(thread),
        })
    }
    pub fn finish(&mut self) {
        self.audio.take();
    }
    pub fn cancel(&self) {
        let _ = self.cancel.send(true);
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.cancel();
        self.audio.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn validate(c: &GoogleConfig) -> Result<(), String> {
    if c.project.len() < 6
        || c.project.len() > 63
        || !c
            .project
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("Project ID Google Cloud tidak valid.".into());
    }
    if !matches!(c.region.as_str(), "us" | "eu") {
        return Err("Region Chirp 3 harus us atau eu.".into());
    }
    Ok(())
}

async fn token() -> Result<String, String> {
    let path = std::env::var_os("GOOGLE_APPLICATION_CREDENTIALS")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("APPDATA").map(|p| {
                std::path::PathBuf::from(p).join("gcloud/application_default_credentials.json")
            })
        })
        .ok_or("Lokasi ADC tidak tersedia.")?;
    // Hanya ADC authorized_user dari gcloud; tidak menjalankan credential process eksternal.
    let bytes = tokio::task::spawn_blocking(move || std::fs::read(path))
        .await
        .map_err(|_| "Pembacaan ADC gagal.")?
        .map_err(|_| "ADC belum tersedia. Jalankan gcloud auth application-default login.")?;
    {
        let mut cache = TOKEN_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(access) = cache.as_ref().and_then(|t| t.get(&bytes, Instant::now())) {
            return Ok(access);
        }
        *cache = None;
    }
    let adc: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "File ADC tidak valid.")?;
    if adc["type"] != "authorized_user" {
        return Err("Gunakan ADC pengguna dari gcloud auth application-default login.".into());
    }
    let field = |key: &str| {
        adc[key]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("ADC tidak lengkap.")
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "TLS autentikasi gagal dibuat.")?;
    let requested_at = Instant::now();
    let response = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", field("client_id")?),
            ("client_secret", field("client_secret")?),
            ("refresh_token", field("refresh_token")?),
        ])
        .send()
        .await
        .map_err(|_| "Tidak dapat menghubungi Google untuk autentikasi.")?;
    if !response.status().is_success() {
        return Err(
            "Login ADC gagal. Jalankan ulang gcloud auth application-default login.".into(),
        );
    }
    let value: serde_json::Value = response
        .json()
        .await
        .map_err(|_| "Respons autentikasi tidak valid.")?;
    let access = value["access_token"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("Token ADC tidak tersedia.")?
        .to_owned();
    // Sisakan waktu untuk koneksi, sesi 120 detik, dan hasil final.
    if let Some(lifetime) = value["expires_in"]
        .as_u64()
        .and_then(|s| s.checked_sub(180))
        && let Some(usable_until) = requested_at.checked_add(Duration::from_secs(lifetime))
    {
        *TOKEN_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some(CachedToken {
            adc: bytes,
            access: access.clone(),
            usable_until,
        });
    }
    Ok(access)
}

async fn recognize(
    c: &GoogleConfig,
    language: Language,
    mut audio: mpsc::Receiver<Vec<u8>>,
    sent: Arc<AtomicU64>,
) -> Result<String, String> {
    use api::streaming_recognize_request::StreamingRequest;
    let access = token().await?;
    let endpoint = tonic::transport::Endpoint::from_shared(format!(
        "https://{}-speech.googleapis.com",
        c.region
    ))
    .map_err(|_| "Endpoint tidak valid.")?
    .tls_config(tonic::transport::ClientTlsConfig::new().with_native_roots())
    .map_err(|_| "TLS cloud gagal dibuat.")?
    .connect_timeout(Duration::from_secs(8));
    let channel = endpoint
        .connect()
        .await
        .map_err(|_| "Google Cloud tidak terhubung. Periksa internet.")?;
    let mut client = api::speech_client::SpeechClient::new(channel);
    let (tx, rx) = mpsc::channel(2);
    tx.send(api::StreamingRecognizeRequest {
        recognizer: format!(
            "projects/{}/locations/{}/recognizers/_",
            c.project, c.region
        ),
        streaming_request: Some(StreamingRequest::StreamingConfig(
            api::StreamingRecognitionConfig {
                config: Some(api::RecognitionConfig {
                    model: "chirp_3".into(),
                    language_codes: vec![language.chirp_code().into()],
                    decoding_config: Some(
                        api::recognition_config::DecodingConfig::ExplicitDecodingConfig(
                            api::ExplicitDecodingConfig {
                                encoding: api::explicit_decoding_config::AudioEncoding::Linear16
                                    as i32,
                                sample_rate_hertz: 16000,
                                audio_channel_count: 1,
                            },
                        ),
                    ),
                    features: Some(api::RecognitionFeatures {
                        enable_automatic_punctuation: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )),
    })
    .await
    .map_err(|_| "Stream cloud tertutup.")?;
    let mut request = tonic::Request::new(ReceiverStream::new(rx));
    request.set_timeout(Duration::from_secs(145));
    request.metadata_mut().insert(
        "authorization",
        format!("Bearer {access}")
            .parse()
            .map_err(|_| "Token tidak valid.")?,
    );
    request.metadata_mut().insert(
        "x-goog-user-project",
        c.project.parse().map_err(|_| "Project ID tidak valid.")?,
    );
    let resource = format!(
        "recognizer=projects/{}/locations/{}/recognizers/_",
        c.project, c.region
    );
    request.metadata_mut().insert(
        "x-goog-request-params",
        resource.parse().map_err(|_| "Recognizer tidak valid.")?,
    );
    let (finished, mut finish_rx) = watch::channel(false);
    let sending = async {
        let mut next = tokio::time::Instant::now();
        while let Some(bytes) = audio.recv().await {
            tokio::time::sleep_until(next).await;
            let len = bytes.len();
            // Catat konservatif sebelum menyerahkan audio ke transport, termasuk sesi gagal.
            sent.fetch_add(len as u64, Ordering::Relaxed);
            tx.send(api::StreamingRecognizeRequest {
                recognizer: String::new(),
                streaming_request: Some(StreamingRequest::Audio(bytes)),
            })
            .await
            .map_err(|_| "Pengiriman audio terputus.".to_owned())?;
            // Jadwal tetap mencegah keterlambatan timer menumpuk sepanjang rekaman.
            next += Duration::from_secs_f64(len as f64 / 32000.0);
        }
        drop(tx);
        let _ = finished.send(true);
        Ok::<_, String>(())
    };
    let receiving = async {
        let mut stream = client
            .streaming_recognize(request)
            .await
            .map_err(status)?
            .into_inner();
        let mut text = String::new();
        let mut last_end = None;
        let mut deadline = None;
        loop {
            let item = tokio::select! {
                biased;
                _ = finish_rx.changed(), if deadline.is_none() => { deadline = Some(tokio::time::Instant::now() + Duration::from_secs(20)); continue; }
                _ = async { if let Some(d) = deadline { tokio::time::sleep_until(d).await; } else { std::future::pending::<()>().await; } } => return Err("Hasil final cloud melewati batas waktu.".into()),
                item = stream.message() => item.map_err(status)?,
            };
            let Some(response) = item else {
                if !*finish_rx.borrow() {
                    return Err("Stream cloud berhenti sebelum rekaman selesai.".into());
                }
                break;
            };
            for result in response.results.into_iter().filter(|r| r.is_final) {
                if let Some(offset) = result.result_end_offset {
                    let end = (offset.seconds, offset.nanos);
                    if last_end.is_some_and(|last| end <= last) {
                        continue;
                    }
                    last_end = Some(end);
                }
                if let Some(alt) = result.alternatives.first() {
                    let segment = alt.transcript.trim();
                    if !segment.is_empty() {
                        if !text.is_empty() {
                            text.push(' ');
                        }
                        text.push_str(segment);
                    }
                }
            }
        }
        Ok(text)
    };
    let (_, text) = tokio::try_join!(sending, receiving)?;
    Ok(text)
}
fn status(s: tonic::Status) -> String {
    if s.code() == tonic::Code::Unauthenticated {
        *TOKEN_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    // Detail server bisa berisi data request. Hanya kode aman yang ditampilkan.
    match s.code() {
        tonic::Code::Unauthenticated => "Login diperlukan. Perbarui ADC.",
        tonic::Code::PermissionDenied => "Periksa izin Speech Client, API, dan billing project.",
        tonic::Code::ResourceExhausted => "Kuota atau kapasitas Google Cloud habis.",
        tonic::Code::InvalidArgument => "Konfigurasi audio, region, atau model ditolak Google.",
        tonic::Code::DeadlineExceeded => "Batas waktu Google Cloud terlampaui.",
        _ => "Transkripsi cloud gagal. Periksa koneksi dan konfigurasi project.",
    }
    .into()
}

#[derive(Default, Serialize, Deserialize)]
struct Usage {
    months: BTreeMap<String, BTreeMap<String, u64>>,
}
fn month() -> String {
    let mut t = windows_sys::Win32::Foundation::SYSTEMTIME::default();
    unsafe {
        windows_sys::Win32::System::SystemInformation::GetSystemTime(&mut t);
    }
    format!("{:04}-{:02}", t.wYear, t.wMonth)
}
fn read_usage() -> Result<Usage, String> {
    let path = config::app_dir()
        .ok_or("Lokasi penggunaan tidak tersedia.")?
        .join("cloud-usage.json");
    match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b)
            .map_err(|_| "Penghitung penggunaan rusak; cloud diblokir.".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Usage::default()),
        Err(_) => Err("Penghitung penggunaan tidak dapat dibaca.".into()),
    }
}
fn save_usage(u: &Usage) -> Result<(), String> {
    use std::io::Write;
    let dir = config::app_dir().ok_or("Lokasi penggunaan tidak tersedia.")?;
    let save = || -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        let temp = dir.join("cloud-usage.json.tmp");
        let mut f = std::fs::File::create(&temp)?;
        f.write_all(&serde_json::to_vec(u)?)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(temp, dir.join("cloud-usage.json"))
    };
    save().map_err(|_| "Penghitung penggunaan gagal disimpan; cloud diblokir.".into())
}
pub fn usage_seconds(project: &str) -> Result<u64, String> {
    Ok(read_usage()?
        .months
        .get(&month())
        .and_then(|p| p.get(project))
        .copied()
        .unwrap_or(0))
}
struct Reservation {
    month: String,
    project: String,
}
fn reserve(c: &GoogleConfig) -> Result<Reservation, String> {
    let mut usage = read_usage()?;
    let month = month();
    let seconds = usage
        .months
        .entry(month.clone())
        .or_default()
        .entry(c.project.clone())
        .or_default();
    if seconds.saturating_add(120) > u64::from(c.monthly_limit_minutes) * 60 {
        return Err("Batas bulanan tercapai atau sisa kurang dari 2 menit.".into());
    }
    *seconds += 120;
    save_usage(&usage)?;
    Ok(Reservation {
        month,
        project: c.project.clone(),
    })
}
fn settle(r: Reservation, seconds: u64) -> Result<(), String> {
    let mut usage = read_usage()?;
    let total = usage
        .months
        .entry(r.month)
        .or_default()
        .entry(r.project)
        .or_default();
    *total = total.saturating_sub(120).saturating_add(seconds);
    save_usage(&usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_cache_checks_credentials_and_expiry() {
        let now = Instant::now();
        let cached = CachedToken {
            adc: b"test-credentials".to_vec(),
            access: "test-token".into(),
            usable_until: now + Duration::from_secs(30),
        };
        assert_eq!(
            cached.get(b"test-credentials", now).as_deref(),
            Some("test-token")
        );
        assert!(cached.get(b"changed-credentials", now).is_none());
        assert!(
            cached
                .get(b"test-credentials", cached.usable_until)
                .is_none()
        );
    }

    #[test]
    fn startup_buffer_preserves_audio_during_auth_and_connect() {
        // Modelkan 8 detik autentikasi + 8 detik koneksi tanpa konsumsi audio.
        let (tx, mut rx) = mpsc::channel(AUDIO_BUFFER_CHUNKS);
        for n in 0..160u16 {
            tx.try_send(vec![(n % 256) as u8; 3200])
                .expect("startup buffer");
        }
        drop(tx);
        for n in 0..160u16 {
            assert_eq!(rx.try_recv().unwrap(), vec![(n % 256) as u8; 3200]);
        }
        assert!(rx.try_recv().is_err());
        let (tx, _rx) = mpsc::channel(AUDIO_BUFFER_CHUNKS);
        for _ in 0..AUDIO_BUFFER_CHUNKS {
            tx.try_send(vec![0; 3200]).unwrap();
        }
        assert!(matches!(
            tx.try_send(vec![0; 3200]),
            Err(mpsc::error::TrySendError::Full(_))
        ));
    }

    fn settings() -> GoogleConfig {
        let project = std::env::var("FI_WHISPER_TEST_PROJECT")
            .expect("Set FI_WHISPER_TEST_PROJECT untuk opt-in pengujian berbayar.");
        GoogleConfig {
            project,
            ..Default::default()
        }
    }
    fn samples(name: &str) -> Vec<f32> {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures/fleurs-id")
                .join(name),
        )
        .unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 16000);
        bytes[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
            .collect()
    }
    fn run(c: &GoogleConfig, samples: &[f32]) -> String {
        let chunks = crate::audio::fixture_pcm(samples, 16000);
        let before = usage_seconds(&c.project).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut session = Session::start(c.clone(), Language::Indonesian, move |result| {
            tx.send(result).unwrap();
        })
        .unwrap();
        let started = Instant::now();
        for chunk in chunks {
            let duration = Duration::from_secs_f64(chunk.len() as f64 / 32000.0);
            session
                .audio
                .as_ref()
                .unwrap()
                .try_send(chunk)
                .expect("capture buffer");
            std::thread::sleep(duration);
        }
        let stopped = Instant::now();
        session.finish();
        let text = rx
            .recv_timeout(Duration::from_secs(35))
            .expect("final timeout")
            .expect("cloud recognition");
        let after = usage_seconds(&c.project).unwrap();
        let seconds = samples.len().div_ceil(16000) as u64;
        assert_eq!(after - before, seconds, "usage settlement");
        eprintln!(
            "cloud audio={:.3}s post_stop={:.3}s total={:.3}s chars={} counted={}s",
            samples.len() as f64 / 16000.0,
            stopped.elapsed().as_secs_f64(),
            started.elapsed().as_secs_f64(),
            text.chars().count(),
            after - before
        );
        text
    }
    fn words(s: &str) -> Vec<String> {
        s.to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    }
    fn distance(a: &[String], b: &[String]) -> usize {
        let mut row: Vec<usize> = (0..=b.len()).collect();
        for (i, x) in a.iter().enumerate() {
            let mut diag = row[0];
            row[0] = i + 1;
            for (j, y) in b.iter().enumerate() {
                let prev = row[j + 1];
                row[j + 1] = (diag + usize::from(x != y)).min(row[j] + 1).min(prev + 1);
                diag = prev;
            }
        }
        row[b.len()]
    }
    #[test]
    fn invalid_cloud_settings_are_rejected_before_authentication() {
        assert!(validate(&GoogleConfig::default()).is_err());
        let mut c: GoogleConfig =
            serde_json::from_str(r#"{"project":"test-project","enabled":false}"#).unwrap();
        assert!(validate(&c).is_ok());
        assert!(serde_json::to_value(&c).unwrap().get("enabled").is_none());
        c.region = "localhost".into();
        assert!(validate(&c).is_err());
    }
    #[test]
    #[ignore = "ADC dan FI_WHISPER_TEST_PROJECT diperlukan; memakai kuota Google"]
    fn cloud_functional() {
        let c = settings();
        let manifest: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("fixtures/fleurs-id/manifest.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let mut errors = 0;
        let mut count = 0;
        for sample in manifest["samples"].as_array().unwrap() {
            let file = sample["file"].as_str().unwrap();
            let text = run(&c, &samples(file));
            assert!(!text.trim().is_empty());
            let reference = words(sample["reference"].as_str().unwrap());
            let hypothesis = words(&text);
            let e = distance(&reference, &hypothesis);
            errors += e;
            count += reference.len();
            eprintln!("{file} word_errors={e}/{}", reference.len());
        }
        eprintln!(
            "cloud aggregate WER={:.2}% ({errors}/{count})",
            100.0 * errors as f64 / count as f64
        );
        assert!(
            run(&c, &vec![0.0; 48000]).trim().is_empty(),
            "silence hallucination"
        );
        let mut limited = c.clone();
        limited.monthly_limit_minutes = 0;
        assert!(
            Session::start(limited, Language::Indonesian, |_| {}).is_err(),
            "usage limit"
        );
        let before = usage_seconds(&c.project).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let session = Session::start(c.clone(), Language::Indonesian, move |r| {
            tx.send(r).unwrap();
        })
        .unwrap();
        for _ in 0..20 {
            session
                .audio
                .as_ref()
                .unwrap()
                .try_send(vec![0; 3200])
                .unwrap();
            std::thread::sleep(Duration::from_millis(100));
        }
        let at = Instant::now();
        session.cancel();
        drop(session);
        assert!(rx.recv_timeout(Duration::from_secs(3)).unwrap().is_err());
        assert!(at.elapsed() < Duration::from_secs(3));
        let delta = usage_seconds(&c.project).unwrap() - before;
        assert!(delta <= 2);
        eprintln!("cloud cancellation={:?} counted={delta}s", at.elapsed());
    }
    #[test]
    #[ignore = "Streaming 120 detik berbayar; ADC dan FI_WHISPER_TEST_PROJECT diperlukan"]
    fn cloud_120_seconds() {
        let c = settings();
        let source: Vec<f32> = ["id_id_0000.wav", "id_id_0001.wav", "id_id_0002.wav"]
            .iter()
            .flat_map(|n| samples(n))
            .collect();
        let long: Vec<f32> = source.iter().copied().cycle().take(16000 * 120).collect();
        let text = run(&c, &long);
        assert!(
            words(&text).len() > 100,
            "long stream must return multiple utterances"
        );
    }
}

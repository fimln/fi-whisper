use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::util::{assert_sha256, download_verified, extract_zip, http_client};

const WHISPER_SHA256: &str = "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69";
const SILERO_SHA256: &str = "2aa269b785eeb53a82983a20501ddf7c1d9c48e33ab63a41391ac6c9f7fb6987";
const RUNTIME_URL: &str = "https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-win-cuda-12.4.0-x64.zip";
const RUNTIME_SHA256: &str = "afef0b881c500958921c3f5523b50e59ee2ec9b6f5cbd25b324c51ed308a957a";

#[derive(Deserialize)]
struct Manifest {
    mirror: String,
    revision: String,
    samples: Vec<Sample>,
}

#[derive(Deserialize)]
struct Sample {
    id: String,
    file: String,
    sha256: String,
    reference: String,
}

#[derive(Serialize)]
struct SampleResult {
    sample: String,
    backend: String,
    audio_seconds: f64,
    wall_seconds: f64,
    word_errors: usize,
    reference_words: usize,
    wer_percent: f64,
    process_id: u32,
    peak_gpu_percent: Option<u32>,
    peak_device_memory_mib: Option<u32>,
    arguments: Vec<String>,
    reference: String,
    hypothesis: String,
}

struct Options {
    model_dir: PathBuf,
    cpu: bool,
    self_test: bool,
}

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let opts = parse_args(root, args)?;
    if opts.self_test {
        run_self_test()?;
        println!("WER self-test: 6 lulus.");
        return Ok(());
    }

    let asr = opts
        .model_dir
        .join("whisper-large-v3-turbo")
        .join("ggml-large-v3-turbo.bin");
    let vad = opts
        .model_dir
        .join("silero-vad-v6.2.0")
        .join("ggml-silero-v6.2.0.bin");
    assert_sha256(&asr, WHISPER_SHA256)?;
    assert_sha256(&vad, SILERO_SHA256)?;

    let cache = root.join(".local");
    let runtime = cache.join("whisper-b5454");
    let downloads = cache.join("downloads");
    let fixtures = root.join("fixtures").join("fleurs-id");
    let backend = if opts.cpu { "cpu" } else { "cuda" };
    let output = cache
        .join("runs")
        .join(format!("{}-{backend}", utc_timestamp()));
    fs::create_dir_all(&downloads)
        .map_err(|e| format!("gagal membuat {}: {e}", downloads.display()))?;
    fs::create_dir_all(&output).map_err(|e| format!("gagal membuat {}: {e}", output.display()))?;

    let client = http_client()?;
    // Build CUDA resmi juga mendukung -ng untuk pembanding CPU eksplisit.
    let archive = downloads.join("whisper-cuda-b5454.zip");
    download_verified(&client, RUNTIME_URL, &archive, RUNTIME_SHA256, None)?;
    if !runtime.exists() {
        extract_zip(&archive, &runtime, false)?;
    }

    let cli = find_unique_file(&runtime, "whisper-cli.exe")?;
    let manifest_raw = fs::read_to_string(fixtures.join("manifest.json"))
        .map_err(|e| format!("gagal membaca manifest.json: {e}"))?;
    let manifest: Manifest = serde_json::from_str(&manifest_raw)
        .map_err(|e| format!("manifest.json tidak valid: {e}"))?;

    if !opts.cpu {
        let dev = Command::new("nvidia-smi")
            .args([
                "--query-gpu=name,driver_version,compute_cap",
                "--format=csv",
            ])
            .output()
            .map_err(|e| format!("nvidia-smi gagal: {e}"))?;
        if !dev.status.success() {
            return Err(
                "nvidia-smi gagal. Gunakan --cpu hanya bila memang ingin menguji CPU.".to_string(),
            );
        }
        fs::write(output.join("device.txt"), &dev.stdout)
            .map_err(|e| format!("gagal menulis device.txt: {e}"))?;
    }

    let mut results = Vec::with_capacity(manifest.samples.len());
    for sample in &manifest.samples {
        let wav = fixtures.join(&sample.file);
        let url = format!(
            "https://huggingface.co/datasets/{}/resolve/{}/id_id/{}",
            manifest.mirror, manifest.revision, sample.file
        );
        download_verified(&client, &url, &wav, &sample.sha256, None)?;

        let prefix = output.join(&sample.id);
        let mut cli_args = vec![
            "-m".to_string(),
            asr.display().to_string(),
            "-vm".to_string(),
            vad.display().to_string(),
            "--vad".to_string(),
            "-l".to_string(),
            "id".to_string(),
            "-bs".to_string(),
            "1".to_string(),
            "-bo".to_string(),
            "1".to_string(),
            "-t".to_string(),
            "8".to_string(),
            "-nt".to_string(),
            "-otxt".to_string(),
            "-of".to_string(),
            prefix.display().to_string(),
            "-f".to_string(),
            wav.display().to_string(),
        ];
        if opts.cpu {
            cli_args.push("-ng".to_string());
        }

        let stdout_path = PathBuf::from(format!("{}.stdout.log", prefix.display()));
        let stderr_path = PathBuf::from(format!("{}.stderr.log", prefix.display()));
        let gpu_path = PathBuf::from(format!("{}.gpu.csv", prefix.display()));
        let txt_path = PathBuf::from(format!("{}.txt", prefix.display()));

        let stdout_file = File::create(&stdout_path)
            .map_err(|e| format!("gagal membuat {}: {e}", stdout_path.display()))?;
        let stderr_file = File::create(&stderr_path)
            .map_err(|e| format!("gagal membuat {}: {e}", stderr_path.display()))?;

        let started = Instant::now();
        let mut child = Command::new(&cli)
            .args(&cli_args)
            .stdout(stdout_file)
            .stderr(stderr_file)
            .spawn()
            .map_err(|e| format!("gagal menjalankan {}: {e}", cli.display()))?;
        let pid = child.id();

        let mut telemetry = Vec::new();
        let wait_res = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => {
                    if !opts.cpu {
                        match query_gpu_telemetry() {
                            Ok(line) => telemetry.push(line),
                            Err(err) => break Err(err),
                        }
                    }
                    if started.elapsed() > Duration::from_secs(180) {
                        break Err("Inferensi melampaui batas 180 detik.".to_string());
                    }
                    thread::sleep(Duration::from_millis(150));
                }
                Err(e) => break Err(format!("gagal menunggu whisper-cli: {e}")),
            }
        };

        if wait_res.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::write(&gpu_path, telemetry.join("\n"));
        let status = wait_res?;
        let wall_seconds = round3(started.elapsed().as_secs_f64());

        if !status.success() {
            return Err(format!(
                "whisper-cli exit {}. Baca {}",
                status.code().unwrap_or(-1),
                stderr_path.display()
            ));
        }

        let log = fs::read_to_string(&stderr_path)
            .map_err(|e| format!("gagal membaca {}: {e}", stderr_path.display()))?;
        verify_backend_log(&log, opts.cpu, &stderr_path)?;

        let hypothesis = fs::read_to_string(&txt_path)
            .map_err(|e| format!("gagal membaca {}: {e}", txt_path.display()))?
            .trim()
            .to_string();
        if hypothesis.is_empty() {
            return Err(format!("Hasil ucapan kosong: {}", sample.id));
        }

        let (errors, words) = word_error(&sample.reference, &hypothesis);
        let (peak_util, peak_mem) = parse_peak_gpu(&telemetry);
        if !opts.cpu && !peak_util.is_some_and(|v| v > 0) {
            return Err(format!("Aktivitas GPU belum teramati: {}", sample.id));
        }

        let wav_bytes =
            fs::read(&wav).map_err(|e| format!("gagal membaca {}: {e}", wav.display()))?;
        let audio_seconds = validate_pcm16_wav(&wav_bytes, &wav)?;
        let wer_percent = if words == 0 {
            0.0
        } else {
            (100.0 * errors as f64 / words as f64 * 100.0).round() / 100.0
        };

        println!("Selesai {}: {errors}/{words} word errors.", sample.id);
        results.push(SampleResult {
            sample: sample.id.clone(),
            backend: backend.to_string(),
            audio_seconds,
            wall_seconds,
            word_errors: errors,
            reference_words: words,
            wer_percent,
            process_id: pid,
            peak_gpu_percent: peak_util,
            peak_device_memory_mib: peak_mem,
            arguments: cli_args,
            reference: sample.reference.clone(),
            hypothesis,
        });
    }

    let json = serde_json::to_string_pretty(&results)
        .map_err(|e| format!("gagal serialize hasil JSON: {e}"))?;
    fs::write(output.join("results.json"), json)
        .map_err(|e| format!("gagal menulis results.json: {e}"))?;

    println!(
        "\n{:<12} {:<6} {:>10} {:>10} {:>8}",
        "sample", "backend", "audio_s", "wall_s", "wer_%"
    );
    for r in &results {
        println!(
            "{:<12} {:<6} {:>10.3} {:>10.3} {:>8.2}",
            r.sample, r.backend, r.audio_seconds, r.wall_seconds, r.wer_percent
        );
    }
    println!("Hasil, log backend, dan telemetry: {}", output.display());
    Ok(())
}

fn parse_args(root: &Path, args: &[String]) -> Result<Options, String> {
    let mut opts = Options {
        model_dir: root.join("models"),
        cpu: false,
        self_test: false,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--cpu" => opts.cpu = true,
            "--self-test" => opts.self_test = true,
            "--model-dir" => {
                i += 1;
                let val = args
                    .get(i)
                    .ok_or_else(|| "--model-dir membutuhkan path".to_string())?;
                opts.model_dir = PathBuf::from(val);
            }
            other => {
                return Err(format!(
                    "argumen test-model tidak dikenal: `{other}` (gunakan --model-dir, --cpu, --self-test)"
                ));
            }
        }
        i += 1;
    }
    Ok(opts)
}

pub fn word_error(reference: &str, hypothesis: &str) -> (usize, usize) {
    let ref_words = tokenize(reference);
    let hyp_words = tokenize(hypothesis);
    let mut previous: Vec<usize> = (0..=hyp_words.len()).collect();
    for (i, r) in ref_words.iter().enumerate() {
        let mut current = vec![0usize; hyp_words.len() + 1];
        current[0] = i + 1;
        for (j, h) in hyp_words.iter().enumerate() {
            let cost = usize::from(r != h);
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + cost);
        }
        previous = current;
    }
    (previous[hyp_words.len()], ref_words.len())
}

fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in text.to_lowercase().chars() {
        if ch.is_alphanumeric() {
            current.push(ch);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn run_self_test() -> Result<(), String> {
    let cases = [
        ("Halo, INDONESIA!", "halo indonesia", 0, 2),
        ("satu dua", "satu tiga", 1, 2),
        ("satu dua", "satu", 1, 2),
        ("satu", "satu dua", 1, 1),
        ("tim-tim", "tim tim", 0, 2),
        ("satu", "", 1, 1),
    ];
    for (reference, hypothesis, exp_err, exp_words) in cases {
        let (errors, words) = word_error(reference, hypothesis);
        if errors != exp_err || words != exp_words {
            return Err(format!(
                "WER self-test gagal: {reference} (dapat {errors}/{words}, harap {exp_err}/{exp_words})"
            ));
        }
    }
    Ok(())
}

fn verify_backend_log(log: &str, cpu: bool, stderr_path: &Path) -> Result<(), String> {
    let has_cuda = log.lines().any(|line| {
        if let Some(rest) = line.split("using CUDA").nth(1) {
            let digits = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
            digits > 0 && rest[digits..].starts_with(" backend")
        } else {
            false
        }
    });
    if !cpu && !has_cuda {
        return Err(format!(
            "CUDA tidak terbukti aktif. Baca {}",
            stderr_path.display()
        ));
    }
    if cpu {
        let has_use_gpu_0 = log.lines().any(|line| {
            line.split("use gpu")
                .nth(1)
                .map(|r| r.trim_start())
                .and_then(|r| r.strip_prefix('='))
                .map(|r| r.trim_start())
                .is_some_and(|r| r.starts_with('0'))
        });
        if !has_use_gpu_0 || has_cuda {
            return Err(format!(
                "Mode CPU tidak terkonfirmasi. Baca {}",
                stderr_path.display()
            ));
        }
    }
    Ok(())
}

fn query_gpu_telemetry() -> Result<String, String> {
    let out = Command::new("nvidia-smi")
        .args([
            "--query-gpu=timestamp,utilization.gpu,memory.used",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .map_err(|e| format!("Pembacaan telemetry GPU gagal: {e}"))?;
    if !out.status.success() {
        return Err("Pembacaan telemetry GPU gagal.".to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn parse_peak_gpu(rows: &[String]) -> (Option<u32>, Option<u32>) {
    let mut peak_util: Option<u32> = None;
    let mut peak_mem: Option<u32> = None;
    for row in rows {
        let mut parts = row.split(',').map(|s| s.trim());
        let _ts = parts.next();
        if let Some(u) = parts.next().and_then(|s| s.parse::<u32>().ok()) {
            peak_util = Some(peak_util.map_or(u, |p| p.max(u)));
        }
        if let Some(m) = parts.next().and_then(|s| s.parse::<u32>().ok()) {
            peak_mem = Some(peak_mem.map_or(m, |p| p.max(m)));
        }
    }
    (peak_util, peak_mem)
}

fn validate_pcm16_wav(audio: &[u8], path: &Path) -> Result<f64, String> {
    // shortcut: fixtures pinned berupa PCM16 mono 16 kHz dengan header WAV 44 byte;
    // gunakan parser WAV saat menerima file lain.
    if audio.len() < 44
        || &audio[0..4] != b"RIFF"
        || u16::from_le_bytes([audio[22], audio[23]]) != 1
        || u32::from_le_bytes([audio[24], audio[25], audio[26], audio[27]]) != 16000
        || u16::from_le_bytes([audio[34], audio[35]]) != 16
        || &audio[36..40] != b"data"
    {
        return Err(format!("Format fixture berubah: {}", path.display()));
    }
    Ok(round3((audio.len() - 44) as f64 / 32000.0))
}

fn find_unique_file(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let mut found = Vec::new();
    walk_find(dir, name, &mut found)?;
    if found.len() != 1 {
        return Err(format!(
            "Tidak ditemukan tepat satu {name} dalam runtime (ditemukan {}).",
            found.len()
        ));
    }
    Ok(found.remove(0))
}

fn walk_find(dir: &Path, name: &str, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("gagal membaca {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("gagal membaca entri: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            walk_find(&path, name, out)?;
        } else if path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
        {
            out.push(path);
        }
    }
    Ok(())
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn utc_timestamp() -> String {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = dur.as_secs();
    let millis = dur.subsec_millis();
    let days = (total_secs / 86_400) as i64;
    let secs_of_day = total_secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}-{millis:03}")
}

// Algoritma Howard Hinnant untuk konversi hari sejak 1970-01-01 ke tanggal sipil UTC.
fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + i64::from(m <= 2);
    (year, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wer_self_test_passes() {
        run_self_test().unwrap();
    }

    #[test]
    fn backend_log_detection() {
        let p = Path::new("stderr.log");
        assert!(verify_backend_log("ggml_cuda_init: using CUDA0 backend\n", false, p).is_ok());
        assert!(verify_backend_log("use gpu = 0\n", false, p).is_err());
        assert!(verify_backend_log("whisper_init: use gpu = 0\n", true, p).is_ok());
        assert!(verify_backend_log("use gpu = 0\nusing CUDA0 backend\n", true, p).is_err());
    }

    #[test]
    fn telemetry_peak_parsing() {
        let rows = vec![
            "2026/10/18 00:00:00.000, 12, 1024".to_string(),
            "2026/10/18 00:00:00.150, 97, 2196".to_string(),
        ];
        assert_eq!(parse_peak_gpu(&rows), (Some(97), Some(2196)));
    }
}

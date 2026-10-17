//! Konfigurasi lokal `%LOCALAPPDATA%\fi-whisper\config.json`.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const ASR_FILE: &str = "ggml-large-v3-turbo.bin";
pub const VAD_FILE: &str = "ggml-silero-v6.2.0.bin";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[default]
    #[serde(rename = "id")]
    Indonesian,
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "en")]
    English,
}

impl Language {
    pub const ALL: [Self; 3] = [Self::Indonesian, Self::Auto, Self::English];

    /// Kode bahasa whisper.cpp; `None` berarti deteksi otomatis (pointer null).
    pub fn code(self) -> Option<&'static str> {
        match self {
            Self::Indonesian => Some("id"),
            Self::Auto => None,
            Self::English => Some("en"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Indonesian => "Indonesia (id)",
            Self::Auto => "Auto",
            Self::English => "English (en)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub asr_model: Option<PathBuf>,
    pub vad_model: Option<PathBuf>,
    /// ID perangkat CPAL; `None` berarti input default sistem.
    pub microphone: Option<String>,
    pub language: Language,
    pub shortcut: String,
    pub close_hint_shown: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            asr_model: None,
            vad_model: None,
            microphone: None,
            language: Language::Indonesian,
            shortcut: "Ctrl+Alt+Space".into(),
            close_hint_shown: false,
        }
    }
}

pub fn app_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("fi-whisper"))
}

pub fn config_path() -> Option<PathBuf> {
    app_dir().map(|d| d.join("config.json"))
}

/// `Ok(None)` jika file belum ada. File rusak dikembalikan sebagai error dan tidak disentuh.
pub fn load(path: &Path) -> Result<Option<Config>, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("Konfigurasi rusak ({e}). File tidak diubah.")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Konfigurasi tidak dapat dibaca: {e}")),
    }
}

/// Tulis ke file sementara di direktori yang sama, lalu ganti file lama.
pub fn save(path: &Path, config: &Config) -> io::Result<()> {
    let dir = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    fs::create_dir_all(dir)?;
    let tmp = path.with_extension("json.tmp");
    let mut file = fs::File::create(&tmp)?;
    file.write_all(&serde_json::to_vec_pretty(config).map_err(io::Error::other)?)?;
    file.sync_all()?;
    drop(file);
    // std::fs::rename memakai MoveFileExW(MOVEFILE_REPLACE_EXISTING) di Windows.
    fs::rename(&tmp, path)
}

/// Lokasi model bawaan bila konfigurasi belum memilih file.
pub fn default_model(file: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = app_dir() {
        candidates.push(dir.join("models").join(file));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_owned))
    {
        candidates.push(dir.join("models").join(file));
    }
    if let Ok(dir) = std::env::current_dir() {
        candidates.push(dir.join("models").join(file));
    }
    candidates.into_iter().find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_roundtrip_and_corrupt_file_is_preserved() {
        let dir = std::env::temp_dir().join(format!("fi-whisper-test-{}", std::process::id()));
        let path = dir.join("config.json");
        let config = Config {
            asr_model: Some(PathBuf::from(r"C:\Model ü\a.bin")),
            language: Language::Auto,
            ..Config::default()
        };
        save(&path, &config).unwrap();
        assert_eq!(load(&path).unwrap(), Some(config));

        fs::write(&path, b"{ rusak").unwrap();
        assert!(load(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{ rusak");
        assert_eq!(load(&dir.join("tidak-ada.json")).unwrap(), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_fields_use_defaults() {
        let config: Config = serde_json::from_str(r#"{"language":"en"}"#).unwrap();
        assert_eq!(config.language, Language::English);
        assert_eq!(config.shortcut, "Ctrl+Alt+Space");
    }
}

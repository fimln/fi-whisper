#![allow(dead_code)] // config fields are part of the model schema; not all are used in inference

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct AsrConfig {
    pub thinker_config: ThinkerConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ThinkerConfig {
    pub audio_config: AudioEncoderConfig,
    pub text_config: TextDecoderConfig,
    #[serde(default = "default_audio_start_token_id")]
    pub audio_start_token_id: i64,
    #[serde(default = "default_audio_end_token_id")]
    pub audio_end_token_id: i64,
    #[serde(default = "default_audio_token_id")]
    pub audio_token_id: i64,
}

fn default_audio_start_token_id() -> i64 { 151669 }
fn default_audio_end_token_id() -> i64 { 151670 }
fn default_audio_token_id() -> i64 { 151676 }

#[derive(Debug, Clone, Deserialize)]
pub struct AudioEncoderConfig {
    #[serde(default = "default_d_model")]
    pub d_model: usize,
    #[serde(default = "default_encoder_layers")]
    pub encoder_layers: usize,
    #[serde(default = "default_encoder_attention_heads")]
    pub encoder_attention_heads: usize,
    #[serde(default = "default_encoder_ffn_dim")]
    pub encoder_ffn_dim: usize,
    #[serde(default = "default_num_mel_bins")]
    pub num_mel_bins: usize,
    #[serde(default = "default_max_source_positions")]
    pub max_source_positions: usize,
    #[serde(default = "default_n_window")]
    pub n_window: usize,
    #[serde(default = "default_n_window_infer")]
    pub n_window_infer: usize,
    #[serde(default = "default_conv_chunksize")]
    pub conv_chunksize: usize,
    #[serde(default = "default_output_dim")]
    pub output_dim: usize,
}

fn default_d_model() -> usize { 896 }
fn default_encoder_layers() -> usize { 18 }
fn default_encoder_attention_heads() -> usize { 14 }
fn default_encoder_ffn_dim() -> usize { 3584 }
fn default_num_mel_bins() -> usize { 128 }
fn default_max_source_positions() -> usize { 1500 }
fn default_n_window() -> usize { 50 }
fn default_n_window_infer() -> usize { 800 }
fn default_conv_chunksize() -> usize { 500 }
fn default_output_dim() -> usize { 1024 }

#[derive(Debug, Clone, Deserialize)]
pub struct TextDecoderConfig {
    #[serde(default = "default_vocab_size")]
    pub vocab_size: usize,
    #[serde(default = "default_hidden_size")]
    pub hidden_size: usize,
    #[serde(default = "default_intermediate_size")]
    pub intermediate_size: usize,
    #[serde(default = "default_num_hidden_layers")]
    pub num_hidden_layers: usize,
    #[serde(default = "default_num_attention_heads")]
    pub num_attention_heads: usize,
    #[serde(default = "default_num_key_value_heads")]
    pub num_key_value_heads: usize,
    #[serde(default = "default_head_dim")]
    pub head_dim: usize,
    #[serde(default = "default_rms_norm_eps")]
    pub rms_norm_eps: f64,
    #[serde(default = "default_rope_theta")]
    pub rope_theta: f64,
    pub rope_scaling: Option<RopeScaling>,
}

fn default_vocab_size() -> usize { 151936 }
fn default_hidden_size() -> usize { 1024 }
fn default_intermediate_size() -> usize { 3072 }
fn default_num_hidden_layers() -> usize { 28 }
fn default_num_attention_heads() -> usize { 16 }
fn default_num_key_value_heads() -> usize { 8 }
fn default_head_dim() -> usize { 128 }
fn default_rms_norm_eps() -> f64 { 1e-6 }
fn default_rope_theta() -> f64 { 1_000_000.0 }

#[derive(Debug, Clone, Deserialize)]
pub struct RopeScaling {
    #[serde(default)]
    pub rope_type: String,
    #[serde(default = "default_mrope_section")]
    pub mrope_section: Vec<usize>,
    #[serde(default)]
    pub interleaved: bool,
    #[serde(default)]
    pub mrope_interleaved: bool,
}

fn default_mrope_section() -> Vec<usize> { vec![24, 20, 20] }

impl AsrConfig {
    /// Returns the config and whether the file uses the Transformers-native (`*-hf`) layout.
    pub fn from_file(path: &std::path::Path) -> anyhow::Result<(Self, bool)> {
        let content = std::fs::read_to_string(path)?;
        Self::from_json(&content)
    }

    /// fi-whisper: accepts both the original `thinker_config` layout and the Transformers-native
    /// layout (`audio_config`/`text_config` at the top level, `rope_parameters`).
    pub fn from_json(content: &str) -> anyhow::Result<(Self, bool)> {
        let (config, hf) = Self::parse_json(content)?;
        config.validate()?;
        Ok((config, hf))
    }

    fn parse_json(content: &str) -> anyhow::Result<(Self, bool)> {
        let mut value: serde_json::Value = serde_json::from_str(content)?;
        if value.get("thinker_config").is_some() {
            return Ok((serde_json::from_value(value)?, false));
        }
        let obj = value
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("config.json is not an object"))?;
        if obj.get("model_type").and_then(|v| v.as_str()) != Some("qwen3_asr") {
            anyhow::bail!("config.json is not a Qwen3-ASR config");
        }
        let mut thinker = serde_json::Map::new();
        for key in ["audio_config", "text_config", "audio_token_id", "audio_start_token_id", "audio_end_token_id"] {
            if let Some(v) = obj.remove(key) {
                thinker.insert(key.into(), v);
            }
        }
        if let Some(text) = thinker.get_mut("text_config").and_then(|v| v.as_object_mut()) {
            let theta = text
                .get("rope_parameters")
                .and_then(|p| p.get("rope_theta"))
                .cloned();
            if let (Some(theta), false) = (theta, text.contains_key("rope_theta")) {
                text.insert("rope_theta".into(), theta);
            }
        }
        let config = serde_json::json!({ "thinker_config": thinker });
        Ok((serde_json::from_value(config)?, true))
    }

    /// fi-whisper: rejects values that would otherwise divide by zero or index out of bounds in
    /// the encoder/decoder (config.json comes from a user-chosen folder).
    pub fn validate(&self) -> anyhow::Result<()> {
        let a = &self.thinker_config.audio_config;
        let t = &self.thinker_config.text_config;
        let check = |ok: bool, msg: &str| if ok { Ok(()) } else { Err(anyhow::anyhow!("invalid config.json: {msg}")) };
        check(a.num_mel_bins > 0, "audio_config.num_mel_bins must be > 0")?;
        check(
            a.encoder_attention_heads > 0 && a.d_model % a.encoder_attention_heads == 0,
            "audio_config.d_model must be a positive multiple of encoder_attention_heads",
        )?;
        let chunk = a.n_window.checked_mul(2).filter(|&c| c > 0);
        check(
            chunk.is_some_and(|c| a.n_window_infer >= c),
            "audio_config.n_window must be > 0 and n_window_infer >= 2 * n_window",
        )?;
        check(t.num_hidden_layers > 0, "text_config.num_hidden_layers must be > 0")?;
        check(
            t.num_key_value_heads > 0
                && t.num_attention_heads > 0
                && t.num_attention_heads % t.num_key_value_heads == 0,
            "text_config.num_attention_heads must be a positive multiple of num_key_value_heads",
        )?;
        check(t.head_dim > 0 && t.head_dim % 2 == 0, "text_config.head_dim must be even and > 0")?;
        let section = t.mrope_section();
        check(
            section.len() == 3 && section.iter().try_fold(0usize, |s, &x| s.checked_add(x)) == Some(t.head_dim / 2),
            "rope_scaling.mrope_section must have 3 entries summing to head_dim / 2",
        )
    }
}

impl TextDecoderConfig {
    pub fn mrope_section(&self) -> Vec<usize> {
        self.rope_scaling
            .as_ref()
            .map(|rs| rs.mrope_section.clone())
            .unwrap_or_else(default_mrope_section)
    }

    pub fn mrope_interleaved(&self) -> bool {
        self.rope_scaling
            .as_ref()
            .map(|rs| rs.mrope_interleaved || rs.interleaved)
            .unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_base_config() -> TextDecoderConfig {
        TextDecoderConfig {
            vocab_size: 151936,
            hidden_size: 1024,
            intermediate_size: 3072,
            num_hidden_layers: 28,
            num_attention_heads: 16,
            num_key_value_heads: 8,
            head_dim: 128,
            rms_norm_eps: 1e-6,
            rope_theta: 1_000_000.0,
            rope_scaling: None,
        }
    }

    #[test]
    fn test_mrope_section_default_when_no_rope_scaling() {
        let cfg = make_base_config();
        assert_eq!(cfg.mrope_section(), vec![24, 20, 20]);
    }

    #[test]
    fn test_mrope_section_custom() {
        let mut cfg = make_base_config();
        cfg.rope_scaling = Some(RopeScaling {
            rope_type: "mrope".to_string(),
            mrope_section: vec![4, 4, 8],
            interleaved: false,
            mrope_interleaved: false,
        });
        assert_eq!(cfg.mrope_section(), vec![4, 4, 8]);
    }

    #[test]
    fn test_mrope_interleaved_default_true_when_no_rope_scaling() {
        let cfg = make_base_config();
        assert!(cfg.mrope_interleaved(), "should default to true when rope_scaling is None");
    }

    #[test]
    fn test_mrope_interleaved_via_mrope_interleaved_flag() {
        let mut cfg = make_base_config();
        cfg.rope_scaling = Some(RopeScaling {
            rope_type: "mrope".to_string(),
            mrope_section: vec![24, 20, 20],
            interleaved: false,
            mrope_interleaved: true,
        });
        assert!(cfg.mrope_interleaved());
    }

    #[test]
    fn test_mrope_interleaved_via_interleaved_flag() {
        let mut cfg = make_base_config();
        cfg.rope_scaling = Some(RopeScaling {
            rope_type: "mrope".to_string(),
            mrope_section: vec![24, 20, 20],
            interleaved: true,
            mrope_interleaved: false,
        });
        assert!(cfg.mrope_interleaved());
    }

    #[test]
    fn hf_layout_is_wrapped_into_thinker_config() {
        let json = r#"{
            "model_type": "qwen3_asr",
            "audio_token_id": 151676,
            "audio_config": {"d_model": 896, "encoder_layers": 18, "output_dim": 1024},
            "text_config": {"hidden_size": 1024, "num_hidden_layers": 28,
                            "rope_parameters": {"rope_theta": 5000, "rope_type": "default"}}
        }"#;
        let (cfg, hf) = AsrConfig::from_json(json).unwrap();
        assert!(hf);
        assert_eq!(cfg.thinker_config.audio_token_id, 151676);
        assert_eq!(cfg.thinker_config.audio_start_token_id, 151669);
        assert_eq!(cfg.thinker_config.text_config.rope_theta, 5000.0);
        assert!(cfg.thinker_config.text_config.mrope_interleaved());
        let (_, hf) = AsrConfig::from_json(
            r#"{"thinker_config": {"audio_config": {}, "text_config": {}}}"#,
        )
        .unwrap();
        assert!(!hf);
        assert!(AsrConfig::from_json(r#"{"model_type": "whisper"}"#).is_err());
    }

    #[test]
    fn hf_weight_names_map_to_original_layout() {
        use crate::inference::remap_hf_name as r;
        assert_eq!(r("model.audio_tower.conv2d1.weight"), "thinker.audio_tower.conv2d1.weight");
        assert_eq!(r("model.multi_modal_projector.linear_1.bias"), "thinker.audio_tower.proj1.bias");
        assert_eq!(r("model.multi_modal_projector.linear_2.weight"), "thinker.audio_tower.proj2.weight");
        assert_eq!(r("model.language_model.norm.weight"), "thinker.model.norm.weight");
        assert_eq!(r("thinker.model.norm.weight"), "thinker.model.norm.weight");
    }

    #[test]
    fn invalid_divisors_are_rejected() {
        let ok = r#"{"thinker_config": {"audio_config": {}, "text_config": {}}}"#;
        assert!(AsrConfig::from_json(ok).is_ok());
        for bad in [
            r#"{"encoder_attention_heads": 0}"#,
            r#"{"encoder_attention_heads": 5}"#,
            r#"{"n_window": 0}"#,
            r#"{"n_window_infer": 50}"#,
            r#"{"num_mel_bins": 0}"#,
        ] {
            let json = format!(r#"{{"thinker_config": {{"audio_config": {bad}, "text_config": {{}}}}}}"#);
            assert!(AsrConfig::from_json(&json).is_err(), "{bad}");
        }
        for bad in [
            r#"{"num_key_value_heads": 0}"#,
            r#"{"num_attention_heads": 12}"#,
            r#"{"num_hidden_layers": 0}"#,
            r#"{"head_dim": 0}"#,
            r#"{"head_dim": 64}"#,
            r#"{"rope_scaling": {"mrope_section": []}}"#,
        ] {
            let json = format!(r#"{{"thinker_config": {{"audio_config": {{}}, "text_config": {bad}}}}}"#);
            assert!(AsrConfig::from_json(&json).is_err(), "{bad}");
        }
        let hf = r#"{"model_type": "qwen3_asr", "audio_config": {"n_window": 0}, "text_config": {}}"#;
        assert!(AsrConfig::from_json(hf).is_err());
    }

    #[test]
    fn test_mrope_interleaved_both_false() {
        let mut cfg = make_base_config();
        cfg.rope_scaling = Some(RopeScaling {
            rope_type: "mrope".to_string(),
            mrope_section: vec![24, 20, 20],
            interleaved: false,
            mrope_interleaved: false,
        });
        assert!(!cfg.mrope_interleaved());
    }
}

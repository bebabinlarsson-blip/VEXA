use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use vexa_types::OperatingProfile;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct VexaConfig {
    #[serde(default)]
    pub profile: OperatingProfile,
    #[serde(default = "default_context_tokens")]
    pub max_context_tokens: usize,
    #[serde(default = "default_warm_timeout")]
    pub warm_timeout_seconds: u64,
    #[serde(default = "default_true")]
    pub auto_update: bool,
    #[serde(default = "default_channel")]
    pub update_channel: String,
    #[serde(default)]
    pub telemetry: bool,
    #[serde(default)]
    pub semantic: bool,
    #[serde(default)]
    pub allow_workspace_execution: bool,
}

impl Default for VexaConfig {
    fn default() -> Self {
        Self {
            profile: OperatingProfile::Auto,
            max_context_tokens: default_context_tokens(),
            warm_timeout_seconds: default_warm_timeout(),
            auto_update: true,
            update_channel: default_channel(),
            telemetry: false,
            semantic: false,
            allow_workspace_execution: false,
        }
    }
}

const fn default_context_tokens() -> usize {
    1800
}

const fn default_warm_timeout() -> u64 {
    600
}

const fn default_true() -> bool {
    true
}

fn default_channel() -> String {
    "stable".to_owned()
}

pub fn config_root() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("VEXA_CONFIG_ROOT") {
        if path.is_empty() {
            bail!("VEXA_CONFIG_ROOT cannot be empty");
        }
        return Ok(PathBuf::from(path));
    }
    dirs::config_dir()
        .context("cannot determine per-user configuration directory")
        .map(|path| path.join("vexa"))
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_root()?.join("config.json"))
}

pub fn load() -> Result<VexaConfig> {
    let path = config_path()?;
    match fs::read(&path) {
        Ok(bytes) => {
            let config: VexaConfig = serde_json::from_slice(&bytes)
                .with_context(|| format!("invalid configuration JSON: {}", path.display()))?;
            validate(&config)?;
            Ok(config)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(VexaConfig::default()),
        Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
    }
}

pub fn save(config: &VexaConfig) -> Result<PathBuf> {
    validate(config)?;
    let path = config_path()?;
    let parent = path.parent().context("configuration path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("cannot create {}", parent.display()))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(temp.as_file_mut(), config)?;
    temp.write_all(b"\n")?;
    temp.as_file().sync_all()?;
    temp.persist(&path).map_err(|error| error.error)?;
    Ok(path)
}

pub fn set_value(config: &mut VexaConfig, key: &str, value: &str) -> Result<()> {
    match key {
        "profile" => {
            config.profile = match value.to_ascii_lowercase().as_str() {
                "auto" => OperatingProfile::Auto,
                "lite" => OperatingProfile::Lite,
                "standard" => OperatingProfile::Standard,
                "enhanced" => OperatingProfile::Enhanced,
                _ => bail!("profile must be auto, lite, standard, or enhanced"),
            };
        }
        "max_context_tokens" => {
            config.max_context_tokens = value
                .parse()
                .context("max_context_tokens must be an integer")?;
        }
        "warm_timeout_seconds" => {
            config.warm_timeout_seconds = value
                .parse()
                .context("warm_timeout_seconds must be an integer")?;
        }
        "auto_update" => config.auto_update = parse_bool(value)?,
        "update_channel" => config.update_channel = value.to_ascii_lowercase(),
        "telemetry" => config.telemetry = parse_bool(value)?,
        "semantic" => config.semantic = parse_bool(value)?,
        "allow_workspace_execution" => config.allow_workspace_execution = parse_bool(value)?,
        _ => bail!("unknown configuration key: {key}"),
    }
    validate(config)
}

fn parse_bool(value: &str) -> Result<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => bail!("expected a boolean value (true/false)"),
    }
}

fn validate(config: &VexaConfig) -> Result<()> {
    if !(128..=1_000_000).contains(&config.max_context_tokens) {
        bail!("max_context_tokens must be between 128 and 1000000");
    }
    if config.warm_timeout_seconds > 86_400 {
        bail!("warm_timeout_seconds must be at most 86400");
    }
    if !matches!(config.update_channel.as_str(), "stable" | "beta" | "dev") {
        bail!("update_channel must be stable, beta, or dev");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_keep_optional_features_off() {
        let config = VexaConfig::default();
        assert!(!config.telemetry);
        assert!(!config.semantic);
        assert!(!config.allow_workspace_execution);
        assert!(config.auto_update);
    }

    #[test]
    fn set_value_validates_profile_and_budget() {
        let mut config = VexaConfig::default();
        set_value(&mut config, "profile", "lite")
            .unwrap_or_else(|error| panic!("profile set failed: {error}"));
        assert_eq!(config.profile, OperatingProfile::Lite);
        assert!(set_value(&mut config, "max_context_tokens", "8").is_err());
    }
}

use anyhow::Result;
use cpal::traits::HostTrait;
use std::path::Path;

pub const SYSTEM_DEFAULT: &str = "System default";

pub fn devices(output: bool) -> Result<Vec<String>> {
    let host = cpal::default_host();
    let devices = if output {
        host.output_devices()?
    } else {
        host.input_devices()?
    };
    let mut names = devices.map(|d| d.to_string()).collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names.insert(0, SYSTEM_DEFAULT.into());
    Ok(names)
}

#[derive(Default)]
pub struct AudioSettings {
    pub model: crate::local_audio::ModelChoice,
    pub input: String,
    pub output: String,
}
impl AudioSettings {
    pub fn load(path: &Path) -> Self {
        let value = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .unwrap_or_default();
        Self {
            model: serde_json::from_value(value["model"].clone()).unwrap_or_default(),
            input: value["input"].as_str().unwrap_or(SYSTEM_DEFAULT).into(),
            output: value["output"].as_str().unwrap_or(SYSTEM_DEFAULT).into(),
        }
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        let temporary = path.with_extension("audio.tmp");
        std::fs::write(
            &temporary,
            serde_json::to_vec_pretty(
                &serde_json::json!({"input": self.input, "output": self.output, "model": self.model}),
            )?,
        )?;
        std::fs::rename(temporary, path)?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn device_preferences_survive_restart_and_missing_files_use_defaults() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("audio.json");
        assert_eq!(AudioSettings::load(&path).input, SYSTEM_DEFAULT);
        AudioSettings {
            model: crate::local_audio::ModelChoice::Parakeet,
            input: "Microphone".into(),
            output: "Headphones".into(),
        }
        .save(&path)?;
        let loaded = AudioSettings::load(&path);
        assert_eq!(loaded.input, "Microphone");
        assert_eq!(loaded.output, "Headphones");
        assert_eq!(loaded.model, crate::local_audio::ModelChoice::Parakeet);
        std::fs::write(&path, r#"{"input":"Old mic","output":"Old output"}"#)?;
        assert_eq!(
            AudioSettings::load(&path).model,
            crate::local_audio::ModelChoice::Nemotron
        );
        Ok(())
    }
}

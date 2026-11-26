//! Receiver settings, stored as JSON next to the log in the user's
//! application data folder.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::vban::DEFAULT_PORT;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Delay {
    Auto,
    Low,
    Balanced,
    Stable,
}

impl Delay {
    pub const ALL: [Delay; 4] = [Delay::Auto, Delay::Low, Delay::Balanced, Delay::Stable];

    pub fn label(self) -> &'static str {
        match self {
            Delay::Auto => "Automatic",
            Delay::Low => "Low",
            Delay::Balanced => "Balanced",
            Delay::Stable => "Stable",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            Delay::Auto => "Follows the quality chosen in the plugin and grows a little if the network is unsteady.",
            Delay::Low => "About 20 ms of buffer. Best on a wired network.",
            Delay::Balanced => "About 40 ms of buffer. Good for most setups.",
            Delay::Stable => "About 80 ms of buffer. Steadier on Wi-Fi.",
        }
    }

    /// Buffer in milliseconds; `hint` is what the sender suggested.
    pub fn target_ms(self, hint: Option<f64>) -> f64 {
        match self {
            Delay::Auto => hint
                .filter(|h| h.is_finite())
                .unwrap_or(40.0)
                .clamp(10.0, 200.0),
            Delay::Low => 20.0,
            Delay::Balanced => 40.0,
            Delay::Stable => 80.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub port: u16,
    /// "auto" for the main network adapter, "all" for every adapter, or an IPv4 address.
    pub listen: String,
    /// None follows the Windows default output.
    pub output_device: Option<String>,
    pub volume: f32,
    pub muted: bool,
    pub delay: Delay,
    pub answer_discovery: bool,
    pub start_minimized: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            listen: "auto".into(),
            output_device: None,
            volume: 1.0,
            muted: false,
            delay: Delay::Auto,
            answer_discovery: true,
            start_minimized: false,
        }
    }
}

impl Settings {
    pub fn sanitized(mut self) -> Self {
        if self.port < 1024 {
            self.port = DEFAULT_PORT;
        }
        if !self.volume.is_finite() {
            self.volume = 1.0;
        }
        self.volume = self.volume.clamp(0.0, 1.0);
        if self.listen != "auto"
            && self.listen != "all"
            && self.listen.parse::<std::net::Ipv4Addr>().is_err()
        {
            self.listen = "auto".into();
        }
        self
    }
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("AudioBridge")
}

fn settings_path() -> PathBuf {
    data_dir().join("receiver.json")
}

pub fn load() -> Settings {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|text| serde_json::from_str::<Settings>(&text).ok())
        .unwrap_or_default()
        .sanitized()
}

pub fn save(settings: &Settings) -> std::io::Result<()> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    let tmp = dir.join("receiver.json.tmp");
    let text = serde_json::to_string_pretty(settings).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, settings_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_use_defaults() {
        let s: Settings = serde_json::from_str(r#"{"volume": 0.5}"#).unwrap();
        assert_eq!(s.port, DEFAULT_PORT);
        assert_eq!(s.volume, 0.5);
        assert_eq!(s.delay, Delay::Auto);
    }

    #[test]
    fn sanitizes_bad_values() {
        let s = Settings {
            port: 80,
            volume: 7.0,
            listen: "nonsense".into(),
            ..Settings::default()
        }
        .sanitized();
        assert_eq!(s.port, DEFAULT_PORT);
        assert_eq!(s.volume, 1.0);
        assert_eq!(s.listen, "auto");
    }

    #[test]
    fn delay_targets() {
        assert_eq!(Delay::Auto.target_ms(Some(20.0)), 20.0);
        assert_eq!(Delay::Auto.target_ms(None), 40.0);
        assert_eq!(Delay::Auto.target_ms(Some(5000.0)), 200.0);
        assert_eq!(Delay::Stable.target_ms(Some(20.0)), 80.0);
    }
}

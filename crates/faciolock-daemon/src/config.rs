use std::path::{Path, PathBuf};

use faciolock_camera::Geometry;
use faciolock_vision::Tier;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub socket: PathBuf,
    pub models: PathBuf,
    pub templates: PathBuf,
    pub ratelimit: PathBuf,
    pub device: Option<PathBuf>,
    pub tier: String,
    pub width: u32,
    pub height: u32,
    pub burst_frames: usize,
    pub capture_deadline_ms: u64,
    pub warmup_frames: usize,
    pub idle_timeout_secs: u64,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            socket: PathBuf::from("/run/faciolock.sock"),
            models: PathBuf::from("/usr/share/faciolock/models"),
            templates: PathBuf::from("/var/lib/faciolock/templates"),
            ratelimit: PathBuf::from("/var/lib/faciolock/ratelimit.db"),
            device: None,
            tier: "ir-strobe".into(),
            width: 640,
            height: 360,
            burst_frames: 6,
            capture_deadline_ms: 1_500,
            warmup_frames: 5,
            idle_timeout_secs: 30,
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn tier(&self) -> Result<Tier, String> {
        match self.tier.as_str() {
            "ir-strobe" => Ok(Tier::IrStrobe),
            "ir-constant" => Ok(Tier::IrConstant),
            "rgb" => Ok(Tier::Rgb),
            other => Err(format!(
                "tier = {other:?} is not one of ir-strobe, ir-constant, rgb"
            )),
        }
    }

    pub fn geometry(&self) -> Geometry {
        Geometry::new(self.width, self.height)
    }

    pub fn capture_deadline(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.capture_deadline_ms)
    }

    pub fn idle_timeout(&self) -> Option<std::time::Duration> {
        match self.idle_timeout_secs {
            0 => None,
            n => Some(std::time::Duration::from_secs(n)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_defaults_and_a_broken_one_is_an_error() {
        let missing = Config::load(Path::new("/nonexistent/faciolockd.toml")).expect("defaults");
        assert_eq!(missing.burst_frames, 6);

        let dir = std::env::temp_dir().join("faciolockd-cfg-test");
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let bad = dir.join("bad.toml");
        std::fs::write(&bad, "burst_frames = \"six\"").expect("write");
        assert!(Config::load(&bad).is_err(), "a malformed config must not start the daemon");

        let typo = dir.join("typo.toml");
        std::fs::write(&typo, "burst_frame = 6").expect("write");
        assert!(Config::load(&typo).is_err(), "an unknown key must be refused");
    }

    #[test]
    fn an_unknown_tier_is_refused_rather_than_defaulted() {
        let mut cfg = Config::default();
        assert_eq!(cfg.tier().expect("the default parses"), Tier::IrStrobe);

        for good in [("ir-constant", Tier::IrConstant), ("rgb", Tier::Rgb)] {
            cfg.tier = good.0.into();
            assert_eq!(cfg.tier().expect(good.0), good.1);
        }
        for bad in ["ir_strobe", "IR-STROBE", "strobe", ""] {
            cfg.tier = bad.into();
            assert!(cfg.tier().is_err(), "{bad:?} must not resolve to a tier");
        }
    }

    #[test]
    fn defaults_agree_with_the_vision_crate() {
        assert_eq!(
            Config::default().tier().expect("parses"),
            faciolock_vision::Config::default().tier
        );
    }
}

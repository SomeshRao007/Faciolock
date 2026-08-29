//! Daemon configuration — paths, the device, and the capture shape (P2-b).
//!
//! **No liveness or matching threshold appears here, and none ever should.** wiki/05's
//! standard is that a threshold is a security parameter that must be *measured*; a
//! threshold reachable from an editable file is a threshold an attacker with one write
//! sets to zero. `faciolock_vision::Config::default()` stays the single source of those
//! numbers, and the [`Enforcement`](faciolock_vision::Enforcement) level travels with them.
//!
//! What is configurable is what genuinely differs between machines: where things live, and
//! which camera to use.

use std::path::{Path, PathBuf};

use faciolock_camera::Geometry;
use faciolock_vision::Tier;
use serde::Deserialize;

/// Everything the daemon reads from disk at startup.
///
/// `deny_unknown_fields` on purpose: a typo'd key silently taking its default is how a
/// machine ends up not running the configuration its operator believes it is running.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// wiki/04: `AF_UNIX` `SOCK_SEQPACKET`, mode 0600. Phase 6 replaces self-binding with
    /// a systemd `.socket` unit and inherits the fd instead.
    pub socket: PathBuf,
    /// Directory holding the ONNX models and their `MANIFEST.sha256`.
    pub models: PathBuf,
    /// One encrypted file per enrolled uid, plus the key that decrypts them. The key lives
    /// in this directory because it is the same trust domain: anything that can read one can
    /// read the other, and pretending otherwise would be theatre (P4-a, wiki/03 R6).
    pub templates: PathBuf,
    /// The persistent failure budget (wiki/03 R5). A **path**, not a policy — the backoff
    /// and lockout numbers are hardcoded in `ratelimit.rs` for the reason this module's
    /// header gives, and P4-c makes that explicit.
    pub ratelimit: PathBuf,
    /// The **`by-path`** symlink for the IR camera, not `/dev/videoN` — the numbering
    /// renumbers across boots and `by-id` collides on the reference machine
    /// ([wiki/10 Probe 4]). `None` runs discovery on every open.
    ///
    /// [wiki/10 Probe 4]: ../../../wiki/10-hardware-probe.md
    pub device: Option<PathBuf>,
    /// Which illumination this machine's camera provides: `ir-strobe`, `ir-constant` or
    /// `rgb`. Observed once by `faciolockctl diagnose` and written here.
    ///
    /// **This is the one security-relevant value in this file, and it is here because P1-e
    /// says it must be.** The tier decides which liveness checks apply, and inferring it from
    /// the burst being authenticated would let an attacker with a bright enough continuous IR
    /// source wash out the ON/OFF separation and *demote* the machine to a tier whose checks
    /// they can pass. Configured means an operator chose it; a burst that does not conform is
    /// a reject, not a downgrade.
    ///
    /// A `String` rather than [`faciolock_vision::Tier`] because that crate has no `serde`
    /// and must not gain one — its dependency count is a security claim (wiki/04). Parsed by
    /// [`Config::tier`], which refuses an unknown value rather than defaulting.
    pub tier: String,
    pub width: u32,
    pub height: u32,
    /// Frames per authentication burst. Six gives three ON/OFF pairs against a `min_pairs`
    /// of 2, and at the **measured** 12–15 fps it spans ~340–410 ms ([B19]).
    ///
    /// [B19]: ../../../wiki/11-bug-log.md
    pub burst_frames: usize,
    /// Hard cap on how long a burst may take. The client's `timeout_ms` is a hint; this is
    /// the contract (wiki/04).
    pub capture_deadline_ms: u64,
    /// Frames discarded when a stream is opened, while the AGC converges. Paid at startup
    /// so authentication does not pay it (wiki/10 Probe 3).
    pub warmup_frames: usize,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            socket: PathBuf::from("/run/faciolock.sock"),
            models: PathBuf::from("/usr/share/faciolock/models"),
            templates: PathBuf::from("/var/lib/faciolock/templates"),
            ratelimit: PathBuf::from("/var/lib/faciolock/ratelimit.db"),
            device: None,
            // Matches `faciolock_vision::Config::default().tier`. The two defaults are
            // spelled separately because the crates do not share a config format, and
            // `defaults_agree_with_the_vision_crate` below is what keeps them equal.
            tier: "ir-strobe".into(),
            width: 640,
            height: 360,
            burst_frames: 6,
            capture_deadline_ms: 1_500,
            warmup_frames: 5,
        }
    }
}

impl Config {
    /// Read the file, or fall back to the defaults if it is not there.
    ///
    /// A **missing** file is a fresh install and defaults are the right answer. A file that
    /// exists and does not parse is an operator error, and starting anyway on defaults
    /// would run a configuration nobody wrote.
    pub fn load(path: &Path) -> Result<Config, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Parse [`tier`](Config::tier), refusing anything unrecognised.
    ///
    /// A typo must not silently take a default here of all places: `rgb` and `ir-strobe`
    /// enforce different checks, so a machine running the wrong one is a machine whose
    /// liveness nobody chose. `deny_unknown_fields` gives that guarantee for the *key*; this
    /// gives it for the value.
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

        // A typo must not silently take a default -- that is what `deny_unknown_fields` buys.
        let typo = dir.join("typo.toml");
        std::fs::write(&typo, "burst_frame = 6").expect("write");
        assert!(Config::load(&typo).is_err(), "an unknown key must be refused");
    }

    /// The tier decides which liveness checks run, so an unreadable value must stop the
    /// daemon rather than quietly select one. `deny_unknown_fields` covers the key; this is
    /// the value.
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

    /// Two crates spell this default separately because they share no config format. They
    /// must still agree, or a machine with no `tier =` line runs different checks than the
    /// vision crate's own tests do.
    #[test]
    fn defaults_agree_with_the_vision_crate() {
        assert_eq!(
            Config::default().tier().expect("parses"),
            faciolock_vision::Config::default().tier
        );
    }
}

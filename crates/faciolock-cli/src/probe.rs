//! `diagnose` — what this machine has, and what to write in the config for it.
//!
//! Read-only throughout. It opens a device and takes frames; it sets no control, writes no
//! register and touches no emitter. wiki/08 Q3 settled that there is no emitter code in this
//! project at all, and a diagnostic is the one place someone would be tempted to add some.
//!
//! ## Why this refuses to guess a tier downward
//!
//! P1-e: the capability tier is enrolment-time configuration precisely so that an attacker
//! with a bright continuous IR source cannot *demote* the machine to a tier whose checks they
//! can pass. A `diagnose` that observed no alternation and helpfully wrote `tier =
//! "ir-constant"` would hand that attack back — and it would not even need an attacker,
//! because [B18] showed daylight alone raises the floor far enough to flatten the ratio.
//!
//! So the rule here is one-directional: **a clean strobe is reported as `ir-strobe`;
//! anything else is reported as "not observed" and no tier is written.** A machine that
//! genuinely has a constant emitter needs a human to say so.
//!
//! [B18]: ../../../wiki/11-bug-log.md

use std::path::{Path, PathBuf};
use std::time::Duration;

use faciolock_camera::{Capture, Geometry, enumerate};

/// Enough to see the AGC settle and still leave ~25 frames of pattern. wiki/10 Probe 3 took
/// 60 and the first four were convergence; 30 at the measured 12-15 fps is ~2 s of hardware
/// exposure, which is the right cost for a command someone runs once.
const PROBE_FRAMES: usize = 30;

/// Generous: this is not on any latency path, and a stall here would report "no camera" for
/// a camera that was merely slow.
const PROBE_DEADLINE: Duration = Duration::from_secs(5);

/// Frames discarded before the pattern is measured. Same reason as the daemon's
/// `warmup_frames`: the reference machine reads 22.7, 23.4, 14.3, 31.4 before the AGC settles
/// into a clean alternation, and measuring those as signal would report a broken strobe.
const PROBE_WARMUP: usize = 5;

/// What 30 frames said about the illumination.
pub struct Strobe {
    /// Per-frame mean luma, in order, after the warm-up discard. Printed so a reader can see
    /// the pattern rather than take the verdict on trust — wiki/10 Probe 3 is exactly this
    /// list and it is what settled the question in the first place.
    pub means: Vec<f64>,
    /// Fraction of adjacent frames that crossed the median. 1.0 is perfect alternation.
    pub alternation: f64,
    /// mean(bright half) / mean(dark half). The reference machine measured 14,000:1 and
    /// 100,000:1 on two different days (wiki/10 Probe 3, 3b).
    pub ratio: f64,
}

impl Strobe {
    /// The only tier this function will assert. See the module header.
    ///
    /// Both numbers are generous by orders of magnitude against the reference machine, on
    /// purpose: this is a *diagnostic* gate, and its job is to be unambiguous rather than
    /// tight. A marginal reading is a "no" here, and a "no" costs a re-run after dark.
    pub fn is_strobing(&self) -> bool {
        // One dropped frame in a 25-frame window breaks two adjacencies, so 0.9 rather than
        // 1.0 — and a dropped frame is a thing `CapturedBurst::dropped` counts for real.
        self.alternation >= 0.9 && self.ratio >= 2.0
    }
}

/// Mean luma per frame, then the pattern those means make.
///
/// Split from the capture so it can be tested against numbers this project already has —
/// wiki/10's two recorded runs — rather than only against whatever the camera does today.
pub fn analyse(means: Vec<f64>) -> Strobe {
    if means.len() < 4 {
        return Strobe { means, alternation: 0.0, ratio: 0.0 };
    }

    let mut sorted = means.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    // **Averaged across the two middle values on an even count**, which is not pedantry here.
    // A clean strobe splits exactly in half, so `sorted[n/2]` is the *dimmest ON frame* — and
    // testing `m > median` against it drops that frame to the OFF side, costing one adjacency
    // and reading a perfect alternation as 0.96. The midpoint lands in the gap between the two
    // clusters, where a divider belongs.
    let mid = sorted.len() / 2;
    let median = match sorted.len() % 2 {
        0 => (sorted[mid - 1] + sorted[mid]) / 2.0,
        _ => sorted[mid],
    };

    // Crossings of the median, not of each other: an absolute-brightness comparison would be
    // fooled by wiki/10's measured 13% AGC droop across a single capture, where consecutive
    // ON frames descend monotonically.
    let above: Vec<bool> = means.iter().map(|m| *m > median).collect();
    let flips = above.windows(2).filter(|w| w[0] != w[1]).count();
    let alternation = flips as f64 / (above.len() - 1) as f64;

    let mean_of = |want: bool| {
        let picked: Vec<f64> =
            means.iter().zip(&above).filter(|(_, a)| **a == want).map(|(m, _)| *m).collect();
        match picked.is_empty() {
            true => 0.0,
            false => picked.iter().sum::<f64>() / picked.len() as f64,
        }
    };
    let (bright, dark) = (mean_of(true), mean_of(false));
    // The dark half is genuine sensor read noise, never a true zero (wiki/10 Probe 3, point
    // 3), so this divides by something real. `f64::MAX` rather than an infinity if it ever is
    // zero, because a printed `inf` reads like a bug in the tool.
    let ratio = if dark > 0.0 { bright / dark } else { f64::MAX };

    Strobe { means, alternation, ratio }
}

/// Open the device, take [`PROBE_FRAMES`], measure.
pub fn observe(node: &Path, geometry: Geometry) -> Result<Strobe, String> {
    let device = enumerate::open(node, geometry).map_err(|e| format!("{}: {e}", node.display()))?;
    let mut capture = Capture::start(device, geometry, PROBE_WARMUP)
        .map_err(|e| format!("{}: {e}", node.display()))?;
    let burst = capture
        .burst(PROBE_FRAMES, PROBE_DEADLINE)
        .map_err(|e| format!("{}: {e}", node.display()))?;

    let stride = burst.width * burst.height;
    let means = burst
        .planar
        .chunks_exact(stride)
        .map(|frame| frame.iter().map(|b| *b as f64).sum::<f64>() / stride as f64)
        .collect();
    Ok(analyse(means))
}

/// The config file `diagnose --write` produces.
///
/// Hand-written rather than serialised: `Config` derives `Deserialize` only, and the comments
/// are the reason to write a config file at all — a bare `tier = "ir-strobe"` with nothing
/// saying what it means is how an operator ends up editing it wrongly.
pub fn config_text(device: &Path, tier: &str) -> String {
    format!(
        "# Written by `faciolockctl diagnose --write`.\n\
         #\n\
         # The device is a by-path symlink, not /dev/videoN: the numbering renumbers across\n\
         # boots, and by-id collides between the RGB and IR interfaces of the same USB device\n\
         # on at least one machine (wiki/10 Probe 4). The daemon re-verifies the format and\n\
         # geometry on open regardless of what this says.\n\
         device = \"{}\"\n\
         \n\
         # Which illumination this camera provides, and therefore which liveness checks apply.\n\
         # Observed once, here. Never inferred per authentication: a burst that does not\n\
         # conform to this tier is a reject, not a downgrade (P1-e).\n\
         tier = \"{tier}\"\n",
        device.display()
    )
}

/// Write the config, and never over an existing one.
///
/// `create_new` is the whole safety property: a fresh install gets a working file, and a
/// machine somebody has already configured is never silently rewritten by a diagnostic.
pub fn write_config(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut f) => {
            use std::io::Write;
            f.write_all(text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(format!("{} already exists and will not be overwritten", path.display()))
        }
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Every video node and what looking at it said. Re-exported so `main` need not know the
/// camera crate's shape.
pub fn survey() -> Vec<enumerate::Node> {
    enumerate::survey()
}

pub fn discover() -> Result<enumerate::Selected, enumerate::DiscoveryError> {
    enumerate::discover()
}

/// Where the config wants the device recorded: the stable name if there is one, the raw node
/// if this machine has no `by-path` entry — which is worth showing rather than papering over.
pub fn stable_name(selected: &enumerate::Selected) -> PathBuf {
    selected.by_path.clone().unwrap_or_else(|| selected.node.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **wiki/10 Probe 3's own numbers**, which is the point: the classifier is checked
    /// against the recording that settled the hardware question rather than against a shape
    /// invented to make it pass.
    #[test]
    fn the_reference_machines_recorded_run_reads_as_a_strobe() {
        let means = vec![
            0.00126302, 59.9136, 0.00366753, 61.9249, 0.00404514, 61.0619, 0.00392795, 60.3695,
            0.0040842, 59.8574, 0.00400608, 59.3313, 0.0040625, 58.8541, 0.00408854, 58.4122,
            0.00403212, 58.0754, 0.00400174, 57.7173, 0.00414931, 57.4615, 0.00385851, 56.9237,
        ];
        let s = analyse(means);
        assert_eq!(s.alternation, 1.0, "27 clean pairs, no missed beats");
        assert!(s.ratio > 10_000.0, "wiki/10 measured ~14,000:1, got {}", s.ratio);
        assert!(s.is_strobing());
    }

    /// The AGC droop is real — wiki/10 measured ON frames falling 61.9 → 53.7 across one
    /// capture, 13%. Comparing each frame to its neighbour rather than to the median would
    /// read that descent as signal; this asserts it does not.
    #[test]
    fn a_thirteen_percent_agc_droop_does_not_break_the_pattern() {
        let means: Vec<f64> =
            (0..24).map(|i| if i % 2 == 0 { 0.004 } else { 61.9 - i as f64 * 0.35 }).collect();
        let s = analyse(means);
        assert_eq!(s.alternation, 1.0);
        assert!(s.is_strobing());
    }

    /// **The B18 case, and the one that must not auto-demote.** Ambient infrared lifts the
    /// dark frames until the ratio collapses; a tool that answered `ir-constant` here would
    /// write a weaker tier because somebody opened the blinds — or because somebody aimed a
    /// floodlight, which is the same measurement.
    #[test]
    fn a_washed_out_capture_is_not_a_strobe_and_is_not_a_demotion() {
        let means: Vec<f64> =
            (0..24).map(|i| if i % 2 == 0 { 84.0 } else { 86.0 }).collect();
        let s = analyse(means);
        assert_eq!(s.alternation, 1.0, "it still alternates, faintly");
        assert!(s.ratio < 2.0, "but the ratio is gone: {}", s.ratio);
        assert!(!s.is_strobing(), "and that must not read as a strobe");
    }

    /// A constant emitter: no pattern at all. Also not a strobe, and also not written.
    #[test]
    fn a_constant_emitter_reads_as_no_strobe() {
        let s = analyse(vec![54.0; 24]);
        assert!(!s.is_strobing());

        // A camera that produced nothing usable must not divide its way to a verdict.
        let empty = analyse(vec![]);
        assert!(!empty.is_strobing());
        assert_eq!(empty.alternation, 0.0);
    }

    /// The config file is the deliverable of `--write`, and an existing one is never
    /// overwritten — a diagnostic that clobbered a working machine's configuration would be
    /// worse than one that did nothing.
    #[test]
    fn writing_a_config_refuses_to_overwrite() {
        let dir = std::env::temp_dir().join(format!("faciolockctl-cfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("faciolockd.toml");

        let text = config_text(Path::new("/dev/v4l/by-path/pci-0000:00:14.0-usb-0:7:1.2-video-index0"), "ir-strobe");
        assert!(text.contains("tier = \"ir-strobe\""), "{text}");
        assert!(text.contains("device = \"/dev/v4l/by-path/"), "{text}");

        write_config(&path, &text).expect("writes into a fresh directory");
        assert_eq!(std::fs::read_to_string(&path).expect("reads"), text);

        let second = write_config(&path, "device = \"/dev/video9\"\n");
        assert!(second.is_err(), "an existing config must survive a second diagnose");
        assert_eq!(std::fs::read_to_string(&path).expect("reads"), text, "unchanged");

        let _ = std::fs::remove_dir_all(&dir);
    }
}

use std::path::{Path, PathBuf};
use std::time::Duration;

use faciolock_camera::{Capture, Geometry, enumerate};

const PROBE_FRAMES: usize = 30;

const PROBE_DEADLINE: Duration = Duration::from_secs(5);

const PROBE_WARMUP: usize = 5;

pub struct Strobe {
    pub means: Vec<f64>,
    pub alternation: f64,
    pub ratio: f64,
}

impl Strobe {
    pub fn is_strobing(&self) -> bool {
        self.alternation >= 0.9 && self.ratio >= 2.0
    }
}

pub fn analyse(means: Vec<f64>) -> Strobe {
    if means.len() < 4 {
        return Strobe { means, alternation: 0.0, ratio: 0.0 };
    }

    let mut sorted = means.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mid = sorted.len() / 2;
    let median = match sorted.len() % 2 {
        0 => (sorted[mid - 1] + sorted[mid]) / 2.0,
        _ => sorted[mid],
    };

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
    let ratio = if dark > 0.0 { bright / dark } else { f64::MAX };

    Strobe { means, alternation, ratio }
}

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

pub fn survey() -> Vec<enumerate::Node> {
    enumerate::survey()
}

pub fn discover() -> Result<enumerate::Selected, enumerate::DiscoveryError> {
    enumerate::discover()
}

pub fn stable_name(selected: &enumerate::Selected) -> PathBuf {
    selected.by_path.clone().unwrap_or_else(|| selected.node.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn a_thirteen_percent_agc_droop_does_not_break_the_pattern() {
        let means: Vec<f64> =
            (0..24).map(|i| if i % 2 == 0 { 0.004 } else { 61.9 - i as f64 * 0.35 }).collect();
        let s = analyse(means);
        assert_eq!(s.alternation, 1.0);
        assert!(s.is_strobing());
    }

    #[test]
    fn a_washed_out_capture_is_not_a_strobe_and_is_not_a_demotion() {
        let means: Vec<f64> =
            (0..24).map(|i| if i % 2 == 0 { 84.0 } else { 86.0 }).collect();
        let s = analyse(means);
        assert_eq!(s.alternation, 1.0, "it still alternates, faintly");
        assert!(s.ratio < 2.0, "but the ratio is gone: {}", s.ratio);
        assert!(!s.is_strobing(), "and that must not read as a strobe");
    }

    #[test]
    fn a_constant_emitter_reads_as_no_strobe() {
        let s = analyse(vec![54.0; 24]);
        assert!(!s.is_strobing());

        let empty = analyse(vec![]);
        assert!(!empty.is_strobing());
        assert_eq!(empty.alternation, 0.0);
    }

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

use std::path::{Path, PathBuf};
use std::time::Duration;

use faciolock_camera::{Capture, Geometry, enumerate};

const PROBE_FRAMES: usize = 30;

const PROBE_DEADLINE: Duration = Duration::from_secs(5);

const PROBE_WARMUP: usize = 5;

pub struct Strobe {
    pub means: Vec<f64>,
    pub alternation: f64,
    pub amplitude: f64,
    pub snr: f64,
}

impl Strobe {
    pub fn is_strobing(&self) -> bool {
        self.alternation >= 0.9 && self.snr >= 10.0
    }
}

pub fn analyse(means: Vec<f64>) -> Strobe {
    if means.len() < 3 {
        return Strobe { means, alternation: 0.0, amplitude: 0.0, snr: 0.0 };
    }

    // Match vision::frame::classify without linking the inference runtime into the CLI.
    // Differences preserve the emitter signal when daylight lifts both frame phases.
    let steps: Vec<f64> = means.windows(2).map(|w| w[1] - w[0]).collect();
    let amplitude = median_abs(&steps);
    let same_phase = median_abs(&means.windows(3).map(|w| w[2] - w[0]).collect::<Vec<_>>());
    let snr = match (amplitude > 0.0, same_phase > 0.0) {
        (true, false) => f64::INFINITY,
        (true, true) => amplitude / same_phase,
        (false, _) => 0.0,
    };
    let flips = steps.windows(2).filter(|w| w[0] * w[1] < 0.0).count();
    let alternation = flips as f64 / (steps.len() - 1) as f64;

    Strobe { means, alternation, amplitude, snr }
}

fn median_abs(values: &[f64]) -> f64 {
    let mut sorted: Vec<f64> = values.iter().map(|v| v.abs()).collect();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
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
    fn the_reference_machines_recorded_run_reads_as_a_strobe() -> Result<(), String> {
        let means = vec![
            0.00126302, 59.9136, 0.00366753, 61.9249, 0.00404514, 61.0619, 0.00392795, 60.3695,
            0.0040842, 59.8574, 0.00400608, 59.3313, 0.0040625, 58.8541, 0.00408854, 58.4122,
            0.00403212, 58.0754, 0.00400174, 57.7173, 0.00414931, 57.4615, 0.00385851, 56.9237,
        ];
        let s = analyse(means);
        if !s.is_strobing() || s.alternation != 1.0 || s.amplitude < 50.0 {
            return Err(format!("lost reference strobe: amplitude {}, SNR {}", s.amplitude, s.snr));
        }
        Ok(())
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
    fn daylight_and_agc_drift_preserve_a_clear_strobe() -> Result<(), String> {
        for (ambient, response, drift) in [(84.0, 2.0, 0.0), (84.0, 10.0, 0.4)] {
            let means = (0..30)
                .map(|i| ambient + i as f64 * drift + if i % 2 == 0 { 0.0 } else { response })
                .collect();
            let s = analyse(means);
            if !s.is_strobing() || s.alternation != 1.0 {
                return Err(format!("lost daylight strobe: amplitude {}, SNR {}", s.amplitude, s.snr));
            }
        }
        Ok(())
    }

    #[test]
    fn noise_and_irregular_pulses_are_not_a_strobe() -> Result<(), String> {
        for means in [
            vec![84.0, 86.0, 83.0, 89.0, 84.0, 91.0],
            vec![84.0, 94.0, 94.0, 84.0, 84.0, 94.0],
            vec![84.0, 85.0, 86.0, 87.0, 88.0, 89.0],
            vec![84.0, 94.0],
        ] {
            if analyse(means).is_strobing() {
                return Err("classified noise or an incomplete pattern as a strobe".into());
            }
        }
        Ok(())
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

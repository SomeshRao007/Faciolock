use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use v4l::capability::Flags as CapFlags;
use v4l::video::Capture as _;
use v4l::{Device, FourCC};

use crate::capture::{CaptureError, Geometry};

pub const GREY: FourCC = FourCC { repr: *b"GREY" };

const BY_PATH: &str = "/dev/v4l/by-path";

#[derive(Debug)]
pub enum Node {
    Capture(Candidate),
    NotCapture { path: PathBuf },
    Inaccessible { path: PathBuf, reason: io::Error },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    pub card: String,
    pub greyscale: bool,
    pub max_pixels: u64,
}

#[derive(Clone, Debug)]
pub struct Selected {
    pub node: PathBuf,
    pub by_path: Option<PathBuf>,
    pub card: String,
}

#[derive(Debug)]
pub enum DiscoveryError {
    NoNodes,
    NoneAccessible { tried: Vec<PathBuf> },
    NoInfrared { saw: Vec<Candidate>, inaccessible: Vec<PathBuf> },
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiscoveryError::NoNodes => write!(f, "no /dev/video* nodes exist"),
            DiscoveryError::NoneAccessible { tried } => {
                write!(f, "{} video node(s) exist and none could be opened", tried.len())
            }
            DiscoveryError::NoInfrared { saw, inaccessible } => {
                write!(f, "{} capture device(s), none offering 8-bit greyscale", saw.len())?;
                if !inaccessible.is_empty() {
                    write!(f, " ({} more could not be opened)", inaccessible.len())?;
                }
                Ok(())
            }
        }
    }
}

pub fn survey() -> Vec<Node> {
    let mut nodes = video_nodes();
    nodes.sort_by_key(|p| node_index(p));
    nodes.iter().map(|p| inspect(p)).collect()
}

pub fn discover() -> Result<Selected, DiscoveryError> {
    let nodes = survey();
    if nodes.is_empty() {
        return Err(DiscoveryError::NoNodes);
    }

    let mut candidates = Vec::new();
    let mut inaccessible = Vec::new();
    for node in nodes {
        match node {
            Node::Capture(c) => candidates.push(c),
            Node::Inaccessible { path, .. } => inaccessible.push(path),
            Node::NotCapture { .. } => {}
        }
    }

    if candidates.is_empty() && !inaccessible.is_empty() {
        return Err(DiscoveryError::NoneAccessible { tried: inaccessible });
    }

    let Some(i) = select(&candidates) else {
        return Err(DiscoveryError::NoInfrared { saw: candidates, inaccessible });
    };
    let chosen = &candidates[i];
    Ok(Selected {
        node: chosen.path.clone(),
        by_path: by_path_for(&chosen.path),
        card: chosen.card.clone(),
    })
}

pub fn select(candidates: &[Candidate]) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.greyscale)
        .min_by_key(|(i, c)| (!looks_infrared(&c.card), c.max_pixels, *i))
        .map(|(i, _)| i)
}

fn looks_infrared(card: &str) -> bool {
    card.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|t| t.eq_ignore_ascii_case("ir") || t.eq_ignore_ascii_case("nir") || t.eq_ignore_ascii_case("infrared"))
}

pub fn open(path: &Path, expect: Geometry) -> Result<Device, CaptureError> {
    let dev = Device::with_path(path).map_err(CaptureError::from_io)?;

    let caps = dev.query_caps().map_err(CaptureError::from_io)?;
    if !caps.capabilities.contains(CapFlags::VIDEO_CAPTURE) {
        return Err(CaptureError::NotCapture);
    }

    let mut want = dev.format().map_err(CaptureError::from_io)?;
    want.width = expect.width;
    want.height = expect.height;
    want.fourcc = GREY;
    let got = dev.set_format(&want).map_err(CaptureError::from_io)?;

    if got.fourcc != GREY {
        return Err(CaptureError::WrongFormat { got: fourcc_name(got.fourcc) });
    }
    if got.width != expect.width || got.height != expect.height {
        return Err(CaptureError::WrongGeometry {
            want: expect,
            got: Geometry { width: got.width, height: got.height },
        });
    }
    Ok(dev)
}

pub fn by_path_for(node: &Path) -> Option<PathBuf> {
    let target = fs::canonicalize(node).ok()?;
    let mut links: Vec<PathBuf> = fs::read_dir(BY_PATH)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|l| fs::canonicalize(l).is_ok_and(|r| r == target))
        .collect();
    links.sort();
    links.into_iter().next()
}

fn inspect(path: &Path) -> Node {
    let dev = match Device::with_path(path) {
        Ok(d) => d,
        Err(reason) => return Node::Inaccessible { path: path.to_path_buf(), reason },
    };
    let caps = match dev.query_caps() {
        Ok(c) => c,
        Err(reason) => return Node::Inaccessible { path: path.to_path_buf(), reason },
    };

    if !caps.capabilities.contains(CapFlags::VIDEO_CAPTURE) {
        return Node::NotCapture { path: path.to_path_buf() };
    }

    let greyscale = dev.enum_formats().is_ok_and(|f| f.iter().any(|d| d.fourcc == GREY));
    let max_pixels = if greyscale {
        dev.enum_framesizes(GREY)
            .map(|sizes| {
                sizes
                    .into_iter()
                    .flat_map(|s| s.size.to_discrete())
                    .map(|d| u64::from(d.width) * u64::from(d.height))
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0)
    } else {
        0
    };

    Node::Capture(Candidate { path: path.to_path_buf(), card: caps.card, greyscale, max_pixels })
}

fn video_nodes() -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir("/dev") else { return Vec::new() };
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| node_index(p).is_some())
        .collect()
}

fn node_index(path: &Path) -> Option<u32> {
    path.file_name()?.to_str()?.strip_prefix("video")?.parse().ok()
}

fn fourcc_name(f: FourCC) -> String {
    f.str().unwrap_or("????").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(path: &str, card: &str, greyscale: bool, max_pixels: u64) -> Candidate {
        Candidate { path: PathBuf::from(path), card: card.into(), greyscale, max_pixels }
    }

    #[test]
    fn picks_the_ir_camera_on_the_reference_machine() {
        let saw = [
            candidate("/dev/video0", "ASUS FHD webcam: ASUS FHD webca", false, 0),
            candidate("/dev/video2", "ASUS FHD webcam: ASUS IR camera", true, 640 * 360),
        ];
        assert_eq!(select(&saw), Some(1));
    }

    #[test]
    fn a_camera_called_ir_that_cannot_do_grey_is_not_selected() {
        let saw = [candidate("/dev/video0", "Vendor IR camera", false, 640 * 360)];
        assert_eq!(select(&saw), None);
    }

    #[test]
    fn the_name_outranks_the_sensor_size() {
        let saw = [
            candidate("/dev/video0", "Generic mono machine vision", true, 320 * 240),
            candidate("/dev/video4", "Acme NIR sensor", true, 1280 * 720),
        ];
        assert_eq!(select(&saw), Some(1));
    }

    #[test]
    fn the_smaller_sensor_breaks_a_nameless_tie() {
        let saw = [
            candidate("/dev/video0", "Anonymous camera", true, 1280 * 720),
            candidate("/dev/video4", "Anonymous camera", true, 640 * 360),
        ];
        assert_eq!(select(&saw), Some(1));
    }

    #[test]
    fn a_total_tie_resolves_to_the_earlier_node() {
        let saw = [
            candidate("/dev/video0", "Twin", true, 640 * 360),
            candidate("/dev/video4", "Twin", true, 640 * 360),
        ];
        assert_eq!(select(&saw), Some(0));
    }

    #[test]
    fn ir_is_matched_as_a_word_not_a_substring() {
        for yes in ["ASUS IR camera", "Acme NIR sensor", "Vendor Infrared Cam", "cam(ir)"] {
            assert!(looks_infrared(yes), "{yes} should read as infrared");
        }
        for no in ["Wired webcam", "Direct HD", "Third camera", "Chirality Cam"] {
            assert!(!looks_infrared(no), "{no} should not read as infrared");
        }
    }

    #[test]
    fn video_nodes_are_recognised_by_a_numeric_suffix() {
        assert_eq!(node_index(Path::new("/dev/video2")), Some(2));
        assert_eq!(node_index(Path::new("/dev/video12")), Some(12));
        assert_eq!(node_index(Path::new("/dev/videoX")), None);
        assert_eq!(node_index(Path::new("/dev/video")), None);
        assert_eq!(node_index(Path::new("/dev/vcsa2")), None);
    }

    #[test]
    fn the_three_failures_stay_distinguishable() {
        let absent = DiscoveryError::NoNodes.to_string();
        let denied =
            DiscoveryError::NoneAccessible { tried: vec![PathBuf::from("/dev/video0")] }.to_string();
        let not_ir = DiscoveryError::NoInfrared {
            saw: vec![candidate("/dev/video0", "RGB", false, 0)],
            inaccessible: Vec::new(),
        }
        .to_string();
        assert_ne!(absent, denied);
        assert_ne!(denied, not_ir);
        assert_ne!(absent, not_ir);
    }
}

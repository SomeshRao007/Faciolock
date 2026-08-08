//! Finding the IR camera — and saying which of the three ways it wasn't found.
//!
//! wiki/10's first probe printed nothing and read as "no evidence", because `2>/dev/null`
//! hid a missing binary exactly as well as it would have hidden a permission error. The
//! lesson it produced is a requirement on this module: **`absent`, `inaccessible` and
//! `present-but-not-infrared` are three different answers and must never collapse into one
//! empty result.** [`DiscoveryError`] has a variant for each.
//!
//! The heuristics are wiki/05's, in its order, with one filter in front of them.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use v4l::capability::Flags as CapFlags;
use v4l::video::Capture as _;
use v4l::{Device, FourCC};

use crate::capture::{CaptureError, Geometry};

/// 8-bit greyscale, which `ffplay` reports as `Y800`. The only format the reference
/// machine's IR camera offers, and the only one this pipeline can consume.
pub const GREY: FourCC = FourCC { repr: *b"GREY" };

const BY_PATH: &str = "/dev/v4l/by-path";

/// A `/dev/videoN` node, and what looking at it told us.
#[derive(Debug)]
pub enum Node {
    /// A real capture device.
    Capture(Candidate),
    /// Opens fine, but is not a video-capture device. On the reference machine two of the
    /// four nodes are UVC payload-header metadata nodes that expose no formats and can
    /// never yield a frame (wiki/10 Probe 4).
    NotCapture { path: PathBuf },
    /// Exists and could not be opened. Almost always the `video` group or a missing ACL —
    /// which is a fixable configuration problem, and so must not be reported as "no
    /// camera".
    Inaccessible { path: PathBuf, reason: io::Error },
}

/// A capture device, reduced to the facts the heuristics run on.
///
/// Deliberately plain data with no handle in it, so [`select`] is a pure function that can
/// be tested against hardware this project will never own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    /// `Card type` from `VIDIOC_QUERYCAP`, e.g. `ASUS FHD webcam: ASUS IR camera`.
    pub card: String,
    /// Whether the device offers [`GREY`] at all.
    pub greyscale: bool,
    /// Largest advertised GREY frame size, as `width * height`. Zero when not greyscale.
    pub max_pixels: u64,
}

/// The device we settled on, and the name to persist for it.
#[derive(Clone, Debug)]
pub struct Selected {
    /// The node as it is right now. **Not** what goes in config — it renumbers across
    /// boots.
    pub node: PathBuf,
    /// The stable name, and the one config stores. `None` means this machine has no
    /// `by-path` entry for the node, which is worth reporting rather than papering over.
    pub by_path: Option<PathBuf>,
    pub card: String,
}

/// Why no IR camera was selected.
#[derive(Debug)]
pub enum DiscoveryError {
    /// No `/dev/video*` nodes exist. No camera hardware, or no `uvcvideo`.
    NoNodes,
    /// Nodes exist and not one of them could be opened.
    NoneAccessible { tried: Vec<PathBuf> },
    /// Capture devices exist; none of them looks like an IR camera. The candidates are
    /// carried so `diagnose` can print what it did see instead of an empty list.
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

/// Look at every `/dev/video*` node without choosing between them.
///
/// Split out from [`discover`] so `diagnose` can show the whole picture, including the
/// nodes that were skipped and why.
pub fn survey() -> Vec<Node> {
    let mut nodes = video_nodes();
    nodes.sort_by_key(|p| node_index(p));
    nodes.iter().map(|p| inspect(p)).collect()
}

/// Find the IR camera.
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

/// Pick the IR camera out of a set of capture devices. Pure, so it is testable against
/// hardware layouts nobody here owns.
///
/// wiki/05's heuristics in wiki/05's order:
///
/// 1. **Offers [`GREY`]** — RGB webcams rarely do, and this is decisive on the reference
///    machine where it is the IR camera's *only* format. It is a filter rather than a
///    preference: a device that cannot produce 8-bit greyscale cannot feed this pipeline
///    at all, whatever it is called.
/// 2. **Card name** says `IR` / `NIR` / `Infrared`.
/// 3. **Smaller sensor** — IR sensors run 340×340, 400×400, 640×360, not 1280×720.
///
/// wiki/05's fourth heuristic, capturing a frame and testing chroma variance, is
/// deliberately not implemented: it is unreachable on the only hardware this project has
/// (step 1 decides alone), it would mean streaming from unknown devices during discovery,
/// and there is no second machine to validate it against. A loud failure is better than an
/// untested fallback. It goes in when a machine that needs it exists.
pub fn select(candidates: &[Candidate]) -> Option<usize> {
    // (name matches, smaller sensor, earlier node) — the last only so that identical
    // hardware picks the same device twice in a row.
    candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.greyscale)
        .min_by_key(|(i, c)| (!looks_infrared(&c.card), c.max_pixels, *i))
        .map(|(i, _)| i)
}

/// Whether a card name claims to be an infrared sensor.
///
/// Tokenised rather than substring-matched: `contains("IR")` also fires on `Wired`,
/// `Direct` and `Third`, and picking the wrong camera is the failure this whole module
/// exists to prevent (wiki/10 Probe 4 — a stable name that silently resolves to the RGB
/// sensor is worse than an unstable one that fails loudly).
fn looks_infrared(card: &str) -> bool {
    card.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|t| t.eq_ignore_ascii_case("ir") || t.eq_ignore_ascii_case("nir") || t.eq_ignore_ascii_case("infrared"))
}

/// Open a device and re-verify it is what config claims, before trusting a single frame.
///
/// The daemon re-verifies on every open regardless of what the stored path says. Hardware
/// changes, kernels renumber, and the cost is one `ioctl` against the failure mode of
/// authenticating somebody off the RGB sensor while every liveness check in wiki/05
/// assumes IR.
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

    // V4L2 negotiates rather than refuses: ask for something the device cannot do and it
    // returns the nearest thing it can, successfully. So the answer has to be checked, not
    // the return code.
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

/// The stable name for a node: its `by-path` symlink.
///
/// Not `by-id`, which on the reference machine produces two symlinks for four nodes and
/// **neither resolves to the IR camera** — both UVC interfaces report the same serial, so
/// udev's basename collides and the survivor points at the RGB sensor
/// (wiki/10 Probe 4). Not `videoN` either; it renumbers across boots.
///
/// udev also emits a `usbv2-` spelling of the same path. Both resolve to the same node, so
/// the choice between them only needs to be *stable*: sorting puts `usb-` first and keeps
/// it there.
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

    // `Capabilities::capabilities` carries `device_caps` from the ioctl, which is the
    // per-node value. The union across the whole device would report VIDEO_CAPTURE for the
    // metadata nodes too, and then the format probe below would be aimed at a node that
    // can never answer.
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

/// `/dev/video12` → `Some(12)`. Also the "is this a video node" test, so that `videoX` or
/// a directory called `video` is skipped rather than opened.
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

    /// The reference machine, as wiki/10 Probe 1 recorded it. `video1` and `video3` are
    /// metadata nodes and never reach `select` at all.
    #[test]
    fn picks_the_ir_camera_on_the_reference_machine() {
        let saw = [
            candidate("/dev/video0", "ASUS FHD webcam: ASUS FHD webca", false, 0),
            candidate("/dev/video2", "ASUS FHD webcam: ASUS IR camera", true, 640 * 360),
        ];
        assert_eq!(select(&saw), Some(1));
    }

    /// GREY is a filter, not a preference. A device that cannot produce 8-bit greyscale
    /// cannot feed this pipeline whatever its name says, and answering "found it" would
    /// hand the RGB sensor to a pipeline that believes it is holding IR.
    #[test]
    fn a_camera_called_ir_that_cannot_do_grey_is_not_selected() {
        let saw = [candidate("/dev/video0", "Vendor IR camera", false, 640 * 360)];
        assert_eq!(select(&saw), None);
    }

    /// Two greyscale devices: the name breaks the tie before the resolution does.
    #[test]
    fn the_name_outranks_the_sensor_size() {
        let saw = [
            candidate("/dev/video0", "Generic mono machine vision", true, 320 * 240),
            candidate("/dev/video4", "Acme NIR sensor", true, 1280 * 720),
        ];
        assert_eq!(select(&saw), Some(1));
    }

    /// With nothing else to go on, the smaller sensor wins: wiki/05 notes IR sensors run
    /// 340×340 / 400×400 / 640×360, not 1280×720.
    #[test]
    fn the_smaller_sensor_breaks_a_nameless_tie() {
        let saw = [
            candidate("/dev/video0", "Anonymous camera", true, 1280 * 720),
            candidate("/dev/video4", "Anonymous camera", true, 640 * 360),
        ];
        assert_eq!(select(&saw), Some(1));
    }

    /// Identical hardware must not pick a different device on the next boot.
    #[test]
    fn a_total_tie_resolves_to_the_earlier_node() {
        let saw = [
            candidate("/dev/video0", "Twin", true, 640 * 360),
            candidate("/dev/video4", "Twin", true, 640 * 360),
        ];
        assert_eq!(select(&saw), Some(0));
    }

    /// `contains("IR")` fires on half the English language, and the wrong camera is the
    /// exact failure wiki/10 Probe 4 is about.
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

    /// Nothing found is three different problems and `diagnose` has to tell them apart.
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

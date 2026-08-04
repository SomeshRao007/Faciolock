//! Frames, the ON/OFF pairing, and the burst everything else is a property of.
//!
//! The arithmetic here is ported from `spikes/probe`, where it was measured against the
//! Phase 0 corpus rather than reasoned about. Two rules it carries with it:
//!
//! * **Nothing is thresholded on absolute brightness.** The OFF floor moved 10x between
//!   two sessions on the same machine and the ON series droops 13% inside a single
//!   capture (wiki/10 Probe 3b). Only ratios and shapes survive that.
//! * **ON/OFF phase is re-established by measuring luma on every capture**, never by
//!   assuming frame `n` is ON because frame `n-1` was OFF. Dropped, repeated and
//!   reordered buffers are all real, and ffmpeg silently doing exactly this is what
//!   [B5](../../../wiki/11-bug-log.md) cost.

use crate::config::{Config, Tier};

/// A rectangle in full-frame pixel coordinates.
///
/// Detection results and liveness ROIs share this type on purpose: the falloff test is
/// defined over the detected face region, so the box the detector produces and the box
/// liveness measures over must be the same coordinate space or the test measures nothing
/// ([B7](../../../wiki/11-bug-log.md)).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    /// Snap to whole pixels inside `width`x`height`, dropping to `None` if nothing of
    /// the rectangle is left. Detection boxes routinely hang off the edge of a frame.
    pub fn clamped(&self, width: usize, height: usize) -> Option<PixelRect> {
        let x0 = self.x.max(0.0).floor() as usize;
        let y0 = self.y.max(0.0).floor() as usize;
        let x1 = ((self.x + self.w).ceil() as isize).clamp(0, width as isize) as usize;
        let y1 = ((self.y + self.h).ceil() as isize).clamp(0, height as isize) as usize;
        if x1 <= x0 || y1 <= y0 || x0 >= width || y0 >= height {
            return None;
        }
        Some(PixelRect { x0, y0, w: x1 - x0, h: y1 - y0 })
    }

    pub fn area(&self) -> f32 {
        (self.w.max(0.0)) * (self.h.max(0.0))
    }

    /// Intersection-over-union, for non-maximum suppression.
    pub fn iou(&self, other: &Rect) -> f32 {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.w).min(other.x + other.w);
        let y1 = (self.y + self.h).min(other.y + other.h);
        if x1 <= x0 || y1 <= y0 {
            return 0.0;
        }
        let inter = (x1 - x0) * (y1 - y0);
        let union = self.area() + other.area() - inter;
        if union <= 0.0 { 0.0 } else { inter / union }
    }
}

/// A whole-pixel rectangle known to lie inside a frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PixelRect {
    pub x0: usize,
    pub y0: usize,
    pub w: usize,
    pub h: usize,
}

impl PixelRect {
    /// Shrink to a whole number of `grid`x`grid` cells so every cell holds the same
    /// pixel count. Unequal cells would put a systematic bias straight into the falloff
    /// variance, which is the one quantity the print-rejection claim rests on.
    pub fn tiled(&self, grid: usize) -> Option<PixelRect> {
        let w = (self.w / grid) * grid;
        let h = (self.h / grid) * grid;
        if w == 0 || h == 0 {
            return None;
        }
        Some(PixelRect { x0: self.x0, y0: self.y0, w, h })
    }
}

/// One 8-bit greyscale frame, borrowed.
///
/// Borrowed and not owned because the daemon streams MMAP'd V4L2 buffers: they are
/// mapped, not allocated, and copying each one would cost the memory budget for nothing.
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    pub pixels: &'a [u8],
    pub width: usize,
    pub height: usize,
}

impl<'a> Frame<'a> {
    pub fn new(pixels: &'a [u8], width: usize, height: usize) -> Option<Frame<'a>> {
        if width == 0 || height == 0 || pixels.len() != width * height {
            return None;
        }
        Some(Frame { pixels, width, height })
    }

    /// Mean luma over the whole frame. The quantity ON/OFF classification runs on.
    pub fn mean(&self) -> f64 {
        if self.pixels.is_empty() {
            return 0.0;
        }
        let sum: u64 = self.pixels.iter().map(|&p| u64::from(p)).sum();
        sum as f64 / self.pixels.len() as f64
    }

    /// Mean luma inside a rectangle.
    pub fn mean_in(&self, r: &PixelRect) -> f64 {
        let mut sum = 0u64;
        for y in r.y0..r.y0 + r.h {
            let start = y * self.width + r.x0;
            sum += self.pixels[start..start + r.w].iter().map(|&p| u64::from(p)).sum::<u64>();
        }
        sum as f64 / (r.w * r.h) as f64
    }

    /// Mean luma of each cell of a `grid`x`grid` tiling of `r`, row-major.
    ///
    /// `r` must already be tiled ([`PixelRect::tiled`]) or the trailing pixels are
    /// simply not sampled.
    pub fn cell_means(&self, r: &PixelRect, grid: usize) -> Vec<f64> {
        let cw = r.w / grid;
        let ch = r.h / grid;
        let px = (cw * ch) as f64;
        let mut out = Vec::with_capacity(grid * grid);
        for gy in 0..grid {
            for gx in 0..grid {
                let mut sum = 0u64;
                for y in r.y0 + gy * ch..r.y0 + (gy + 1) * ch {
                    let start = y * self.width + r.x0 + gx * cw;
                    sum += self.pixels[start..start + cw].iter().map(|&p| u64::from(p)).sum::<u64>();
                }
                out.push(sum as f64 / px);
            }
        }
        out
    }

    /// Mean absolute per-pixel difference against another frame, inside `r`.
    ///
    /// The motion statistic. B9 established that the *mean* of a dark frame carries
    /// almost no information — read noise dominates it — so this compares the images
    /// rather than their averages.
    pub fn mean_abs_diff_in(&self, other: &Frame<'_>, r: &PixelRect) -> f64 {
        let mut sum = 0u64;
        for y in r.y0..r.y0 + r.h {
            let a = y * self.width + r.x0;
            let b = y * other.width + r.x0;
            for i in 0..r.w {
                sum += u64::from(self.pixels[a + i].abs_diff(other.pixels[b + i]));
            }
        }
        sum as f64 / (r.w * r.h) as f64
    }
}

/// Whether the emitter was firing when a frame was exposed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    On,
    Off,
}

/// An emitter-ON frame paired with an adjacent emitter-OFF frame.
///
/// Adjacency matters. Neighbouring frames are ~33 ms apart, short enough that the only
/// thing that changed between them is the emitter. Pairing an ON frame with a distant
/// OFF frame lets AGC drift and scene motion contaminate the response.
#[derive(Clone, Copy)]
pub struct Pair<'a> {
    pub on: Frame<'a>,
    pub off: Frame<'a>,
}

/// The pattern of illumination actually observed in a capture.
///
/// Deliberately *not* called a tier. A tier is configuration (wiki/05, and see
/// [`Tier`]); this is an observation, and the two are compared rather than conflated —
/// which is what stops a bright enough attacker from talking the machine down into a
/// weaker mode.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Observed {
    /// Clean ON/OFF alternation.
    Strobing,
    /// Illuminated, but not alternating.
    Constant,
    /// Every frame dark: the emitter never fired.
    Dark,
    /// Frames vary, but not as a strobe.
    Irregular,
}

/// Why a capture could not become a burst worth authenticating against.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BurstError {
    /// Not enough frames survived AGC discard to look for pairs in.
    TooFewFrames { got: usize },
    /// Frames disagree on geometry, or a frame is not `width * height` bytes.
    Malformed,
    /// The capture does not illuminate the way the configured tier says this hardware
    /// does. **Never a downgrade** — see [`Tier`].
    TierMismatch { configured: Tier, observed: Observed },
    /// Alternation was found but yielded too few usable pairs.
    TooFewPairs { got: usize, need: usize },
}

/// A capture that conforms to the configured tier and is worth measuring.
pub struct Burst<'a> {
    pairs: Vec<Pair<'a>>,
    observed: Observed,
    width: usize,
    height: usize,
}

impl<'a> Burst<'a> {
    /// Build a burst from frames already split out of a buffer.
    pub fn from_frames(frames: &[Frame<'a>], cfg: &Config) -> Result<Burst<'a>, BurstError> {
        let (w, h) = match frames.first() {
            None => return Err(BurstError::TooFewFrames { got: 0 }),
            Some(f) => (f.width, f.height),
        };
        if frames.iter().any(|f| f.width != w || f.height != h) {
            return Err(BurstError::Malformed);
        }

        let usable = &frames[cfg.agc_settle_frames.min(frames.len())..];
        if usable.len() < 2 {
            return Err(BurstError::TooFewFrames { got: usable.len() });
        }

        let means: Vec<f64> = usable.iter().map(|f| f.mean()).collect();
        let (phases, observed) = classify(&means);

        // The configured tier is what this hardware was enrolled as. A capture that does
        // not behave that way is an anomaly to reject, not a hint to run weaker checks.
        let conforms = match cfg.tier {
            Tier::IrStrobe => observed == Observed::Strobing,
            Tier::IrConstant => observed == Observed::Constant,
            Tier::Rgb => observed != Observed::Dark,
        };
        if !conforms {
            return Err(BurstError::TierMismatch { configured: cfg.tier, observed });
        }

        let pairs = build_pairs(usable, &phases);
        if pairs.len() < cfg.min_pairs {
            return Err(BurstError::TooFewPairs { got: pairs.len(), need: cfg.min_pairs });
        }

        Ok(Burst { pairs, observed, width: w, height: h })
    }

    /// Build a burst from one contiguous planar buffer of back-to-back frames — the
    /// shape a `.gray` corpus recording arrives in.
    pub fn from_planar(
        bytes: &'a [u8],
        width: usize,
        height: usize,
        cfg: &Config,
    ) -> Result<Burst<'a>, BurstError> {
        let stride = width.checked_mul(height).filter(|&s| s > 0).ok_or(BurstError::Malformed)?;
        if bytes.len() < stride || !bytes.len().is_multiple_of(stride) {
            return Err(BurstError::Malformed);
        }
        let frames: Vec<Frame<'a>> = bytes
            .chunks_exact(stride)
            .filter_map(|c| Frame::new(c, width, height))
            .collect();
        Burst::from_frames(&frames, cfg)
    }

    pub fn pairs(&self) -> &[Pair<'a>] {
        &self.pairs
    }
    pub fn observed(&self) -> Observed {
        self.observed
    }
    pub fn width(&self) -> usize {
        self.width
    }
    pub fn height(&self) -> usize {
        self.height
    }
}

/// Split frames into ON and OFF by mean luma, and judge what pattern they form.
///
/// The split point is the midpoint between the dimmest and brightest frame. On strobing
/// hardware the two populations sit four to five orders of magnitude apart, so where
/// exactly the cut falls between them cannot matter. What matters is recognising when
/// there is no separation at all — that is the constant and dark cases, which is why the
/// spread is checked before anything is split.
pub fn classify(means: &[f64]) -> (Vec<Phase>, Observed) {
    if means.is_empty() {
        return (Vec::new(), Observed::Irregular);
    }

    let lo = means.iter().cloned().fold(f64::INFINITY, f64::min);
    let hi = means.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    // Require the bright population to be at least 10x the dark one before calling it a
    // strobe. Less than that is AGC wobble on a constantly-lit sensor, not an emitter
    // being switched.
    if !(hi > lo * 10.0 && hi > 1.0) {
        let observed = if hi < 1.0 { Observed::Dark } else { Observed::Constant };
        return (means.iter().map(|_| Phase::On).collect(), observed);
    }

    let split = (lo + hi) / 2.0;
    let phases: Vec<Phase> =
        means.iter().map(|&m| if m > split { Phase::On } else { Phase::Off }).collect();

    // A strobe alternates every frame. Count how often it actually does.
    let flips = phases.windows(2).filter(|w| w[0] != w[1]).count();
    let observed = if flips >= phases.len().saturating_sub(1) * 9 / 10 {
        Observed::Strobing
    } else {
        Observed::Irregular
    };

    (phases, observed)
}

/// Pair each ON frame with an adjacent OFF frame, using no frame twice.
fn build_pairs<'a>(frames: &[Frame<'a>], phases: &[Phase]) -> Vec<Pair<'a>> {
    let mut pairs = Vec::new();
    let mut i = 0;
    while i + 1 < phases.len() {
        let (on_i, off_i) = match (phases[i], phases[i + 1]) {
            (Phase::On, Phase::Off) => (i, i + 1),
            (Phase::Off, Phase::On) => (i + 1, i),
            _ => {
                i += 1;
                continue;
            }
        };
        pairs.push(Pair { on: frames[on_i], off: frames[off_i] });
        i += 2;
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{blank, domed, flat, H, W};

    fn cfg() -> Config {
        Config { agc_settle_frames: 0, ..Config::default() }
    }

    #[test]
    fn mean_is_the_arithmetic_mean() {
        let px = blank(W, H, 40);
        let f = Frame::new(&px, W, H).unwrap();
        assert!((f.mean() - 40.0).abs() < 1e-9);
        let r = PixelRect { x0: 8, y0: 8, w: 24, h: 24 };
        assert!((f.mean_in(&r) - 40.0).abs() < 1e-9);
    }

    #[test]
    fn frame_rejects_a_buffer_of_the_wrong_length() {
        assert!(Frame::new(&[0u8; 10], W, H).is_none());
        assert!(Frame::new(&[], 0, 0).is_none());
    }

    #[test]
    fn tiled_rect_divides_evenly_into_cells() {
        let r = PixelRect { x0: 0, y0: 0, w: 40, h: 40 }.tiled(6).unwrap();
        assert_eq!(r.w % 6, 0);
        assert_eq!(r.h % 6, 0);
        let px = blank(W, H, 10);
        let f = Frame::new(&px, W, H).unwrap();
        assert_eq!(f.cell_means(&r, 6).len(), 36);
    }

    #[test]
    fn a_rect_entirely_outside_the_frame_clamps_to_nothing() {
        let r = Rect { x: -50.0, y: -50.0, w: 10.0, h: 10.0 };
        assert!(r.clamped(W, H).is_none());
        let r = Rect { x: 5.0, y: 5.0, w: 1000.0, h: 1000.0 };
        assert_eq!(r.clamped(W, H).unwrap().w, W - 5);
    }

    #[test]
    fn iou_is_one_for_identical_boxes_and_zero_for_disjoint_ones() {
        let a = Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 };
        let b = Rect { x: 20.0, y: 20.0, w: 10.0, h: 10.0 };
        assert!((a.iou(&a) - 1.0).abs() < 1e-6);
        assert_eq!(a.iou(&b), 0.0);
    }

    #[test]
    fn classify_detects_a_strobe() {
        let means: Vec<f64> = (0..30).map(|i| if i % 2 == 0 { 60.0 } else { 0.004 }).collect();
        let (phases, observed) = classify(&means);
        assert_eq!(observed, Observed::Strobing);
        assert_eq!(phases[0], Phase::On);
        assert_eq!(phases[1], Phase::Off);
    }

    #[test]
    fn classify_detects_constant_illumination() {
        // AGC wobble on a steadily-lit sensor must not read as a strobe.
        let means: Vec<f64> = (0..30).map(|i| 55.0 + (i % 3) as f64).collect();
        assert_eq!(classify(&means).1, Observed::Constant);
    }

    #[test]
    fn classify_detects_a_dark_emitter() {
        let means: Vec<f64> = (0..30).map(|i| 0.004 + (i % 3) as f64 * 0.0001).collect();
        assert_eq!(classify(&means).1, Observed::Dark);
    }

    /// The tier-downgrade guard. A capture that does not strobe must be rejected outright
    /// when the machine is enrolled as strobing hardware — never quietly served by the
    /// weaker checks a constant-illumination tier would run. An attacker with a bright
    /// enough continuous IR source can *produce* a constant capture at will, so treating
    /// that as a hint would hand them the tier they can pass.
    #[test]
    fn constant_illumination_is_rejected_not_downgraded() {
        let px = blank(W, H, 60);
        let frames: Vec<Frame<'_>> = (0..20).map(|_| Frame::new(&px, W, H).unwrap()).collect();
        match Burst::from_frames(&frames, &cfg()) {
            Err(BurstError::TierMismatch { configured: Tier::IrStrobe, observed }) => {
                assert_eq!(observed, Observed::Constant);
            }
            other => panic!("expected a tier mismatch, got {:?}", other.map(|_| "a burst")),
        }
    }

    /// Conjunction over an empty set is true, so a burst with no pairs would satisfy
    /// every liveness check that was meant to inspect it. It has to fail here instead.
    #[test]
    fn a_burst_with_too_few_pairs_is_an_error_not_an_empty_pass() {
        let on = domed(W, H, 60, 10);
        let off = blank(W, H, 0);
        // Two frames make exactly one pair; the floor is two.
        let frames =
            vec![Frame::new(&on, W, H).unwrap(), Frame::new(&off, W, H).unwrap()];
        assert!(matches!(
            Burst::from_frames(&frames, &cfg()),
            Err(BurstError::TooFewPairs { got: 1, need: 2 })
        ));
    }

    #[test]
    fn pairs_never_reuse_a_frame() {
        let on = domed(W, H, 60, 10);
        let off = blank(W, H, 0);
        let mut frames = Vec::new();
        for i in 0..20 {
            let px: &[u8] = if i % 2 == 0 { &on } else { &off };
            frames.push(Frame::new(px, W, H).unwrap());
        }
        let burst = Burst::from_frames(&frames, &cfg()).unwrap();
        assert_eq!(burst.observed(), Observed::Strobing);
        assert_eq!(burst.pairs().len(), 10);
    }

    #[test]
    fn planar_input_must_be_a_whole_number_of_frames() {
        let bytes = vec![0u8; W * H + 7];
        assert!(matches!(
            Burst::from_planar(&bytes, W, H, &cfg()),
            Err(BurstError::Malformed)
        ));
        assert!(matches!(
            Burst::from_planar(&[0u8; 10], W, H, &cfg()),
            Err(BurstError::Malformed)
        ));
    }

    /// Two frames with *identical* means but different pixels. This is the shape of the
    /// failure B9 exposed: a statistic computed over frame averages cannot tell these
    /// apart, which is why the motion check compares images instead of their means.
    #[test]
    fn mean_abs_diff_sees_a_change_the_mean_alone_cannot() {
        let a = flat(W, H, 60);
        let mut b = vec![0u8; W * H];
        for (i, p) in b.iter_mut().enumerate() {
            *p = if i % 2 == 0 { 0 } else { 120 };
        }
        let fa = Frame::new(&a, W, H).unwrap();
        let fb = Frame::new(&b, W, H).unwrap();
        let r = PixelRect { x0: 0, y0: 0, w: W, h: H };
        assert_eq!(fa.mean(), fb.mean(), "means are equal by construction");
        assert!(fa.mean_abs_diff_in(&fb, &r) > 50.0, "per-pixel difference is not");
    }
}

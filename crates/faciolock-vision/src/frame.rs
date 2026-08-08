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
///
/// No `Eq`: [`BurstError::AmbientTooBright`] carries the measured floor, and the audit log
/// in Phase 4 wants the number rather than just the fact.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BurstError {
    /// Ambient near-infrared is high enough that the emitter no longer carries the signal
    /// every liveness check depends on. **Not a failed authentication** — the capture was
    /// never measurable. See [`Config::ambient_floor_max`](crate::Config).
    AmbientTooBright { floor: f32 },
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

        // **The ambient gate, and it comes first on purpose.**
        //
        // Before tier conformance, before liveness, before detection — so that nothing
        // biometric runs on a capture this hardware cannot measure, and so that a bright
        // room reports itself as bright rather than as a tier mismatch. An `Environment`
        // outcome that had already detected a face would also make the rate-limiter
        // exemption in Phase 4 into a free probing oracle.
        let floor = means.iter().cloned().fold(f64::INFINITY, f64::min);
        if floor > cfg.ambient_floor_max {
            return Err(BurstError::AmbientTooBright { floor: floor as f32 });
        }

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

/// How far the emitter's contribution must sit above the same-phase noise floor before a
/// capture is called a strobe.
///
/// **Not a security boundary.** This answers "is this hardware doing what it was enrolled
/// as", not "is this an attack" — a floodlight attack is *also* mounted against strobing
/// hardware and reads as `Strobing` here, correctly. Rejecting it is liveness' job.
///
/// So the bar is set for confidence that an emitter exists at all, and every capture the
/// project has clears it by a wide margin:
///
/// ```text
/// live, nocturnal (5 captures)      931 – 15,461
/// live, daylight  (1 capture)             549
/// floodlight attack, nocturnal             53   <- still strobing hardware, correctly
/// ```
///
/// 10 is ~5x below the weakest thing measured. A continuous source bright enough to drive
/// the emitter under it fails this and is rejected as `TierMismatch`, which is fail-closed
/// and is P1-e working as intended.
const STROBE_SNR_MIN: f64 = 10.0;

/// Fraction of steps that must reverse direction.
///
/// Every capture the project has reads exactly 1.000 — including the daylight ones, which
/// is the measurement that showed the emitter never stopped strobing. Carried at 0.9 to
/// tolerate a dropped frame, matching the pre-B18 `flips >= 90%` rule it replaces.
const STROBE_REGULARITY_MIN: f64 = 0.9;

/// Split frames into ON and OFF by mean luma, and judge what pattern they form.
///
/// **This measures a difference, not a ratio, and that is [B18]'s fix.**
///
/// The original asked whether the brightest frame was 10x the dimmest. On this hardware
/// that is 15,000:1 after dark and 1.15:1 in daylight, with the emitter firing every single
/// frame in both cases — because ambient near-infrared adds a pedestal to *both* phases and
/// a ratio saturates against a pedestal. Every corpus recording had been made at night and
/// nothing recorded that fact, so the whole corpus described one lighting condition without
/// saying so:
///
/// ```text
///                        ambient floor   amplitude   same-phase noise      SNR   ratio test
/// live, nocturnal              0.0002        54–81        0.005–0.047    1,525+   Strobing
/// live, daylight                51.74        32.99              0.060       549   Constant  <-
/// ```
///
/// The emitter's contribution (`amplitude`) barely moved; only its *proportion* did. So the
/// statistic is the median step between neighbouring frames, divided by the median step
/// between frames of the same phase — a noise floor measured on the capture itself, since
/// wiki/10 Probe 3b showed that floor move tenfold between two runs on one machine.
/// Subtracting a constant from every frame changes neither term.
///
/// [B18]: ../../../wiki/11-bug-log.md
pub fn classify(means: &[f64]) -> (Vec<Phase>, Observed) {
    if means.len() < 3 {
        // Three frames is the minimum that contains a same-phase pair, and the noise floor
        // below is measured from those. Fewer is not a weak measurement, it is no
        // measurement -- and an unmeasurable capture must not be able to pass for a strobe.
        return (means.iter().map(|_| Phase::On).collect(), Observed::Irregular);
    }

    let hi = means.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let steps: Vec<f64> = means.windows(2).map(|w| w[1] - w[0]).collect();

    // The emitter's contribution to the frame mean, and the floor it has to beat.
    //
    // `same_phase` skips a frame, so the emitter is in the *same* state at both ends and
    // what remains is AGC drift, scene motion and read noise -- measured on this capture
    // rather than assumed, because wiki/10 Probe 3b showed the floor move by an order of
    // magnitude between two runs on the same machine.
    let amplitude = median_abs(&steps);
    let same_phase = median_abs(&means.windows(3).map(|w| w[2] - w[0]).collect::<Vec<_>>());
    let snr = match (amplitude > 0.0, same_phase > 0.0) {
        // A zero floor is a real reading on this hardware at night: two true dark frames
        // are identical, so the emitter beat the noise by more than the capture can
        // resolve.
        (true, false) => f64::INFINITY,
        (true, true) => amplitude / same_phase,
        // No step between neighbouring frames at all. Nothing is alternating, and 0/0 must
        // not read as "infinitely clean".
        (false, _) => 0.0,
    };

    // A strobe reverses direction every frame. Drift, however steep, does not.
    let reversals = steps.windows(2).filter(|w| w[0] * w[1] < 0.0).count();
    let regularity = reversals as f64 / steps.len().saturating_sub(1).max(1) as f64;

    if snr < STROBE_SNR_MIN || regularity < STROBE_REGULARITY_MIN {
        // The three not-a-strobe answers, kept distinct because they mean different
        // things to an operator: nothing is lit, something is lit but steady, or something
        // is varying strongly without alternating.
        let observed = if hi < 1.0 {
            Observed::Dark
        } else if snr < STROBE_SNR_MIN {
            Observed::Constant
        } else {
            Observed::Irregular
        };
        return (means.iter().map(|_| Phase::On).collect(), observed);
    }

    // Phase by comparison with immediate neighbours, **not** against a split of the global
    // range. The global split is what the pre-B18 version used and it holds only while the
    // capture's drift stays smaller than the emitter's amplitude. In daylight the ON series
    // climbed 105 -> 116 while OFF climbed 92 -> 102, leaving 3.5 levels of headroom on a
    // 14-level amplitude; a slightly longer capture would have put a late OFF frame above
    // an early ON one and silently mislabelled both. Neighbours cannot drift apart, so this
    // is immune to both drift and to any ambient pedestal.
    let phases: Vec<Phase> = (0..means.len())
        .map(|i| {
            let reference = match (i.checked_sub(1), means.get(i + 1)) {
                (Some(p), Some(&n)) => (means[p] + n) / 2.0,
                (Some(p), None) => means[p],
                (None, Some(&n)) => n,
                (None, None) => return Phase::On,
            };
            if means[i] > reference { Phase::On } else { Phase::Off }
        })
        .collect();

    (phases, Observed::Strobing)
}

fn median_abs(v: &[f64]) -> f64 {
    let mut s: Vec<f64> = v.iter().map(|x| x.abs()).collect();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if s.is_empty() { 0.0 } else { s[s.len() / 2] }
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
    use crate::testing::{blank, domed, flat, strobe, H, W};

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
    fn constant_capture(level: u8) -> Vec<u8> {
        blank(W, H, level)
    }

    /// The tier-downgrade guard, and it now has two layers.
    ///
    /// **The gate answers first, and that is the better answer.** A capture that is
    /// uniformly lit at level 60 *is* a bright room — the ambient floor equals the frame
    /// mean — so reporting "too bright to verify" is more accurate than "wrong illumination
    /// pattern". Before [B18] the only available answer was the latter.
    ///
    /// What must not change is the property underneath: an attacker with a continuous IR
    /// source can produce a non-strobing capture at will, and no arrangement of that
    /// capture may ever yield a `Burst` on hardware enrolled as strobing. Both layers
    /// refuse; neither downgrades.
    ///
    /// [B18]: ../../../wiki/11-bug-log.md
    #[test]
    fn constant_illumination_is_rejected_not_downgraded() {
        let px = constant_capture(60);
        let frames: Vec<Frame<'_>> = (0..20).map(|_| Frame::new(&px, W, H).unwrap()).collect();
        match Burst::from_frames(&frames, &cfg()) {
            Err(BurstError::AmbientTooBright { floor }) => {
                assert!((floor - 60.0).abs() < 0.5, "floor {floor} should be the frame mean");
            }
            other => panic!("expected the ambient gate, got {:?}", other.map(|_| "a burst")),
        }
    }

    /// The same property stated as a property: across the whole brightness range an
    /// attacker could produce, a constant capture never becomes a burst. Which layer
    /// catches it is an implementation detail; that one of them always does is not.
    #[test]
    fn no_constant_capture_ever_yields_a_burst() {
        for level in [0u8, 1, 5, 30, 60, 120, 200, 255] {
            let px = constant_capture(level);
            let frames: Vec<Frame<'_>> = (0..20).map(|_| Frame::new(&px, W, H).unwrap()).collect();
            assert!(
                Burst::from_frames(&frames, &cfg()).is_err(),
                "constant illumination at {level} produced a burst"
            );
        }
    }

    /// And the gate itself: it fires on the ambient floor, so a capture that is dark on
    /// its OFF frames still passes however bright its ON frames are. Otherwise a strong
    /// emitter would gate itself out.
    #[test]
    fn the_gate_measures_the_floor_and_not_the_peak() {
        let on = domed(W, H, 200, 150); // a very bright ON frame
        let off = blank(W, H, 0);
        let bytes = strobe(&on, &off, 4);
        assert!(
            Burst::from_planar(&bytes, W, H, &cfg()).is_ok(),
            "a bright emitter over a dark room must not trip the ambient gate"
        );
    }

    /// Conjunction over an empty set is true, so a burst with no pairs would satisfy
    /// every liveness check that was meant to inspect it. It has to fail here instead.
    ///
    /// Three frames, not two: since [B18] the noise floor is measured from same-phase
    /// pairs, and two frames contain none, so a two-frame capture is now unclassifiable
    /// rather than thinly classifiable. `ON, OFF, ON` is the shortest capture that both
    /// classifies as a strobe and yields fewer pairs than the floor.
    ///
    /// [B18]: ../../../wiki/11-bug-log.md
    #[test]
    fn a_burst_with_too_few_pairs_is_an_error_not_an_empty_pass() {
        let on = domed(W, H, 60, 10);
        let off = blank(W, H, 0);
        let frames = vec![
            Frame::new(&on, W, H).unwrap(),
            Frame::new(&off, W, H).unwrap(),
            Frame::new(&on, W, H).unwrap(),
        ];
        assert!(matches!(
            Burst::from_frames(&frames, &cfg()),
            Err(BurstError::TooFewPairs { got: 1, need: 2 })
        ));
    }

    /// And the shorter capture is refused too — earlier, and for a reason that names the
    /// real problem. A capture too short to measure its own noise floor must not be able
    /// to pass for a strobe, or "unmeasurable" becomes a way through.
    #[test]
    fn a_capture_too_short_to_measure_is_not_a_strobe() {
        let on = domed(W, H, 60, 10);
        let off = blank(W, H, 0);
        let frames = vec![Frame::new(&on, W, H).unwrap(), Frame::new(&off, W, H).unwrap()];
        assert!(matches!(
            Burst::from_frames(&frames, &cfg()),
            Err(BurstError::TierMismatch { observed: Observed::Irregular, .. })
        ));
        assert_eq!(classify(&[60.0, 0.0]).1, Observed::Irregular);
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

//! Liveness: arithmetic, not a neural network.
//!
//! **A liveness verdict is a property of the burst, not of a frame.** Every check here
//! aggregates *conjunctively* — one contaminated pair sinks the whole burst — and that is
//! not a stylistic choice. A pulsed IR source idles between packets, so 7 of 42 pairs of a
//! real floodlight attack read as perfectly clean. Under a per-pair rule that is a
//! 1-in-36 bypass at the two-pair capture budget; under conjunction it rejects with four
//! orders of magnitude to spare ([B9](../../../wiki/11-bug-log.md)).
//!
//! Matching, in `embed`, aggregates k-of-n instead — because there the risk runs the other
//! way and a single bad frame is a blink. **The two rules are opposites and any refactor
//! that unifies them reintroduces B9.**
//!
//! The checks split into two groups by whether they need a face box, which is what lets
//! the pipeline reject a screen or a floodlight for a few `f32` reductions without paying
//! for detection at all.

use crate::config::{Config, Enforcement, Threshold};
use crate::frame::{Burst, Pair, PixelRect, Rect};

/// Cells per side of the falloff grid.
///
/// 6x6 gives 36 samples of the face box: enough for the shape of the falloff to show up,
/// coarse enough that each cell still averages over hundreds of pixels rather than sensor
/// noise. On a typical 134x181 detected face that is ~660 px per cell.
pub const GRID: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Check {
    /// The OFF frame must be dark *relative to the ON frame of the same pair*.
    ///
    /// Never an absolute floor: the OFF floor measured 0.004 in one session and 0.0005 in
    /// another on the same machine, from ambient light alone. A fixed floor calibrated in
    /// a dark room rejects every honest attempt in daylight; one calibrated in daylight
    /// accepts a floodlight attack at night.
    DarkFrame,
    /// The emitter must actually be lighting whatever is in front of it. An LCD or OLED
    /// emits essentially nothing in NIR, so a phone or laptop replay collapses this to ~0.
    Response,
    /// The response must vary across the face. A live face is a curved, subsurface-
    /// scattering reflector — nose and brow brighten far more than cheeks and jaw. A flat
    /// print reflects near-uniformly.
    Falloff,
    /// Consecutive OFF frames should be near-identical over ~100 ms.
    Motion,
}

impl Check {
    pub fn name(&self) -> &'static str {
        match self {
            Check::DarkFrame => "dark-frame",
            Check::Response => "response",
            Check::Falloff => "falloff",
            Check::Motion => "motion",
        }
    }
}

/// Which direction of comparison fails a check.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fails {
    /// Reject when the measured value is above the threshold.
    Above,
    /// Reject when the measured value is below the threshold.
    Below,
}

/// One check's outcome over a whole burst.
#[derive(Clone, Copy, Debug)]
pub struct CheckResult {
    pub check: Check,
    /// The worst value any single pair produced — the one conjunction turns on.
    pub worst: f64,
    pub threshold: f64,
    pub enforcement: Enforcement,
    /// How many pairs failed. Non-zero on a passing burst is possible only for an
    /// advisory check, and is worth logging when it happens.
    pub failing_pairs: usize,
}

impl CheckResult {
    pub fn passed(&self) -> bool {
        self.failing_pairs == 0
    }
    /// True only when this check both failed and is trusted enough to say so.
    pub fn rejects(&self) -> bool {
        !self.passed() && self.enforcement == Enforcement::Enforced
    }
}

/// Everything measured about a burst, and whether any of it is grounds for refusal.
#[derive(Clone, Debug, Default)]
pub struct LivenessReport {
    pub results: Vec<CheckResult>,
    pub pairs: usize,
}

impl LivenessReport {
    /// The first enforced failure, if any. `None` means nothing *provable* is wrong —
    /// which is not the same as "this is definitely a live face", and the distinction is
    /// why `results` is carried out of here rather than reduced to a bool.
    pub fn rejection(&self) -> Option<CheckResult> {
        self.results.iter().copied().find(CheckResult::rejects)
    }

    /// Checks that failed but were not trusted enough to reject on. These are the ones
    /// waiting on a spoof recording to calibrate against.
    pub fn advisory_failures(&self) -> impl Iterator<Item = &CheckResult> {
        self.results.iter().filter(|r| !r.passed() && r.enforcement == Enforcement::Advisory)
    }

    pub fn get(&self, check: Check) -> Option<CheckResult> {
        self.results.iter().copied().find(|r| r.check == check)
    }

    /// Combine the pre-box and post-box halves into one report.
    pub fn merge(mut self, other: LivenessReport) -> LivenessReport {
        self.results.extend(other.results);
        self.pairs = self.pairs.max(other.pairs);
        self
    }
}

/// Fold one check across every pair of a burst, conjunctively.
///
/// The empty case returns `None` rather than a pass. Conjunction over zero pairs is
/// vacuously true, and a burst that produced no pairs would sail through every check that
/// was supposed to inspect it — an attacker who can suppress pairing would face no
/// liveness at all. `Burst` refuses to exist below `min_pairs` for the same reason; this
/// is the second lock on the same door.
fn conjunctive<F>(
    check: Check,
    t: Threshold,
    fails: Fails,
    pairs: &[Pair<'_>],
    mut measure: F,
) -> Option<CheckResult>
where
    F: FnMut(&Pair<'_>) -> Option<f64>,
{
    if pairs.is_empty() {
        return None;
    }
    let mut worst = match fails {
        Fails::Above => f64::NEG_INFINITY,
        Fails::Below => f64::INFINITY,
    };
    let mut failing = 0usize;
    let mut measured = 0usize;

    for p in pairs {
        let Some(v) = measure(p) else { continue };
        measured += 1;
        let bad = match fails {
            Fails::Above => v > t.value,
            Fails::Below => v < t.value,
        };
        if bad {
            failing += 1;
        }
        worst = match fails {
            Fails::Above => worst.max(v),
            Fails::Below => worst.min(v),
        };
    }

    // A check that could not be measured on any pair has not passed. Reporting it as a
    // pass is how "no evidence" becomes "evidence of absence" — see B3.
    if measured == 0 {
        return None;
    }

    Some(CheckResult {
        check,
        worst,
        threshold: t.value,
        enforcement: t.enforcement,
        failing_pairs: failing,
    })
}

/// The checks that need no face box, run before detection as an early-out.
///
/// Cheap: a handful of `f32` reductions against YuNet's ~11 ms. Detection is only reached
/// by something already behaving as though our emitter is lighting it.
pub fn pre_box(burst: &Burst<'_>, cfg: &Config) -> LivenessReport {
    let pairs = burst.pairs();
    let whole = PixelRect { x0: 0, y0: 0, w: burst.width(), h: burst.height() };
    let mut results = Vec::new();

    if let Some(r) = conjunctive(
        Check::DarkFrame,
        cfg.liveness.dark_ratio_max,
        Fails::Above,
        pairs,
        |p| {
            let on = p.on.mean_in(&whole);
            if on <= 0.0 {
                // No illumination at all: the ratio is undefined, and treating a
                // divide-by-nothing as "clean" would be exactly backwards.
                return Some(f64::INFINITY);
            }
            Some(p.off.mean_in(&whole) / on)
        },
    ) {
        results.push(r);
    }

    if let Some(r) =
        conjunctive(Check::Response, cfg.liveness.response_min, Fails::Below, pairs, |p| {
            Some(p.on.mean_in(&whole) - p.off.mean_in(&whole))
        })
    {
        results.push(r);
    }

    LivenessReport { results, pairs: pairs.len() }
}

/// The checks defined over the detected face region.
///
/// One box for the whole burst rather than one per pair: the frames are ~33 ms apart, a
/// face does not travel far in that time, and the latency budget affords exactly one
/// detection pass (wiki/06).
pub fn post_box(burst: &Burst<'_>, face: &Rect, cfg: &Config) -> LivenessReport {
    let pairs = burst.pairs();
    let Some(roi) = face
        .clamped(burst.width(), burst.height())
        .and_then(|r| r.tiled(GRID))
    else {
        // A box that does not survive clamping to the frame is not a region anything can
        // be measured over. No results means no passes.
        return LivenessReport { results: Vec::new(), pairs: pairs.len() };
    };

    let mut results = Vec::new();

    if let Some(r) =
        conjunctive(Check::Falloff, cfg.liveness.falloff_cv_min, Fails::Below, pairs, |p| {
            let on = p.on.cell_means(&roi, GRID);
            let off = p.off.cell_means(&roi, GRID);
            let d: Vec<f64> = on.iter().zip(&off).map(|(a, b)| a - b).collect();
            Some(falloff_cv(&d))
        })
    {
        results.push(r);
    }

    // Motion compares *consecutive* OFF frames, so it is defined on gaps between pairs
    // rather than on pairs. With n pairs there are n-1 comparisons.
    if pairs.len() >= 2 {
        let windows: Vec<Pair<'_>> = pairs
            .windows(2)
            .map(|w| Pair { on: w[0].on, off: w[0].off })
            .collect();
        let mut i = 0usize;
        if let Some(r) =
            conjunctive(Check::Motion, cfg.liveness.motion_max, Fails::Above, &windows, |_| {
                let a = pairs[i].off;
                let b = pairs[i + 1].off;
                let response = pairs[i].on.mean_in(&roi) - pairs[i].off.mean_in(&roi);
                i += 1;
                if response <= 0.0 {
                    return None;
                }
                // Normalised by the response so it is a ratio, not an absolute — the same
                // reason everything else here is.
                Some(a.mean_abs_diff_in(&b, &roi) / response)
            })
        {
            results.push(r);
        }
    }

    LivenessReport { results, pairs: pairs.len() }
}

/// Both halves, for callers that already hold a box and do not need the early-out.
pub fn evaluate(burst: &Burst<'_>, face: &Rect, cfg: &Config) -> LivenessReport {
    pre_box(burst, cfg).merge(post_box(burst, face, cfg))
}

/// Coefficient of variation of the per-cell response — the falloff metric.
///
/// Expressed as std/mean rather than raw std so it stays comparable across captures at
/// different distances and different AGC states. A negative mean means the "ON" frame was
/// darker than the "OFF" one over this region, which is not a falloff shape at all; it
/// reports 0 so a degenerate-flatness floor treats it as the failure it is.
pub fn falloff_cv(d_cells: &[f64]) -> f64 {
    if d_cells.is_empty() {
        return 0.0;
    }
    let m = mean(d_cells);
    if m <= f64::EPSILON {
        return 0.0;
    }
    let var = d_cells.iter().map(|v| (v - m).powi(2)).sum::<f64>() / d_cells.len() as f64;
    var.sqrt() / m
}

/// Ratio of response in the centre cells to response in the border ring.
///
/// A second, independent read on the same physics as [`falloff_cv`], and one with a
/// direction: on a face the centre outshines the periphery, so this sits above 1. On a
/// flat surface held square to the camera it sits near 1. Reported alongside CV because a
/// crumpled or tilted print could raise CV without producing a centre-bright pattern.
pub fn centre_edge_ratio(d_cells: &[f64]) -> f64 {
    let mut centre = Vec::new();
    let mut edge = Vec::new();
    for gy in 0..GRID {
        for gx in 0..GRID {
            let v = d_cells[gy * GRID + gx];
            if gx == 0 || gy == 0 || gx == GRID - 1 || gy == GRID - 1 {
                edge.push(v);
            } else if (GRID / 2 - 1..=GRID / 2).contains(&gx)
                && (GRID / 2 - 1..=GRID / 2).contains(&gy)
            {
                centre.push(v);
            }
        }
    }
    let e = mean(&edge);
    if e.abs() < f64::EPSILON { f64::INFINITY } else { mean(&centre) / e }
}

fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() { 0.0 } else { xs.iter().sum::<f64>() / xs.len() as f64 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Threshold, Tier};
    use crate::frame::Frame;
    use crate::testing::{blank, domed, flat, strobe, strobe_one_contaminated, H, W};

    fn cfg() -> Config {
        Config { agc_settle_frames: 0, tier: Tier::IrStrobe, ..Config::default() }
    }

    fn whole_face() -> Rect {
        Rect { x: 0.0, y: 0.0, w: W as f32, h: H as f32 }
    }

    /// The B9 regression, and the single most important test in this module.
    ///
    /// One contaminated pair out of four. A per-pair rule, a median, or a k-of-n vote all
    /// let this through. Only conjunction rejects it.
    #[test]
    fn one_contaminated_pair_sinks_the_whole_burst() {
        let on = domed(W, H, 60, 10);
        let dark = blank(W, H, 0);
        // Contamination at roughly the ratio the real attack produced — wiki/11 B9 pair 1
        // read mean(ON) 82.93 against mean(OFF) 9.29, so the OFF frame is polluted but
        // still an order of magnitude darker than the ON frame beside it. That is what
        // makes it dangerous: it stays classified as an OFF frame and the alternation
        // survives, so nothing upstream notices. See the test below for the louder case.
        let flooded = blank(W, H, 5);

        let clean = strobe(&on, &dark, 4);
        let dirty = strobe_one_contaminated(&on, &dark, &flooded, 4, 2);

        let c = cfg();
        let clean_burst = Burst::from_planar(&clean, W, H, &c).expect("clean burst");
        let dirty_burst = Burst::from_planar(&dirty, W, H, &c).expect("dirty burst");

        let clean_report = pre_box(&clean_burst, &c);
        let dirty_report = pre_box(&dirty_burst, &c);

        assert!(clean_report.rejection().is_none(), "{clean_report:?}");

        let r = dirty_report.rejection().expect("one bad pair must reject the burst");
        assert_eq!(r.check, Check::DarkFrame);
        assert_eq!(r.failing_pairs, 1, "exactly one pair was contaminated");

        // And the point of the bug: three of four pairs were spotless. A rule that looked
        // at the median, or required a majority to fail, would have passed this.
        let dark_result = dirty_report.get(Check::DarkFrame).unwrap();
        assert!(
            dark_result.failing_pairs * 2 < dirty_report.pairs,
            "the failing pairs are a minority — which is exactly why voting fails here"
        );
    }

    /// The louder half of the same attack.
    ///
    /// **Which layer catches this changed at [B18], and the change is an improvement.**
    /// Before, a contaminant bright enough to rival the ON frame pushed the glare frame
    /// above `classify`'s *global* ON/OFF split, the alternation appeared to break, and the
    /// burst was refused as a tier mismatch — rejected because the classifier had been
    /// confused, not because anything detected an attack. B18 replaced that split with a
    /// comparison against immediate neighbours, which labels the glare frame correctly as
    /// the OFF frame it is. So the burst now forms, and the attack is rejected by the check
    /// built to reject it, naming itself in the verdict.
    ///
    /// The cost is real and is recorded rather than glossed: this input used to be refused
    /// at two independent layers and is now refused at one. That makes the dark-frame check
    /// load-bearing on its own for the loud floodlight case, which is why replacing it
    /// (wiki/11 B18's open item) cannot be done without re-running this test.
    #[test]
    fn a_bright_contaminant_is_rejected_by_the_check_built_for_it() {
        let on = domed(W, H, 60, 10); // mean ~41
        let dark = blank(W, H, 0);
        let glare = blank(W, H, 30); // bright enough to rival the ON frame
        let bytes = strobe_one_contaminated(&on, &dark, &glare, 4, 2);
        let c = cfg();

        let burst = Burst::from_planar(&bytes, W, H, &c)
            .expect("neighbour-relative phase survives a bright contaminant");

        let r = pre_box(&burst, &c).rejection().expect("the attack must still be rejected");
        assert_eq!(r.check, Check::DarkFrame);
        assert_eq!(r.failing_pairs, 1, "exactly one pair carried the glare");
    }

    #[test]
    fn a_clean_strobe_of_a_curved_face_passes_every_check() {
        let on = domed(W, H, 60, 5);
        let dark = blank(W, H, 0);
        let c = cfg();
        let burst_bytes = strobe(&on, &dark, 6);
        let burst = Burst::from_planar(&burst_bytes, W, H, &c).unwrap();
        let report = evaluate(&burst, &whole_face(), &c);
        assert!(report.rejection().is_none(), "{report:?}");
        assert!(report.get(Check::Falloff).unwrap().passed());
        assert!(report.get(Check::Response).unwrap().passed());
    }

    /// The falloff test's mechanism, on data where the answer is known. If a curved
    /// reflector does not out-score a flat one here, the print-rejection claim in wiki/05
    /// has nothing behind it at all.
    #[test]
    fn a_curved_reflector_has_higher_falloff_than_a_flat_one() {
        let dark = blank(W, H, 0);
        let c = cfg();

        let curved_bytes = strobe(&domed(W, H, 60, 10), &dark, 4);
        let curved = Burst::from_planar(&curved_bytes, W, H, &c).unwrap();
        let print_bytes = strobe(&flat(W, H, 60), &dark, 4);
        let print = Burst::from_planar(&print_bytes, W, H, &c).unwrap();

        let curved_cv = evaluate(&curved, &whole_face(), &c).get(Check::Falloff).unwrap().worst;
        let flat_cv = evaluate(&print, &whole_face(), &c).get(Check::Falloff).unwrap().worst;

        assert!(flat_cv < 0.01, "a flat surface should be near-uniform, got {flat_cv}");
        assert!(curved_cv > flat_cv * 10.0, "curved {curved_cv} vs flat {flat_cv}");
    }

    /// Absolute brightness must not move the falloff verdict. AGC droop moves the ON
    /// series 13% inside one capture and 10x between sessions (wiki/10 Probe 3b).
    #[test]
    fn falloff_is_invariant_to_overall_gain() {
        let dark = blank(W, H, 0);
        let c = cfg();
        let cv_at = |peak: u8, floor: u8| {
            let bytes = strobe(&domed(W, H, peak, floor), &dark, 4);
            let b = Burst::from_planar(&bytes, W, H, &c).unwrap();
            evaluate(&b, &whole_face(), &c).get(Check::Falloff).unwrap().worst
        };
        // Same shape, double the gain.
        let bright = cv_at(120, 20);
        let dim = cv_at(60, 10);
        assert!((bright - dim).abs() < 0.02, "gain must not move CV: {bright} vs {dim}");
    }

    /// A flat print fails a floor set anywhere above zero — this is what the
    /// degenerate-flatness floor buys, and the whole of what it buys.
    #[test]
    fn a_flat_presentation_fails_an_enforced_flatness_floor() {
        let dark = blank(W, H, 0);
        let mut c = cfg();
        c.liveness.falloff_cv_min = Threshold::enforced(0.05);

        let print_bytes = strobe(&flat(W, H, 60), &dark, 4);
        let print = Burst::from_planar(&print_bytes, W, H, &c).unwrap();
        let r = evaluate(&print, &whole_face(), &c).rejection().expect("flat must reject");
        assert_eq!(r.check, Check::Falloff);

        let face_bytes = strobe(&domed(W, H, 60, 10), &dark, 4);
        let face = Burst::from_planar(&face_bytes, W, H, &c).unwrap();
        assert!(evaluate(&face, &whole_face(), &c).rejection().is_none());
    }

    /// An advisory check that fails must be reported and must not reject. This is the
    /// mechanism that keeps the verdict honest about what the corpus can actually support.
    #[test]
    fn an_advisory_failure_reports_without_rejecting() {
        let dark = blank(W, H, 0);
        let mut c = cfg();
        c.liveness.falloff_cv_min = Threshold::advisory(0.05);

        let print_bytes = strobe(&flat(W, H, 60), &dark, 4);
        let print = Burst::from_planar(&print_bytes, W, H, &c).unwrap();
        let report = evaluate(&print, &whole_face(), &c);

        assert!(report.rejection().is_none(), "advisory must not reject");
        let failures: Vec<_> = report.advisory_failures().collect();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].check, Check::Falloff);
    }

    /// Conjunction over an empty set is true. If a check is ever evaluated over no pairs
    /// it must produce no result at all, not a pass.
    #[test]
    fn no_pairs_yields_no_results_rather_than_empty_passes() {
        let px = blank(W, H, 60);
        let frame = Frame::new(&px, W, H).unwrap();
        let empty: Vec<Pair<'_>> = Vec::new();
        let r = conjunctive(Check::Response, Threshold::enforced(1.0), Fails::Below, &empty, |_| {
            Some(0.0)
        });
        assert!(r.is_none(), "an empty burst must not manufacture a passing check");
        // And a measurement that never succeeds is likewise not a pass.
        let one = [Pair { on: frame, off: frame }];
        let r = conjunctive(Check::Response, Threshold::enforced(1.0), Fails::Below, &one, |_| None);
        assert!(r.is_none());
    }

    /// A face box that lands entirely outside the frame must yield nothing measurable,
    /// never a silent pass on the checks that were supposed to run over it.
    #[test]
    fn a_box_off_the_frame_produces_no_post_box_results() {
        let dark = blank(W, H, 0);
        let c = cfg();
        let burst_bytes = strobe(&domed(W, H, 60, 10), &dark, 4);
        let burst = Burst::from_planar(&burst_bytes, W, H, &c).unwrap();
        let off_frame = Rect { x: -500.0, y: -500.0, w: 10.0, h: 10.0 };
        assert!(post_box(&burst, &off_frame, &c).results.is_empty());
    }

    #[test]
    fn centre_edge_separates_a_dome_from_a_flat_field() {
        let flat_d = vec![10.0; GRID * GRID];
        assert!((centre_edge_ratio(&flat_d) - 1.0).abs() < 1e-9);

        let mut dome = vec![0.0; GRID * GRID];
        for gy in 0..GRID {
            for gx in 0..GRID {
                let dx = gx as f64 - (GRID as f64 - 1.0) / 2.0;
                let dy = gy as f64 - (GRID as f64 - 1.0) / 2.0;
                dome[gy * GRID + gx] = 40.0 - (dx * dx + dy * dy).sqrt() * 6.0;
            }
        }
        assert!(centre_edge_ratio(&dome) > 1.2);
    }
}

use crate::config::{Config, Enforcement, Threshold};
use crate::frame::{Burst, Pair, PixelRect, Rect};

pub const GRID: usize = 6;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Check {
    DarkFrame,
    Response,
    Falloff,
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fails {
    Above,
    Below,
}

#[derive(Clone, Copy, Debug)]
pub struct CheckResult {
    pub check: Check,
    pub worst: f64,
    pub threshold: f64,
    pub enforcement: Enforcement,
    pub failing_pairs: usize,
}

impl CheckResult {
    pub fn passed(&self) -> bool {
        self.failing_pairs == 0
    }
    pub fn rejects(&self) -> bool {
        !self.passed() && self.enforcement == Enforcement::Enforced
    }
}

#[derive(Clone, Debug, Default)]
pub struct LivenessReport {
    pub results: Vec<CheckResult>,
    pub pairs: usize,
}

impl LivenessReport {
    pub fn rejection(&self) -> Option<CheckResult> {
        self.results.iter().copied().find(CheckResult::rejects)
    }

    pub fn advisory_failures(&self) -> impl Iterator<Item = &CheckResult> {
        self.results.iter().filter(|r| !r.passed() && r.enforcement == Enforcement::Advisory)
    }

    pub fn get(&self, check: Check) -> Option<CheckResult> {
        self.results.iter().copied().find(|r| r.check == check)
    }

    pub fn merge(mut self, other: LivenessReport) -> LivenessReport {
        self.results.extend(other.results);
        self.pairs = self.pairs.max(other.pairs);
        self
    }
}

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

pub fn post_box(burst: &Burst<'_>, face: &Rect, cfg: &Config) -> LivenessReport {
    let pairs = burst.pairs();
    let Some(roi) = face
        .clamped(burst.width(), burst.height())
        .and_then(|r| r.tiled(GRID))
    else {
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
                Some(a.mean_abs_diff_in(&b, &roi) / response)
            })
        {
            results.push(r);
        }
    }

    LivenessReport { results, pairs: pairs.len() }
}

pub fn evaluate(burst: &Burst<'_>, face: &Rect, cfg: &Config) -> LivenessReport {
    pre_box(burst, cfg).merge(post_box(burst, face, cfg))
}

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

pub fn centre_edge_ratio(d_cells: &[f64]) -> f64 {
    if d_cells.len() < GRID * GRID {
        return 0.0;
    }

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
    if e <= 0.0 { 0.0 } else { mean(&centre) / e }
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

    #[test]
    fn one_contaminated_pair_sinks_the_whole_burst() {
        let on = domed(W, H, 60, 10);
        let dark = blank(W, H, 0);
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

        let dark_result = dirty_report.get(Check::DarkFrame).unwrap();
        assert!(
            dark_result.failing_pairs * 2 < dirty_report.pairs,
            "the failing pairs are a minority — which is exactly why voting fails here"
        );
    }

    #[test]
    fn a_bright_contaminant_is_rejected_by_the_check_built_for_it() {
        let on = domed(W, H, 60, 10);
        let dark = blank(W, H, 0);
        let glare = blank(W, H, 30);
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

    #[test]
    fn falloff_is_invariant_to_overall_gain() {
        let dark = blank(W, H, 0);
        let c = cfg();
        let cv_at = |peak: u8, floor: u8| {
            let bytes = strobe(&domed(W, H, peak, floor), &dark, 4);
            let b = Burst::from_planar(&bytes, W, H, &c).unwrap();
            evaluate(&b, &whole_face(), &c).get(Check::Falloff).unwrap().worst
        };
        let bright = cv_at(120, 20);
        let dim = cv_at(60, 10);
        assert!((bright - dim).abs() < 0.02, "gain must not move CV: {bright} vs {dim}");
    }

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

    #[test]
    fn no_pairs_yields_no_results_rather_than_empty_passes() {
        let px = blank(W, H, 60);
        let frame = Frame::new(&px, W, H).unwrap();
        let empty: Vec<Pair<'_>> = Vec::new();
        let r = conjunctive(Check::Response, Threshold::enforced(1.0), Fails::Below, &empty, |_| {
            Some(0.0)
        });
        assert!(r.is_none(), "an empty burst must not manufacture a passing check");
        let one = [Pair { on: frame, off: frame }];
        let r = conjunctive(Check::Response, Threshold::enforced(1.0), Fails::Below, &one, |_| None);
        assert!(r.is_none());
    }

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

    #[test]
    fn centre_edge_fails_closed_on_every_degenerate_input() {
        let dark_border = {
            let mut d = vec![0.0; GRID * GRID];
            for gy in 1..GRID - 1 {
                for gx in 1..GRID - 1 {
                    d[gy * GRID + gx] = 50.0;
                }
            }
            d
        };
        assert_eq!(centre_edge_ratio(&dark_border), 0.0, "a dead border must not read as live");

        let all_negative = {
            let mut d = vec![-1.0; GRID * GRID];
            for gy in GRID / 2 - 1..=GRID / 2 {
                for gx in GRID / 2 - 1..=GRID / 2 {
                    d[gy * GRID + gx] = -10.0;
                }
            }
            d
        };
        assert_eq!(centre_edge_ratio(&all_negative), 0.0, "ON darker than OFF is not a dome");

        assert_eq!(centre_edge_ratio(&[]), 0.0);
        assert_eq!(centre_edge_ratio(&[1.0; GRID * GRID - 1]), 0.0);
    }
}

//! What ambient near-infrared does to the strobe, measured rather than argued.
//!
//! On 2026-08-06 the reference machine stopped classifying as `ir-strobe`
//! ([B18](../../../../wiki/11-bug-log.md)). The emitter had not changed — it fires every
//! frame, perfectly regularly, as it always did. What changed is that daylight adds an
//! ambient pedestal to *both* phases, and [`faciolock_vision::frame::classify`] asks a
//! **ratio** question (`hi > lo * 10`) that saturates against a pedestal.
//!
//! This file measures the same alternation as a **difference**, which a pedestal cannot
//! touch, at two scopes:
//!
//! * **whole frame** — what `classify` uses, so this is the direct like-for-like;
//! * **face box** — because the emitter obeys inverse-square, so a face at 40 cm receives
//!   far more of it than a wall at 3 m. If the face-box response survives daylight even
//!   where the whole-frame response is diluted, the liveness checks can be rescoped rather
//!   than redesigned.
//!
//! **Nothing here asserts a threshold.** It prints the quantities a threshold would have
//! to be chosen from, in the tradition of `spikes/probe`. The one thing it does assert is
//! that the emitter is still detectable at all — because if that ever stops being true,
//! every number below is meaningless and the failure should be loud.
//!
//! Run with `-- --nocapture` to see the table.

mod common;

use faciolock_vision::config::DetectConfig;
use faciolock_vision::{Detector, Frame, PixelRect};

/// Frames discarded while the AGC converges (wiki/10 Probe 3).
const AGC_SETTLE: usize = 5;

/// The alternation, seen as a difference rather than a ratio.
struct Alternation {
    pedestal: f64,
    amplitude: f64,
    #[allow(dead_code)] // reported by the sibling test, kept so the struct mirrors `spikes/probe`
    same_phase: f64,
    snr: f64,
    sign_regularity: f64,
}

/// `means` must be consecutive frames in capture order.
///
/// `same_phase` skips a frame, so both ends have the emitter in the same state and what
/// remains is AGC drift, scene motion and read noise — the floor the emitter has to beat,
/// measured on the capture itself rather than assumed.
fn alternation(means: &[f64]) -> Option<Alternation> {
    if means.len() < 3 {
        return None;
    }
    let steps: Vec<f64> = means.windows(2).map(|w| w[1] - w[0]).collect();
    let amplitude = median(&steps.iter().map(|d| d.abs()).collect::<Vec<_>>());
    let same_phase = median(&means.windows(3).map(|w| (w[2] - w[0]).abs()).collect::<Vec<_>>());
    let flips = steps.windows(2).filter(|w| w[0] * w[1] < 0.0).count();

    Some(Alternation {
        pedestal: means.iter().cloned().fold(f64::INFINITY, f64::min),
        amplitude,
        same_phase,
        // Two true dark frames on this hardware really are identical, so a zero floor is a
        // real reading and not a bug. Reported as infinite rather than divided by.
        snr: if same_phase > 0.0 { amplitude / same_phase } else { f64::INFINITY },
        sign_regularity: flips as f64 / steps.len().saturating_sub(1).max(1) as f64,
    })
}

fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if s.is_empty() { 0.0 } else { s[s.len() / 2] }
}

/// The brightest frame past the AGC window is an emitter-ON frame. Measured, never
/// assumed by index — B5's lesson, and detecting on an OFF frame means detecting on black.
fn brightest_frame(cap: &common::Capture) -> usize {
    let start = AGC_SETTLE.min(cap.frame_count().saturating_sub(1));
    let end = 20.min(cap.frame_count());
    (start..end)
        .max_by(|&a, &b| frame_mean(cap.frame(a)).partial_cmp(&frame_mean(cap.frame(b))).unwrap())
        .unwrap_or(0)
}

fn frame_mean(f: &[u8]) -> f64 {
    f.iter().map(|&p| u64::from(p)).sum::<u64>() as f64 / f.len() as f64
}

#[test]
fn ambient_light_changes_the_ratio_but_not_the_difference() {
    let model = common::yunet();
    if !model.exists() {
        common::skip("YuNet model", &model);
        return;
    }
    let captures = common::captures();
    if captures.is_empty() {
        return;
    }

    let cfg = DetectConfig::default();
    let det = Detector::load(&model, &cfg).expect("model loads");

    eprintln!(
        "\n{:<34} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "capture", "pedestal", "whole:amp", "whole:SNR", "face", "face:amp", "face:SNR"
    );
    eprintln!("{}", "-".repeat(94));

    let mut checked = 0;
    for cap in &captures {
        let frames: Vec<Frame<'_>> = (AGC_SETTLE..cap.frame_count())
            .filter_map(|i| Frame::new(cap.frame(i), cap.width, cap.height))
            .collect();
        if frames.len() < 3 {
            continue;
        }

        let whole: Vec<f64> = frames.iter().map(|f| f.mean()).collect();
        let Some(w) = alternation(&whole) else { continue };

        // Detect on an ON frame; the box is then reused for every frame in the capture,
        // exactly as the pipeline shares one detection across a burst.
        let on = brightest_frame(cap);
        let on_frame = Frame::new(cap.frame(on), cap.width, cap.height).expect("geometry");
        let box_px: Option<PixelRect> = match det.detect_best(&on_frame) {
            Ok(Some(d)) => d.bounds.clamped(cap.width, cap.height),
            _ => None,
        };

        let face = match &box_px {
            Some(r) => {
                let means: Vec<f64> = frames.iter().map(|f| f.mean_in(r)).collect();
                alternation(&means)
            }
            None => None,
        };

        match (&box_px, &face) {
            (Some(_), Some(f)) => eprintln!(
                "{:<34} {:>9.2} {:>9.2} {:>9.1} {:>9} {:>9.2} {:>9.1}",
                truncate(&cap.stem),
                w.pedestal,
                w.amplitude,
                w.snr,
                "yes",
                f.amplitude,
                f.snr
            ),
            _ => eprintln!(
                "{:<34} {:>9.2} {:>9.2} {:>9.1} {:>9} {:>9} {:>9}",
                truncate(&cap.stem),
                w.pedestal,
                w.amplitude,
                w.snr,
                "none",
                "-",
                "-"
            ),
        }

        // The one assertion. Every capture in this corpus was taken with the emitter
        // running, so the alternation must be visible in the difference domain whatever
        // the ambient level does to the ratio. If this fails, either the hardware has
        // genuinely stopped strobing or the statistic below it is wrong — and both are
        // worth failing loudly for, because everything else here is descriptive.
        assert!(
            w.sign_regularity > 0.9,
            "{}: frame means do not alternate (sign regularity {:.3}) — the emitter is not \
             strobing and no difference-domain statistic can rescue that",
            cap.stem,
            w.sign_regularity
        );
        checked += 1;
    }

    eprintln!(
        "\n{checked} captures. `pedestal` is the ambient floor: the variable that separates \
         the nocturnal\ncorpus from the daylight one, and the variable nothing recorded until \
         B18.\n"
    );
}

fn truncate(s: &str) -> String {
    s.chars().take(33).collect()
}

/// Can the dark-frame check be rewritten so it survives ambient light *without* losing the
/// property [B9] bought with blood — that liveness is judged **per pair, conjunctively**?
///
/// `dark_ratio_max` is `mean(OFF)/mean(ON) > 1e-3 -> reject`. It is per-pair and
/// conjunctive, which is correct, and it is a ratio, which is why it rejects 100% of
/// daylight captures (the daylight face capture reads 0.664 against a 0.001 threshold, all
/// 42 pairs failing).
///
/// The obvious replacement — a burst-level SNR floor — would be an *average*, and B9 is
/// precisely the finding that averaging liveness across a burst is a 1-in-36 bypass. So
/// this measures a candidate that stays per-pair: **how far each OFF frame sits from the
/// other OFF frames**, scaled by the emitter's own amplitude.
///
/// A clean OFF frame is ambient alone, so it matches every other OFF frame in the burst.
/// A contaminated one does not. Subtracting a common ambient pedestal changes neither the
/// deviation nor the amplitude, so the statistic is pedestal-free by construction — and a
/// *pulsed* source, which is what B9's attack actually was, lands on some OFF frames and
/// not others, which is exactly what this sees.
///
/// [B9]: ../../../../wiki/11-bug-log.md
#[test]
fn a_per_pair_dark_frame_statistic_that_survives_ambient_light() {
    let captures = common::captures();
    if captures.is_empty() {
        return;
    }

    eprintln!(
        "\n{:<34} {:>10} {:>10} {:>10} {:>10}",
        "capture", "old ratio", "cand A", "cand B", "cand C"
    );
    eprintln!("{}", "-".repeat(78));

    for cap in &captures {
        let means: Vec<f64> = (AGC_SETTLE..cap.frame_count())
            .map(|i| frame_mean(cap.frame(i)))
            .collect();
        if means.len() < 3 {
            continue;
        }

        let (phases, observed) = faciolock_vision::frame::classify(&means);
        if observed != faciolock_vision::Observed::Strobing {
            eprintln!("{:<34} {:>10} {:>10} {:>10} {:>10}", truncate(&cap.stem), "-", "not", "strobing", "-");
            continue;
        }

        let mut on = Vec::new();
        let mut off = Vec::new();
        for (m, p) in means.iter().zip(&phases) {
            match p {
                faciolock_vision::Phase::On => on.push(*m),
                faciolock_vision::Phase::Off => off.push(*m),
            }
        }
        if on.is_empty() || off.is_empty() {
            continue;
        }

        // What the check does today: worst OFF/ON over the burst.
        let old = off.iter().cloned().fold(0.0f64, f64::max)
            / on.iter().cloned().fold(f64::INFINITY, f64::min);

        let amplitude = median(&means.windows(2).map(|w| (w[1] - w[0]).abs()).collect::<Vec<_>>());

        // Candidate A -- each OFF frame against the burst's median OFF.
        // Pedestal-free, but *not* drift-free, and daylight ambient drifts.
        let baseline = median(&off);
        let a = off
            .iter()
            .map(|o| (o - baseline).abs() / amplitude.max(f64::EPSILON))
            .fold(0.0f64, f64::max);

        // Candidate B -- each OFF frame against its immediate OFF neighbours. The same
        // move that fixed `classify`: neighbours cannot drift apart over two frames, so
        // slow ambient change cancels while a pulse landing on one frame does not.
        let b = off
            .windows(3)
            .map(|w| (w[1] - (w[0] + w[2]) / 2.0).abs() / amplitude.max(f64::EPSILON))
            .fold(0.0f64, f64::max);

        // Candidate C -- each pair's own response against the burst's median response.
        // Contaminating an OFF frame lifts it toward its ON frame, so that pair's D
        // collapses whether or not the contaminant is steady.
        let d: Vec<f64> = on.iter().zip(&off).map(|(o, f)| o - f).collect();
        let dm = median(&d);
        let c = d.iter().map(|x| (x - dm).abs() / dm.abs().max(f64::EPSILON)).fold(0.0f64, f64::max);

        eprintln!(
            "{:<34} {:>10.4} {:>10.4} {:>10.4} {:>10.4}",
            truncate(&cap.stem),
            old,
            a,
            b,
            c
        );
    }
    eprintln!(
        "\nA: OFF vs burst median   B: OFF vs OFF neighbours   C: pair response vs median\n\
         All three are per-pair worst-case, so all stay conjunctive as B9 requires.\n"
    );
}

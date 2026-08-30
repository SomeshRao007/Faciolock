mod common;

use faciolock_vision::config::DetectConfig;
use faciolock_vision::{Detector, Frame, PixelRect, Rect};

const AGC_SETTLE: usize = 5;

struct Alternation {
    pedestal: f64,
    amplitude: f64,
    #[allow(dead_code)]
    same_phase: f64,
    snr: f64,
    sign_regularity: f64,
}

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
        snr: if same_phase > 0.0 { amplitude / same_phase } else { f64::INFINITY },
        sign_regularity: flips as f64 / steps.len().saturating_sub(1).max(1) as f64,
    })
}

fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if s.is_empty() { 0.0 } else { s[s.len() / 2] }
}

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

fn falloff_over_placements(
    cap: &common::Capture,
    frames: &[Frame<'_>],
    whole: &[f64],
    size: (f32, f32),
) -> Vec<f64> {
    const STEP: f32 = 12.0;
    const ON_OBJECT: f64 = 8.0;

    let grid = faciolock_vision::liveness::GRID;
    let Some(i) = (1..frames.len().saturating_sub(1))
        .find(|&i| whole[i] > (whole[i - 1] + whole[i + 1]) / 2.0)
    else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut y = 0.0;
    while y + size.1 <= cap.height as f32 {
        let mut x = 0.0;
        while x + size.0 <= cap.width as f32 {
            if let Some(roi) = (Rect { x, y, w: size.0, h: size.1 })
                .clamped(cap.width, cap.height)
                .and_then(|c| c.tiled(grid))
            {
                let d: Vec<f64> = frames[i]
                    .cell_means(&roi, grid)
                    .into_iter()
                    .zip(frames[i + 1].cell_means(&roi, grid))
                    .map(|(on, off)| on - off)
                    .collect();
                let weakest = d.iter().cloned().fold(f64::INFINITY, f64::min);
                let mean = d.iter().sum::<f64>() / d.len() as f64;
                if weakest > ON_OBJECT && mean > 0.0 {
                    let var =
                        d.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / d.len() as f64;
                    out.push(var.sqrt() / mean);
                }
            }
            x += STEP;
        }
        y += STEP;
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out
}

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

        let old = off.iter().cloned().fold(0.0f64, f64::max)
            / on.iter().cloned().fold(f64::INFINITY, f64::min);

        let amplitude = median(&means.windows(2).map(|w| (w[1] - w[0]).abs()).collect::<Vec<_>>());

        let baseline = median(&off);
        let a = off
            .iter()
            .map(|o| (o - baseline).abs() / amplitude.max(f64::EPSILON))
            .fold(0.0f64, f64::max);

        let b = off
            .windows(3)
            .map(|w| (w[1] - (w[0] + w[2]) / 2.0).abs() / amplitude.max(f64::EPSILON))
            .fold(0.0f64, f64::max);

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

#[test]
fn falloff_is_computed_on_a_difference_so_ambient_should_cancel() {
    let model = common::yunet();
    if !model.exists() {
        common::skip("YuNet model", &model);
        return;
    }
    let captures = common::captures();
    if captures.is_empty() {
        return;
    }

    let det = Detector::load(&model, &DetectConfig::default()).expect("model loads");

    eprintln!(
        "\nfalloff CV on the difference image, against the ambient pedestal\n\n\
         {:<34} {:>9} {:>9} {:>9}",
        "capture", "pedestal", "falloffCV", "ctr/edge"
    );
    eprintln!("{}", "-".repeat(64));

    let mut nocturnal: Vec<f64> = Vec::new();
    let mut daylit: Vec<f64> = Vec::new();
    let mut flat: Vec<f64> = Vec::new();
    let mut nonflat: Vec<f64> = Vec::new();
    let mut flat_spread: Vec<(String, f64, f64, f64)> = Vec::new();

    for cap in &captures {
        let frames: Vec<Frame<'_>> = (AGC_SETTLE..cap.frame_count())
            .filter_map(|i| Frame::new(cap.frame(i), cap.width, cap.height))
            .collect();
        if frames.len() < 3 {
            continue;
        }

        let whole: Vec<f64> = frames.iter().map(|f| f.mean()).collect();
        let Some(alt) = alternation(&whole) else { continue };

        let on_idx = brightest_frame(cap);
        let on_frame = Frame::new(cap.frame(on_idx), cap.width, cap.height).expect("geometry");
        let detected = matches!(det.detect_best(&on_frame), Ok(Some(_)));

        const BOX: (f32, f32) = (108.0, 138.0);
        let presented =
            cap.stem.starts_with("flat_") || cap.stem.starts_with("nonflat_");

        if presented && !matches!(det.detect_best(&on_frame), Ok(Some(_))) {
            let cvs = falloff_over_placements(cap, &frames, &whole, BOX);
            if cvs.is_empty() {
                eprintln!("{:<34} {:>9.3}   no on-object placement", truncate(&cap.stem), alt.pedestal);
            } else {
                let (lo, med, hi) = (cvs[0], cvs[cvs.len() / 2], cvs[cvs.len() - 1]);
                eprintln!(
                    "{:<34} {:>9.3}   CV over {} placements: {:.4} / {:.4} / {:.4}  (min/med/max)",
                    truncate(&cap.stem), alt.pedestal, cvs.len(), lo, med, hi
                );
                if cap.stem.starts_with("flat_") {
                    flat_spread.push((cap.stem.clone(), lo, med, hi));
                }
            }
            continue;
        }

        let bounds = match det.detect_best(&on_frame) {
            Ok(Some(d)) => d.bounds,
            _ => {
                eprintln!(
                    "{:<34} {:>9.3} {:>9} {:>9}",
                    truncate(&cap.stem), alt.pedestal, "no face", "-"
                );
                continue;
            }
        };
        let Some(roi) = bounds
            .clamped(cap.width, cap.height)
            .and_then(|r| r.tiled(faciolock_vision::liveness::GRID))
        else {
            continue;
        };

        let mut worst_cv = f64::INFINITY;
        let mut worst_ce = f64::INFINITY;
        for i in 1..frames.len() - 1 {
            let m = whole[i];
            let is_on = m > (whole[i - 1] + whole[i + 1]) / 2.0;
            if !is_on {
                continue;
            }
            let on_cells = frames[i].cell_means(&roi, faciolock_vision::liveness::GRID);
            let off_cells = frames[i + 1].cell_means(&roi, faciolock_vision::liveness::GRID);
            let d: Vec<f64> = on_cells.iter().zip(&off_cells).map(|(a, b)| a - b).collect();
            worst_cv = worst_cv.min(faciolock_vision::liveness::falloff_cv(&d));
            worst_ce = worst_ce.min(faciolock_vision::liveness::centre_edge_ratio(&d));
        }
        if !worst_cv.is_finite() {
            continue;
        }

        eprintln!(
            "{:<34} {:>9.3} {:>9.4} {:>9.3}   {}",
            truncate(&cap.stem), alt.pedestal, worst_cv, worst_ce,
            if detected { "detected box" } else { "FIXED box, no face detected" }
        );

        if cap.stem.starts_with("flat_") {
            flat.push(worst_cv);
        } else if cap.stem.starts_with("nonflat_") {
            nonflat.push(worst_cv);
        }

        if cap.stem.starts_with("live_") {
            if alt.pedestal > 1.0 { daylit.push(worst_cv) } else { nocturnal.push(worst_cv) }
        }
    }

    let range = |v: &[f64]| {
        let lo = v.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = v.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        (lo, hi)
    };

    eprintln!("\n{:<14} {:>3}  {:>9}  {:>9}", "class", "n", "min CV", "max CV");
    if !nocturnal.is_empty() {
        let (lo, hi) = range(&nocturnal);
        eprintln!("{:<14} {:>3}  {:>9.4}  {:>9.4}", "nocturnal", nocturnal.len(), lo, hi);
    }
    if !daylit.is_empty() {
        let (lo, hi) = range(&daylit);
        eprintln!("{:<14} {:>3}  {:>9.4}  {:>9.4}", "daylit", daylit.len(), lo, hi);
    }
    if !flat.is_empty() {
        let (lo, hi) = range(&flat);
        eprintln!("{:<14} {:>3}  {:>9.4}  {:>9.4}", "flat surface", flat.len(), lo, hi);
    }
    if !nonflat.is_empty() {
        let (lo, hi) = range(&nonflat);
        eprintln!(
            "{:<14} {:>3}  {:>9.4}  {:>9.4}   <- 3D objects, NOT falloff's threat model",
            "non-flat", nonflat.len(), lo, hi
        );
    }

    eprintln!(
        "\nIf the nocturnal and daylit rows overlap, falloff is ambient-invariant in\n\
         measurement and not only in derivation, and the ambient gate is refusing captures\n\
         this check can still read."
    );

    let live: Vec<f64> = nocturnal.iter().chain(&daylit).cloned().collect();
    let worst_live = live.iter().cloned().fold(f64::INFINITY, f64::min);

    if !flat_spread.is_empty() {
        eprintln!(
            "\nPlacement-searched rows (no face detected, so the ROI was *chosen*, and the\n\
             choice decides the answer):"
        );
        for (stem, lo, med, hi) in &flat_spread {
            eprintln!("  {:<34} {:.4} .. {:.4}   median {:.4}", truncate(stem), lo, hi, med);
        }
        if flat.is_empty() {
            eprintln!(
                "\nNO VERDICT. Against worst live {worst_live:.4}, that range straddles the live band\n\
                 rather than sitting below it. Choosing the ROI by 'strongest weakest cell' — the\n\
                 obvious on-object rule — selects the most uniformly lit patch and lands near the\n\
                 bottom of the range. That is how this test reported a 2.45x separation on\n\
                 2026-08-09 which was entirely placement bias.\n\n\
                 The real pipeline never chooses. YuNet returns the box, and against a genuine print\n\
                 attack it returns the box *on the printed face*. So the deciding number is falloff\n\
                 over a DETECTED box on a printed face — which needs a printed face. A blank page\n\
                 cannot stand in: it lacks the albedo structure a face photo carries (dark brows,\n\
                 bright forehead), and that structure adds spatial variance, pushing a real print UP\n\
                 toward the live band. This capture is a lower bound on the flat class, not a sample\n\
                 of it."
            );
        }
    }

    if !live.is_empty() && !flat.is_empty() {
        let best_flat = flat.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        eprintln!(
            "\nworst live {worst_live:.4}   best flat {best_flat:.4}   margin {:.4}",
            worst_live - best_flat
        );
        eprintln!(
            "{}",
            if worst_live > best_flat {
                "SEPARATED, on DETECTED boxes — the comparison the pipeline actually makes.\n\
                 Calibration only: it does not show an end-to-end spoof fails, and says nothing\n\
                 about injected IR, which is `dark_ratio_max`'s job."
            } else {
                "OVERLAP. Falloff cannot carry liveness alone and the daylight non-goal stands."
            }
        );
    } else if flat_spread.is_empty() {
        eprintln!(
            "\nNo `flat_*` capture, so the flatness floor is uncalibrated. What is needed is a\n\
             flat presentation of a FACE, so that detection supplies the ROI instead of this\n\
             test choosing one. Until then the 0.05 floor is a degenerate-flatness guard and\n\
             claims nothing about a real presentation attack."
        );
    }
    eprintln!();

    assert!(
        !nocturnal.is_empty() || !daylit.is_empty(),
        "no live capture yielded a face box; falloff cannot be measured at all"
    );
}

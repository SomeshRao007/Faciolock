//! End-to-end: enrol on one recording, verify against the others.
//!
//! This is the Phase 1 exit criterion, and the first test of the assumption the whole
//! project inherits without ever having checked it — **SFace is an RGB model reporting
//! RGB numbers (99.40% LFW), and every frame we have is 8-bit near-infrared.** wiki/05
//! lists "it underperforms on our own IR corpus" as unmeasured. Here it gets measured.
//!
//! What this can settle: cross-capture similarity for one subject across lighting, angle
//! and glasses, which is the false-*reject* side. What it cannot settle: FAR, because the
//! corpus has exactly one subject and an impostor comparison needs a second person. The
//! cosine threshold therefore remains OpenCV's RGB-calibrated 0.363 — paper-derived and
//! provisional, and it must not be described as anything else.
//!
//! Run with `-- --nocapture` for the numbers.

mod common;

use std::time::Instant;

use faciolock_vision::config::MatchConfig;
use faciolock_vision::embed::{self, Embedder, Template};
use faciolock_vision::pipeline::{enroll_template, Verdict};
use faciolock_vision::{Burst, BurstError, Config, Pipeline};

const ENROL_ON: &str = "live_lit_frontal";

fn pipeline() -> Option<(Pipeline, Vec<common::Capture>)> {
    let models = common::models_dir();
    if !common::yunet().exists() {
        common::skip("YuNet model", &common::yunet());
        return None;
    }
    if !common::sface().exists() {
        common::skip("SFace model", &common::sface());
        return None;
    }
    let captures = common::captures();
    if captures.is_empty() {
        return None;
    }
    match Pipeline::load(&models, Config::default()) {
        Ok(p) => Some((p, captures)),
        Err(e) => panic!("models present but the pipeline would not load: {e}"),
    }
}

#[test]
fn enrol_on_one_capture_and_verify_against_the_rest() {
    let Some((pipe, captures)) = pipeline() else { return };

    let enrol_cap = captures.iter().find(|c| c.label == ENROL_ON);
    let Some(enrol_cap) = enrol_cap else {
        eprintln!("no {ENROL_ON} capture to enrol from");
        return;
    };

    let enrol_burst =
        Burst::from_planar(&enrol_cap.bytes, enrol_cap.width, enrol_cap.height, pipe.config())
            .expect("the enrolment capture must form a burst");
    let template = match enroll_template(&pipe, &enrol_burst) {
        Ok(t) => t,
        Err(o) => panic!("enrolment from a live capture failed: {:?}", o.verdict),
    };

    eprintln!(
        "\nenrolled from {ENROL_ON}: {} embeddings, embedder {}\n\
         verifying every capture against it (threshold {:.3}, {}-of-{})\n\
         {:<26} {:>10} {:>8} {:>7} {:>8}  verdict",
        template.len(),
        template.embedder,
        pipe.config().matching.cosine_min,
        pipe.config().matching.agree_k,
        pipe.config().matching.agree_n,
        "capture",
        "best cos",
        "agreeing",
        "probes",
        "ms"
    );

    let mut live_matched = 0;
    let mut live_total = 0;

    for cap in &captures {
        let start = Instant::now();
        let outcome = match Burst::from_planar(&cap.bytes, cap.width, cap.height, pipe.config()) {
            Ok(b) => pipe.verify(&b, &template),
            Err(e) => {
                eprintln!("{:<26} {:>10} — burst rejected: {e:?}", cap.label, "-");
                // A live capture must form a burst *unless* the ambient gate fired, in
                // which case this hardware could not measure it and refusing is the
                // correct, honest outcome (B18). Anything else is a false reject.
                assert!(
                    !cap.label.starts_with("live_")
                        || matches!(e, BurstError::AmbientTooBright { .. }),
                    "{}: a live capture must form a burst unless gated for ambient, got {e:?}",
                    cap.label
                );
                continue;
            }
        };
        let ms = start.elapsed().as_secs_f64() * 1000.0;

        let (cos, agreeing) = match &outcome.verdict {
            Verdict::Match { similarity, agreeing } => (*similarity, *agreeing),
            Verdict::NoMatch { best } => (*best, 0),
            _ => (f64::NAN, 0),
        };
        eprintln!(
            "{:<26} {:>10} {:>8} {:>7} {ms:>8.0}  {:?}",
            cap.label,
            if cos.is_nan() { "-".into() } else { format!("{cos:.4}") },
            agreeing,
            outcome.probes,
            outcome.verdict
        );

        if cap.label.starts_with("live_") {
            live_total += 1;
            if outcome.verdict.is_allow() {
                live_matched += 1;
            }
        }

        // The security-relevant half: nothing that is not a live face may be allowed.
        // `spoof_` is included — a presentation attack must be denied whichever stage
        // denies it. spoof_phone is currently caught by detection rather than by the
        // response check; see wiki/11 B17 for why that distinction matters.
        if cap.label.starts_with("attack_")
            || cap.label.starts_with("null_")
            || cap.label.starts_with("spoof_")
        {
            assert!(
                !outcome.verdict.is_allow(),
                "{}: allowed a non-face capture — {:?}",
                cap.label,
                outcome.verdict
            );
        }
    }

    eprintln!(
        "\n{live_matched}/{live_total} live captures matched the {ENROL_ON} template.\n\
         One subject, so this is the false-*reject* side only. FAR is unmeasurable from\n\
         this corpus and the 0.363 threshold stays paper-derived until a second consenting\n\
         subject exists."
    );
    assert!(live_total > 0, "no live captures to verify");
}

/// The cross-capture similarity matrix, which is what actually says whether an RGB-trained
/// embedder survives contact with near-infrared.
///
/// Reports rather than asserts a threshold: with one subject every off-diagonal cell is a
/// genuine pair, so the numbers bound FRR and say nothing whatever about FAR.
#[test]
fn cross_capture_similarity_matrix() {
    let Some((pipe, captures)) = pipeline() else { return };
    let live: Vec<&common::Capture> =
        captures.iter().filter(|c| c.label.starts_with("live_")).collect();
    if live.len() < 2 {
        eprintln!("need at least two live captures for a matrix");
        return;
    }

    let mut sets = Vec::new();
    for cap in &live {
        let Ok(burst) = Burst::from_planar(&cap.bytes, cap.width, cap.height, pipe.config()) else {
            continue;
        };
        match pipe.enroll(&burst) {
            Ok(e) => sets.push((cap.label.clone(), e)),
            Err(o) => eprintln!("{}: {:?}", cap.label, o.verdict),
        }
    }
    if sets.len() < 2 {
        return;
    }

    eprintln!("\ncross-capture cosine similarity, same subject (all genuine pairs)");
    eprint!("{:<24}", "");
    for (label, _) in &sets {
        eprint!("{:>12}", &label[..label.len().min(12)]);
    }
    eprintln!();

    let mut worst_genuine = f64::INFINITY;
    for (row_label, row) in &sets {
        eprint!("{row_label:<24}");
        for (col_label, col) in &sets {
            let t = Template::new("sface", col.clone()).unwrap();
            let best = row
                .iter()
                .map(|e| t.best_similarity(e))
                .fold(f64::NEG_INFINITY, f64::max);
            eprint!("{best:>12.4}");
            if row_label != col_label {
                worst_genuine = worst_genuine.min(best);
            }
        }
        eprintln!();
    }

    let threshold = MatchConfig::default().cosine_min;
    eprintln!(
        "\nworst genuine cross-capture similarity: {worst_genuine:.4}\n\
         threshold in config: {threshold:.4}  ({})",
        if worst_genuine >= threshold { "clears it" } else { "BELOW IT — every such attempt false-rejects" }
    );
}

/// 2-of-2 against 2-of-3, on the corpus, since the choice was made on argument and the
/// data is free once the embeddings exist.
#[test]
fn false_reject_rate_for_each_aggregation_rule() {
    let Some((pipe, captures)) = pipeline() else { return };
    let live: Vec<&common::Capture> =
        captures.iter().filter(|c| c.label.starts_with("live_")).collect();
    let Some(enrol_cap) = captures.iter().find(|c| c.label == ENROL_ON) else { return };

    let Ok(enrol_burst) =
        Burst::from_planar(&enrol_cap.bytes, enrol_cap.width, enrol_cap.height, pipe.config())
    else {
        return;
    };
    let Ok(template) = enroll_template(&pipe, &enrol_burst) else { return };

    eprintln!("\naggregation rule vs false rejects\n{:<12} {:>10} {:>14}", "rule", "matched", "of live");

    for (k, n) in [(2usize, 2usize), (2, 3), (3, 3)] {
        let cfg = Config {
            matching: MatchConfig { agree_k: k, agree_n: n, ..MatchConfig::default() },
            ..Config::default()
        };
        let staged = Pipeline::new(
            faciolock_vision::Detector::load(&common::yunet(), &cfg.detect).unwrap(),
            Box::new(faciolock_vision::SFace::load(&common::sface()).unwrap())
                as Box<dyn Embedder>,
            cfg,
        );

        let mut matched = 0;
        for cap in &live {
            let Ok(b) = Burst::from_planar(&cap.bytes, cap.width, cap.height, staged.config())
            else {
                continue;
            };
            if staged.verify(&b, &template).verdict.is_allow() {
                matched += 1;
            }
        }
        eprintln!("{:<12} {matched:>10} {:>14}", format!("{k}-of-{n}"), live.len());
    }
    eprintln!(
        "\nThe enrolment capture matches itself trivially, so treat it as {} genuine\n\
         cross-capture trials plus one self-match.",
        live.len().saturating_sub(1)
    );
    let _ = embed::SFACE_DIMS;
}

/// Does the landmark error at a small detector input actually cost anything?
///
/// `detect_corpus` measured the *displacement*: 5-12 px at 192x192 against a 448x448
/// reference, 1.8-6.2 px at 320. A displacement is not yet a cost. This converts it into
/// the only currency that matters — how far the embedding moves — by two routes:
///
/// 1. Embed the same frame twice, once from the 192 landmarks and once from the 448 ones.
///    The cosine between those two embeddings is the price of running the detector small.
/// 2. Perturb the landmarks by a known number of pixels and watch the embedding drift.
///    That gives the shape of the curve, so the number from (1) can be read against it.
///
/// If (1) sits far above the 0.363 threshold and near the flat part of (2), 192 is fine
/// and the 22 ms that 320 costs buys nothing.
#[test]
fn embedding_sensitivity_to_landmark_error() {
    let Some((_pipe, captures)) = pipeline() else { return };
    let live: Vec<&common::Capture> =
        captures.iter().filter(|c| c.label.starts_with("live_")).collect();
    if live.is_empty() {
        return;
    }

    use faciolock_vision::config::DetectConfig;
    use faciolock_vision::detect::LANDMARKS;
    use faciolock_vision::{Detector, Frame, SFace};

    let sface = SFace::load(&common::sface()).unwrap();
    const SIZES: [usize; 3] = [160, 192, 320];
    let smalls: Vec<(usize, Detector)> = SIZES
        .iter()
        .map(|&s| {
            (s, Detector::load(&common::yunet(), &DetectConfig { input_size: s, ..Default::default() }).unwrap())
        })
        .collect();
    let large = Detector::load(&common::yunet(), &DetectConfig { input_size: 448, ..Default::default() }).unwrap();

    eprintln!("\n(1) embedding cosine against the 448 reference, same frame");
    eprintln!("{:<26} {:>8} {:>12}", "capture", "size", "cosine");
    let mut worst_scale_cost = 1.0f64;

    for cap in &live {
        // Brightest frame past AGC settle, i.e. an emitter-ON frame.
        let idx = (5..20.min(cap.frame_count()))
            .max_by(|&a, &b| {
                let m = |i: usize| {
                    let f = cap.frame(i);
                    f.iter().map(|&p| u64::from(p)).sum::<u64>() as f64 / f.len() as f64
                };
                m(a).partial_cmp(&m(b)).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(0);
        let frame = Frame::new(cap.frame(idx), cap.width, cap.height).unwrap();

        let Ok(Some(b)) = large.detect_best(&frame) else { continue };
        let Some(cb) = Pipeline::chip(&frame, &b) else { continue };
        let Ok(eb) = sface.embed(&cb) else { continue };

        for (size, det) in &smalls {
            let (Ok(Some(a)), ) = (det.detect_best(&frame), ) else {
                eprintln!("{:<26} {size:>8} {:>12}", cap.label, "-");
                continue;
            };
            let Some(ca) = Pipeline::chip(&frame, &a) else { continue };
            let Ok(ea) = sface.embed(&ca) else { continue };
            let cos = ea.similarity(&eb);
            if *size == 192 {
                worst_scale_cost = worst_scale_cost.min(cos);
            }
            eprintln!("{:<26} {size:>8} {cos:>12.4}", cap.label);
        }
    }

    eprintln!("\n(2) embedding drift against deliberate landmark perturbation");
    eprintln!("{:<26} {:>8} {:>10}", "capture", "px", "cosine");

    for cap in live.iter().take(2) {
        let idx = (5..20.min(cap.frame_count()))
            .max_by(|&a, &b| {
                let m = |i: usize| {
                    let f = cap.frame(i);
                    f.iter().map(|&p| u64::from(p)).sum::<u64>() as f64 / f.len() as f64
                };
                m(a).partial_cmp(&m(b)).unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(0);
        let frame = Frame::new(cap.frame(idx), cap.width, cap.height).unwrap();
        let Ok(Some(d)) = large.detect_best(&frame) else { continue };
        let Some(base_chip) = Pipeline::chip(&frame, &d) else { continue };
        let Ok(base) = sface.embed(&base_chip) else { continue };

        for px in [0.5f32, 1.0, 2.0, 4.0, 8.0] {
            // Deterministic pseudo-random directions: a fixed LCG, so the number is
            // reproducible run to run. No rand dependency, and no hidden seed.
            let mut seed = 0x2545_F491u32;
            let mut moved = d;
            for i in 0..LANDMARKS {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let ang = (seed >> 8) as f32 / (1u32 << 24) as f32 * std::f32::consts::TAU;
                moved.landmarks[i].0 += px * ang.cos();
                moved.landmarks[i].1 += px * ang.sin();
            }
            let Some(chip) = Pipeline::chip(&frame, &moved) else { continue };
            let Ok(e) = sface.embed(&chip) else { continue };
            eprintln!("{:<26} {px:>8.1} {:>10.4}", cap.label, base.similarity(&e));
        }
    }

    eprintln!(
        "\nworst 192-vs-448 embedding cosine: {worst_scale_cost:.4}\n\
         Read it against curve (2): if it sits where a small perturbation sits, the\n\
         detector is already fine at 192 and 320 costs 22 ms for nothing."
    );
}

/// Per-stage warm-path latency, measured rather than budgeted.
///
/// wiki/06's table has carried estimates for everything except the two inference figures.
/// This measures the stages this crate owns; the frame burst belongs to Phase 2 and is
/// still the weakest number in that table.
#[test]
fn warm_path_stage_latency() {
    let Some((pipe, captures)) = pipeline() else { return };
    let Some(cap) = captures.iter().find(|c| c.label == ENROL_ON) else { return };

    use faciolock_vision::{Frame, SFace};
    use faciolock_vision::embed::Embedder;

    let sface = SFace::load(&common::sface()).unwrap();
    let idx = (5..20.min(cap.frame_count()))
        .max_by(|&a, &b| {
            let m = |i: usize| {
                let f = cap.frame(i);
                f.iter().map(|&p| u64::from(p)).sum::<u64>() as f64 / f.len() as f64
            };
            m(a).partial_cmp(&m(b)).unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(0);
    let frame = Frame::new(cap.frame(idx), cap.width, cap.height).unwrap();

    // 50 measured iterations after 10 warm-up, and the *median* is what is reported.
    //
    // An earlier version of this test used 12 iterations with no warm-up and reported
    // SFace at 39 ms against the 85 ms in wiki/08. The spike binary, re-run on this
    // machine in the same session, reads `min 38.98 / p50 76.19 / p95 76.68` — bimodal,
    // because the CPU boosts for the first handful of inferences and then settles. A
    // 12-sample median lands inside the boost window and reports a number no real
    // authentication will ever see. **A benchmark that does not outlast the boost clock
    // is measuring the clock, not the code** — the same shape of error as
    // [B11](../../../../wiki/11-bug-log.md), where a figure was trusted because it was
    // convenient rather than because it was controlled.
    const WARMUP: usize = 10;
    const N: usize = 50;
    let p50 = |mut v: Vec<f64>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    };

    // Burst construction over a realistic 3-pair capture, not the 42-pair recording.
    let stride = cap.width * cap.height;
    let short = &cap.bytes[..stride * 11];
    for _ in 0..WARMUP {
        let _ = Burst::from_planar(short, cap.width, cap.height, pipe.config());
    }
    let burst_ms = p50(
        (0..N)
            .map(|_| {
                let t = Instant::now();
                let _ = Burst::from_planar(short, cap.width, cap.height, pipe.config());
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect(),
    );

    let burst = Burst::from_planar(short, cap.width, cap.height, pipe.config()).unwrap();
    for _ in 0..WARMUP {
        let _ = faciolock_vision::liveness::pre_box(&burst, pipe.config());
    }
    let live_ms = p50(
        (0..N)
            .map(|_| {
                let t = Instant::now();
                let _ = faciolock_vision::liveness::pre_box(&burst, pipe.config());
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect(),
    );

    let det = faciolock_vision::Detector::load(&common::yunet(), &pipe.config().detect).unwrap();
    for _ in 0..WARMUP {
        let _ = det.detect_best(&frame);
    }
    let detect_ms = p50(
        (0..N)
            .map(|_| {
                let t = Instant::now();
                let _ = det.detect_best(&frame);
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect(),
    );

    let d = det.detect_best(&frame).unwrap().unwrap();
    for _ in 0..WARMUP {
        let _ = Pipeline::chip(&frame, &d);
    }
    let align_ms = p50(
        (0..N)
            .map(|_| {
                let t = Instant::now();
                let _ = Pipeline::chip(&frame, &d);
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect(),
    );

    let chip = Pipeline::chip(&frame, &d).unwrap();
    for _ in 0..WARMUP {
        let _ = sface.embed(&chip);
    }
    let embed_ms = p50(
        (0..N)
            .map(|_| {
                let t = Instant::now();
                let _ = sface.embed(&chip);
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect(),
    );

    let n = pipe.config().matching.agree_n as f64;
    let rows: [(String, f64); 5] = [
        ("burst build + pairing (3 pairs)".into(), burst_ms),
        ("liveness, pre-box".into(), live_ms),
        ("YuNet detect (x1)".into(), detect_ms),
        (format!("align (x{n:.0})"), align_ms * n),
        (format!("SFace embed (x{n:.0})"), embed_ms * n),
    ];

    let size = pipe.config().detect.input_size;
    eprintln!("\nwarm-path stages, p50 of {N}, single-threaded, detector @{size}x{size}");
    eprintln!("{:<34} {:>9}", "stage", "ms");
    for (name, ms) in &rows {
        eprintln!("{name:<34} {ms:>9.1}");
    }
    eprintln!("{:<34} {:>9.1}", "== crate total", rows.iter().map(|(_, m)| m).sum::<f64>());
    eprintln!(
        "Excludes the frame burst itself (~130 ms estimated, sensor-bound, Phase 2 owns it)\n\
         and the socket round trip."
    );
}

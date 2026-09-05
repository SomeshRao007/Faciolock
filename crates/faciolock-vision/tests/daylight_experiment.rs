mod common;

use std::cmp::Ordering;

use faciolock_vision::config::{LivenessConfig, Threshold};
use faciolock_vision::embed::Embedder;
use faciolock_vision::pipeline::{enroll_template, Verdict};
use faciolock_vision::{
    Burst, Check, Config, Detection, Detector, Frame, LivenessReport, Pipeline, PixelRect, SFace,
    Template,
};

const ENROL_DARK: &str = "live_lit_frontal";
const ENROL_BRIGHT: &str = "live_bright_control";
const SAT: u8 = 250;

fn base() -> Config {
    Config::default()
}

fn relaxed() -> Config {
    let d = Config::default();
    Config {
        ambient_floor_max: 1e9,
        liveness: LivenessConfig { dark_ratio_max: Threshold::advisory(1e-3), ..d.liveness },
        ..d
    }
}

fn frame_mean(f: &[u8]) -> f64 {
    f.iter().map(|&p| u64::from(p)).sum::<u64>() as f64 / f.len() as f64
}

fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    if s.is_empty() { 0.0 } else { s[s.len() / 2] }
}

fn usable(cap: &common::Capture, settle: usize) -> Vec<usize> {
    (settle.min(cap.frame_count())..cap.frame_count()).collect()
}

fn usable_means(cap: &common::Capture, settle: usize) -> Vec<f64> {
    usable(cap, settle).iter().map(|&i| frame_mean(cap.frame(i))).collect()
}

fn pedestal(cap: &common::Capture, settle: usize) -> f64 {
    usable_means(cap, settle).iter().cloned().fold(f64::INFINITY, f64::min)
}

fn brightest_usable(cap: &common::Capture, settle: usize) -> usize {
    usable(cap, settle)
        .into_iter()
        .max_by(|&a, &b| {
            frame_mean(cap.frame(a)).partial_cmp(&frame_mean(cap.frame(b))).unwrap_or(Ordering::Equal)
        })
        .unwrap_or(0)
}

fn short(s: &str) -> String {
    s.chars().take(33).collect()
}

fn class_of(label: &str) -> String {
    label.split('_').next().unwrap_or("?").to_string()
}

fn num(v: f64, dp: usize) -> String {
    if v.is_nan() {
        "nan".into()
    } else if v.is_infinite() {
        if v > 0.0 { "inf".into() } else { "-inf".into() }
    } else {
        format!("{v:.dp$}")
    }
}

fn detect_on(det: &Detector, cap: &common::Capture, idx: usize) -> Option<Detection> {
    let f = Frame::new(cap.frame(idx), cap.width, cap.height)?;
    det.detect_best(&f).ok().flatten()
}

fn detect_in_burst(det: &Detector, burst: &Burst<'_>, take: usize) -> Option<Detection> {
    for pair in burst.pairs().iter().take(take) {
        if let Ok(Some(d)) = det.detect_best(&pair.on) {
            return Some(d);
        }
    }
    None
}

fn sat_fraction(px: &[u8]) -> f64 {
    px.iter().filter(|&&p| p >= SAT).count() as f64 / px.len() as f64
}

fn sat_fraction_in(cap: &common::Capture, idx: usize, r: &PixelRect) -> f64 {
    let f = cap.frame(idx);
    let mut hit = 0usize;
    for y in r.y0..r.y0 + r.h {
        let start = y * cap.width + r.x0;
        hit += f[start..start + r.w].iter().filter(|&&p| p >= SAT).count();
    }
    hit as f64 / (r.w * r.h) as f64
}

/// Detect + align + embed on the single brightest usable frame, bypassing burst
/// formation and every liveness check. Exists so a cosine is available even for
/// captures the tier/ambient gates refuse to turn into a burst.
fn raw_embedding(
    det: &Detector,
    sface: &SFace,
    cap: &common::Capture,
    idx: usize,
) -> Option<faciolock_vision::Embedding> {
    let f = Frame::new(cap.frame(idx), cap.width, cap.height)?;
    let d = det.detect_best(&f).ok().flatten()?;
    let chip = Pipeline::chip(&f, &d)?;
    sface.embed(&chip).ok()
}

fn manual_template(
    det: &Detector,
    sface: &SFace,
    cap: &common::Capture,
    settle: usize,
    n: usize,
) -> Option<Template> {
    let mut idx = usable(cap, settle);
    idx.sort_by(|&a, &b| {
        frame_mean(cap.frame(b)).partial_cmp(&frame_mean(cap.frame(a))).unwrap_or(Ordering::Equal)
    });
    let mut embs = Vec::new();
    for &i in idx.iter().take(n) {
        if let Some(e) = raw_embedding(det, sface, cap, i) {
            embs.push(e);
        }
    }
    Template::new("sface", embs)
}

fn check_cell(report: &LivenessReport, c: Check) -> (String, String) {
    match report.get(c) {
        Some(r) => (num(r.worst, 4), format!("{}/{}", r.failing_pairs, report.pairs)),
        None => ("-".into(), "-".into()),
    }
}

fn enforced_flags(report: &LivenessReport) -> String {
    let mut s = String::new();
    for (c, tag) in [
        (Check::DarkFrame, 'D'),
        (Check::Response, 'R'),
        (Check::Falloff, 'F'),
        (Check::Motion, 'M'),
    ] {
        if report.get(c).is_some_and(|r| r.failing_pairs > 0) {
            s.push(tag);
        }
    }
    if s.is_empty() { "none".into() } else { s }
}

struct Enrolled {
    stem: String,
    how: String,
    template: Template,
}

fn enrol_from(
    pipe: &Pipeline,
    det: &Detector,
    sface: &SFace,
    cap: &common::Capture,
    settle: usize,
) -> Option<Enrolled> {
    match Burst::from_planar(&cap.bytes, cap.width, cap.height, pipe.config()) {
        Ok(b) => match enroll_template(pipe, &b) {
            Ok(t) => {
                return Some(Enrolled {
                    stem: cap.stem.clone(),
                    how: format!("pipeline enroll ({} embeddings)", t.len()),
                    template: t,
                })
            }
            Err(o) => eprintln!("  {} pipeline enrolment refused: {:?}", short(&cap.stem), o.verdict),
        },
        Err(e) => eprintln!("  {} burst refused for enrolment: {e:?}", short(&cap.stem)),
    }
    let t = manual_template(det, sface, cap, settle, 3)?;
    Some(Enrolled {
        stem: cap.stem.clone(),
        how: format!("MANUAL fallback, brightest frames ({} embeddings)", t.len()),
        template: t,
    })
}

fn verify_table(
    title: &str,
    pipe: &Pipeline,
    det: &Detector,
    sface: &SFace,
    captures: &[common::Capture],
    e: &Enrolled,
    settle: usize,
) -> Vec<(String, f64, bool)> {
    eprintln!(
        "\n{title}\n  enrolled from : {}\n  via           : {}\n  threshold     : {:.3}, {}-of-{}\n",
        e.stem,
        e.how,
        pipe.config().matching.cosine_min,
        pipe.config().matching.agree_k,
        pipe.config().matching.agree_n
    );
    eprintln!(
        "{:<34} {:>9} {:>8} {:>6} {:>9}  {}",
        "capture", "best cos", "agreeing", "probes", "raw cos", "verdict"
    );
    eprintln!("(agreeing '<k' = a NoMatch: fewer than agree_k probes cleared the threshold, even\n\
               when best cos is above it. The exact count is not carried by the verdict.)");
    eprintln!("{}", "-".repeat(110));

    let mut rows = Vec::new();
    for cap in captures {
        let raw = raw_embedding(det, sface, cap, brightest_usable(cap, settle))
            .map(|p| e.template.best_similarity(&p));

        let (cos, agreeing, probes, verdict, allow) =
            match Burst::from_planar(&cap.bytes, cap.width, cap.height, pipe.config()) {
                Ok(b) => {
                    let o = pipe.verify(&b, &e.template);
                    // Verdict::NoMatch carries only `best`, never the agreeing count, so it
                    // is reported as "-" rather than invented as 0.
                    let (c, a) = match &o.verdict {
                        Verdict::Match { similarity, agreeing } => {
                            (*similarity, agreeing.to_string())
                        }
                        Verdict::NoMatch { best } => (*best, "<k".to_string()),
                        _ => (f64::NAN, "-".to_string()),
                    };
                    let allow = o.verdict.is_allow();
                    (c, a, o.probes, format!("{:?}", o.verdict), allow)
                }
                Err(err) => {
                    (f64::NAN, "-".to_string(), 0, format!("burst refused: {err:?}"), false)
                }
            };

        eprintln!(
            "{:<34} {:>9} {:>8} {:>6} {:>9}  {}",
            short(&cap.stem),
            if cos.is_nan() { "-".into() } else { num(cos, 4) },
            agreeing,
            probes,
            raw.map(|v| num(v, 4)).unwrap_or_else(|| "-".into()),
            verdict
        );
        rows.push((cap.label.clone(), raw.unwrap_or(f64::NAN), allow));
    }
    rows
}

#[test]
fn daylight_experiment() {
    if !common::yunet().exists() {
        common::skip("YuNet model", &common::yunet());
        return;
    }
    if !common::sface().exists() {
        common::skip("SFace model", &common::sface());
        return;
    }
    let captures = common::captures();
    if captures.is_empty() {
        eprintln!("no captures on disk — nothing to measure");
        return;
    }

    let b = base();
    let r = relaxed();
    let settle = r.agc_settle_frames;

    let det = Detector::load(&common::yunet(), &r.detect).expect("YuNet loads");
    let sface = SFace::load(&common::sface()).expect("SFace loads");
    let pipe_relaxed =
        Pipeline::load(&common::models_dir(), r).expect("models present but pipeline would not load");

    eprintln!(
        "\n================================================================================\n\
         DAYLIGHT EXPERIMENT — measurement only, this test asserts nothing\n\
         BASE    : ambient_floor_max={:.2}  dark_ratio_max={:?}({:.0e})\n\
         RELAXED : ambient_floor_max={:.0e}  dark_ratio_max={:?}({:.0e})\n\
         both    : tier={:?} agc_settle={} min_pairs={} falloff_cv_min={:?}({}) \
         response_min={:?}({}) motion_max={:?}({})\n\
         captures: {}\n\
         ================================================================================",
        b.ambient_floor_max,
        b.liveness.dark_ratio_max.enforcement,
        b.liveness.dark_ratio_max.value,
        r.ambient_floor_max,
        r.liveness.dark_ratio_max.enforcement,
        r.liveness.dark_ratio_max.value,
        r.tier,
        r.agc_settle_frames,
        r.min_pairs,
        r.liveness.falloff_cv_min.enforcement,
        r.liveness.falloff_cv_min.value,
        r.liveness.response_min.enforcement,
        r.liveness.response_min.value,
        r.liveness.motion_max.enforcement,
        r.liveness.motion_max.value,
        captures.len()
    );

    // ---------------------------------------------------------------- TABLE 1
    eprintln!(
        "\n\nTABLE 1 — burst formation, BASE vs RELAXED\n\
         pedestal = min frame mean over usable frames (0-255), AGC settle frames dropped.\n"
    );
    eprintln!(
        "{:<40} {:>9}  {:<38}  {:<38}",
        "capture", "pedestal", "BASE", "RELAXED"
    );
    eprintln!("{}", "-".repeat(132));

    let mut base_ok = Vec::new();
    let mut relaxed_ok = Vec::new();
    for cap in &captures {
        let describe = |cfg: &Config| -> (String, bool) {
            match Burst::from_planar(&cap.bytes, cap.width, cap.height, cfg) {
                Ok(bu) => (format!("{:?}, {} pairs", bu.observed(), bu.pairs().len()), true),
                Err(e) => (format!("{e:?}"), false),
            }
        };
        let (bs, bok) = describe(&b);
        let (rs, rok) = describe(&r);
        if bok {
            base_ok.push(cap.stem.clone());
        }
        if rok {
            relaxed_ok.push(cap.stem.clone());
        }
        eprintln!(
            "{:<40} {:>9.3}  {:<38}  {:<38}",
            cap.stem,
            pedestal(cap, settle),
            bs,
            rs
        );
    }
    eprintln!(
        "\nbursts formed: BASE {}/{}, RELAXED {}/{}",
        base_ok.len(),
        captures.len(),
        relaxed_ok.len(),
        captures.len()
    );

    // ---------------------------------------------------------------- TABLE 2
    eprintln!(
        "\n\nTABLE 2 — liveness under RELAXED (captures that form a burst)\n\
         worst = worst value over pairs; fail = failing_pairs/pairs.\n\
         'if-enforced' lists the checks with failing_pairs > 0, i.e. those that WOULD reject\n\
         this capture were they enforced (today only DarkFrame+Falloff are enforced in BASE).\n"
    );
    eprintln!(
        "{:<34} {:>4} {:>5} {:>10} {:>6} {:>9} {:>6} {:>8} {:>6} {:>8} {:>6}  {}",
        "capture",
        "face",
        "pairs",
        "dark",
        "fail",
        "resp",
        "fail",
        "falloff",
        "fail",
        "motion",
        "fail",
        "if-enforced"
    );
    eprintln!("{}", "-".repeat(140));

    let mut relaxed_burst_no_face = Vec::new();
    for cap in &captures {
        let Ok(burst) = Burst::from_planar(&cap.bytes, cap.width, cap.height, &r) else { continue };
        let mut report = faciolock_vision::liveness::pre_box(&burst, &r);
        let face = detect_in_burst(&det, &burst, r.matching.agree_n);
        if let Some(d) = face {
            report = report.merge(faciolock_vision::liveness::post_box(&burst, &d.bounds, &r));
        } else if !base_ok.contains(&cap.stem) {
            relaxed_burst_no_face.push(cap.stem.clone());
        }
        let (dw, df) = check_cell(&report, Check::DarkFrame);
        let (rw, rf) = check_cell(&report, Check::Response);
        let (fw, ff) = check_cell(&report, Check::Falloff);
        let (mw, mf) = check_cell(&report, Check::Motion);
        eprintln!(
            "{:<34} {:>4} {:>5} {:>10} {:>6} {:>9} {:>6} {:>8} {:>6} {:>8} {:>6}  {}",
            short(&cap.stem),
            if face.is_some() { "yes" } else { "NO" },
            burst.pairs().len(),
            dw,
            df,
            rw,
            rf,
            fw,
            ff,
            mw,
            mf,
            enforced_flags(&report)
        );
    }

    // ---------------------------------------------------------------- TABLE 3
    eprintln!(
        "\n\nTABLE 3 — saturation headroom on the brightest usable frame\n\
         Computed from raw frames (AGC settle dropped), NOT from the burst, so captures that\n\
         form no burst are still covered. sat = fraction of pixels >= {SAT}.\n"
    );
    eprintln!(
        "{:<34} {:>6} {:>10} {:>10} {:>10} {:>10}",
        "capture", "frame", "sat whole", "sat face", "mean whole", "mean face"
    );
    eprintln!("{}", "-".repeat(84));

    for cap in &captures {
        let idx = brightest_usable(cap, settle);
        let whole = cap.frame(idx);
        let (sat_face, mean_face) = match detect_on(&det, cap, idx)
            .and_then(|d| d.bounds.clamped(cap.width, cap.height))
        {
            Some(px) => {
                let f = Frame::new(whole, cap.width, cap.height).expect("geometry");
                (num(sat_fraction_in(cap, idx, &px), 5), num(f.mean_in(&px), 2))
            }
            None => ("-".into(), "-".into()),
        };
        eprintln!(
            "{:<34} {idx:>6} {:>10} {:>10} {:>10} {:>10}",
            short(&cap.stem),
            num(sat_fraction(whole), 5),
            sat_face,
            num(frame_mean(whole), 2),
            mean_face
        );
    }

    // ---------------------------------------------------------------- TABLE 4
    eprintln!(
        "\n\nTABLE 4 — cross-illumination matching under RELAXED\n\
         'best cos'/'agreeing'/'probes'/'verdict' come from Pipeline::verify and are blank when\n\
         an earlier stage (burst, liveness) short-circuits. 'raw cos' bypasses burst formation\n\
         and every liveness check entirely: brightest usable frame -> YuNet -> align -> SFace ->\n\
         best cosine against the template. raw cos answers the identity question on its own."
    );

    let dark_cap = captures.iter().find(|c| c.label == ENROL_DARK);
    let mut run1: Vec<(String, f64, bool)> = Vec::new();
    match dark_cap {
        None => eprintln!("\nno {ENROL_DARK} capture to enrol from — run 1 skipped"),
        Some(cap) => {
            eprintln!("\n--- RUN 1: template from the NOCTURNAL capture ---");
            match enrol_from(&pipe_relaxed, &det, &sface, cap, settle) {
                None => eprintln!("  run 1 enrolment failed entirely (no embeddings)"),
                Some(e) => {
                    run1 = verify_table(
                        "RUN 1 — dark-enrolled template vs every capture",
                        &pipe_relaxed,
                        &det,
                        &sface,
                        &captures,
                        &e,
                        settle,
                    );
                }
            }
        }
    }

    eprintln!("\n--- RUN 2: template from a HIGH-PEDESTAL capture ---");
    let mut bright: Vec<&common::Capture> =
        captures.iter().filter(|c| c.label == ENROL_BRIGHT).collect();
    bright.sort_by(|x, y| {
        pedestal(y, settle).partial_cmp(&pedestal(x, settle)).unwrap_or(Ordering::Equal)
    });
    for c in &bright {
        eprintln!("  candidate {} pedestal {:.2}", short(&c.stem), pedestal(c, settle));
    }
    let mut run2: Vec<(String, f64, bool)> = Vec::new();
    match bright.first() {
        None => eprintln!("  no {ENROL_BRIGHT} capture to enrol from — run 2 skipped"),
        Some(cap) => match enrol_from(&pipe_relaxed, &det, &sface, cap, settle) {
            None => eprintln!("  run 2 enrolment failed entirely (no embeddings)"),
            Some(e) => {
                run2 = verify_table(
                    "RUN 2 — bright-enrolled template vs every capture",
                    &pipe_relaxed,
                    &det,
                    &sface,
                    &captures,
                    &e,
                    settle,
                );
            }
        },
    }

    let headline = ["live_daylight_frontal", "live_daylight_half", "live_room_lit", "live_bright_control"];
    for (name, rows) in [("RUN 1 (dark-enrolled)", &run1), ("RUN 2 (bright-enrolled)", &run2)] {
        if rows.is_empty() {
            continue;
        }
        eprintln!("\n{name} — headline raw cosines vs threshold {:.3}:", r.matching.cosine_min);
        for want in headline {
            for (label, cos, _) in rows.iter().filter(|(l, _, _)| l == want) {
                eprintln!(
                    "  {:<24} raw cos {:>8}   {}",
                    label,
                    num(*cos, 4),
                    if cos.is_nan() {
                        "no face / no embedding"
                    } else if *cos >= r.matching.cosine_min {
                        "ABOVE threshold"
                    } else {
                        "BELOW threshold"
                    }
                );
            }
        }
        let allowed: Vec<&String> = rows
            .iter()
            .filter(|(l, _, a)| {
                *a && (l.starts_with("attack_")
                    || l.starts_with("spoof_")
                    || l.starts_with("null_")
                    || l.starts_with("flat_")
                    || l.starts_with("nonflat_"))
            })
            .map(|(l, _, _)| l)
            .collect();
        eprintln!(
            "  non-live captures with is_allow()==true under RELAXED: {}",
            if allowed.is_empty() { "NONE".to_string() } else { format!("{allowed:?}") }
        );
    }

    // ---------------------------------------------------------------- TABLE 5
    eprintln!(
        "\n\nTABLE 5 — candidate replacement statistic B, under RELAXED\n\
         B = max over pairs of |off_mean - median(off_means)| / median(|frame-to-frame step|)\n\
         off_means come from the burst's pairs; the step median is recomputed from the usable\n\
         raw frame means exactly as frame::classify does. 'old ratio' is the worst per-pair\n\
         off_mean/on_mean, the statistic dark_ratio_max enforces today (threshold 1e-3).\n"
    );
    eprintln!(
        "{:<34} {:>6} {:>10} {:>12} {:>12} {:>12}",
        "capture", "pairs", "amplitude", "median off", "B (worst)", "old ratio"
    );
    eprintln!("{}", "-".repeat(90));

    let mut by_class: Vec<(String, String, f64, f64)> = Vec::new();
    for cap in &captures {
        let Ok(burst) = Burst::from_planar(&cap.bytes, cap.width, cap.height, &r) else { continue };
        let means = usable_means(cap, settle);
        let steps: Vec<f64> = means.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        let amplitude = median(&steps);
        let offs: Vec<f64> = burst.pairs().iter().map(|p| p.off.mean()).collect();
        let med_off = median(&offs);

        let mut worst_b = f64::NEG_INFINITY;
        let mut worst_ratio = f64::NEG_INFINITY;
        for p in burst.pairs() {
            let off = p.off.mean();
            let on = p.on.mean();
            let bstat =
                if amplitude > 0.0 { (off - med_off).abs() / amplitude } else { f64::INFINITY };
            worst_b = worst_b.max(bstat);
            worst_ratio = worst_ratio.max(if on > 0.0 { off / on } else { f64::INFINITY });
        }
        eprintln!(
            "{:<34} {:>6} {:>10} {:>12} {:>12} {:>12}",
            short(&cap.stem),
            burst.pairs().len(),
            num(amplitude, 3),
            num(med_off, 3),
            num(worst_b, 4),
            num(worst_ratio, 6)
        );
        by_class.push((class_of(&cap.label), cap.stem.clone(), worst_b, worst_ratio));
    }

    eprintln!("\nper-class range (min .. max over the captures above)");
    eprintln!("{:<12} {:>5} {:>22} {:>26}", "class", "n", "B", "old ratio");
    eprintln!("{}", "-".repeat(68));
    let mut classes: Vec<String> = by_class.iter().map(|(c, _, _, _)| c.clone()).collect();
    classes.sort();
    classes.dedup();
    for c in &classes {
        let rows: Vec<&(String, String, f64, f64)> =
            by_class.iter().filter(|(cl, _, _, _)| cl == c).collect();
        let bmin = rows.iter().map(|x| x.2).fold(f64::INFINITY, f64::min);
        let bmax = rows.iter().map(|x| x.2).fold(f64::NEG_INFINITY, f64::max);
        let rmin = rows.iter().map(|x| x.3).fold(f64::INFINITY, f64::min);
        let rmax = rows.iter().map(|x| x.3).fold(f64::NEG_INFINITY, f64::max);
        eprintln!(
            "{:<12} {:>5} {:>10} .. {:>8} {:>12} .. {:>10}",
            c,
            rows.len(),
            num(bmin, 4),
            num(bmax, 4),
            num(rmin, 6),
            num(rmax, 6)
        );
    }

    // ------------------------------------------------------------- FOOTNOTES
    eprintln!(
        "\n\nCaptures RELAXED admits that BASE rejected, where the face detector then FAILS:\n  {}",
        if relaxed_burst_no_face.is_empty() {
            "NONE".to_string()
        } else {
            relaxed_burst_no_face.join("\n  ")
        }
    );
    eprintln!("\nend of measurement — nothing above was asserted.\n");
}

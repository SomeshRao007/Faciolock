mod common;

use faciolock_vision::config::DetectConfig;
use faciolock_vision::liveness::{self, Check};
use faciolock_vision::{Burst, Config, Detector, Frame, Rect};

fn brightest_frame(cap: &common::Capture) -> usize {
    let start = 5.min(cap.frame_count().saturating_sub(1));
    let end = 20.min(cap.frame_count());
    (start..end)
        .max_by(|&a, &b| {
            let mean = |i: usize| {
                let f = cap.frame(i);
                f.iter().map(|&p| u64::from(p)).sum::<u64>() as f64 / f.len() as f64
            };
            mean(a).partial_cmp(&mean(b)).unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(0)
}

struct Replay {
    label: String,
    pairs: usize,
    face: Option<Rect>,
    report: Option<liveness::LivenessReport>,
    burst_error: Option<String>,
}

fn replay_corpus() -> Vec<Replay> {
    let path = common::yunet();
    if !path.exists() {
        common::skip("YuNet model", &path);
        return Vec::new();
    }
    let captures = common::captures();
    if captures.is_empty() {
        return Vec::new();
    }
    let cfg = Config::default();
    let det = Detector::load(&path, &DetectConfig::default()).expect("model loads");

    let mut out = Vec::new();
    for cap in &captures {
        let burst = match Burst::from_planar(&cap.bytes, cap.width, cap.height, &cfg) {
            Ok(b) => b,
            Err(e) => {
                out.push(Replay {
                    label: cap.label.clone(),
                    pairs: 0,
                    face: None,
                    report: None,
                    burst_error: Some(format!("{e:?}")),
                });
                continue;
            }
        };

        let px = cap.frame(brightest_frame(cap));
        let frame = Frame::new(px, cap.width, cap.height).expect("geometry matches");
        let face = det.detect_best(&frame).ok().flatten().map(|d| d.bounds);

        let report = match face {
            Some(f) => liveness::evaluate(&burst, &f, &cfg),
            None => liveness::pre_box(&burst, &cfg),
        };

        out.push(Replay {
            label: cap.label.clone(),
            pairs: burst.pairs().len(),
            face,
            report: Some(report),
            burst_error: None,
        });
    }
    out
}

#[test]
fn corpus_liveness_report() {
    let replays = replay_corpus();
    if replays.is_empty() {
        return;
    }

    eprintln!(
        "\ncorpus liveness, ROI = YuNet box (B7 closed)\n\
         {:<26} {:>6} {:>6} {:>11} {:>10} {:>9} {:>9}  verdict",
        "capture", "pairs", "face", "dark(worst)", "D(worst)", "falloff", "motion"
    );

    for r in &replays {
        if let Some(e) = &r.burst_error {
            eprintln!("{:<26} {:>6} {:>6} {e}", r.label, "-", "-");
            continue;
        }
        let rep = r.report.as_ref().unwrap();
        let val = |c: Check| {
            rep.get(c).map(|x| format!("{:.4}", x.worst)).unwrap_or_else(|| "-".into())
        };
        let dark = rep
            .get(Check::DarkFrame)
            .map(|x| format!("{:.2e}", x.worst))
            .unwrap_or_else(|| "-".into());
        let verdict = match rep.rejection() {
            Some(c) => format!("REJECT ({})", c.check.name()),
            None => {
                let advisory: Vec<&str> =
                    rep.advisory_failures().map(|c| c.check.name()).collect();
                if advisory.is_empty() {
                    "allow".into()
                } else {
                    format!("allow (advisory fail: {})", advisory.join(","))
                }
            }
        };
        eprintln!(
            "{:<26} {:>6} {:>6} {dark:>11} {:>10} {:>9} {:>9}  {verdict}",
            r.label,
            r.pairs,
            if r.face.is_some() { "yes" } else { "no" },
            val(Check::Response),
            val(Check::Falloff),
            val(Check::Motion),
        );
    }
}

#[test]
fn live_captures_pass_liveness_including_daylight() -> Result<(), String> {
    let replays = replay_corpus();
    if replays.is_empty() {
        return Ok(());
    }
    let mut checked = 0;
    for r in replays.iter().filter(|r| r.label.starts_with("live_")) {
        let rep = r.report.as_ref()
            .ok_or_else(|| format!("{}: live burst rejected: {:?}", r.label, r.burst_error))?;
        if rep.rejection().is_some() || rep.pairs < 2 {
            return Err(format!("{}: live liveness rejected: {rep:?}", r.label));
        }
        checked += 1;
    }
    if checked == 0 {
        return Err("no live captures were checked".into());
    }
    Ok(())
}

#[test]
fn live_falloff_distribution_bounds_the_flatness_floor() {
    let replays = replay_corpus();
    let live: Vec<&Replay> = replays.iter().filter(|r| r.label.starts_with("live_")).collect();
    if live.is_empty() {
        return;
    }

    let mut worst = f64::INFINITY;
    eprintln!("\nlive falloff CV over the detected face box\n{:<26} {:>10}", "capture", "worst");
    for r in &live {
        let Some(c) = r.report.as_ref().and_then(|rep| rep.get(Check::Falloff)) else {
            eprintln!("{:<26} {:>10}", r.label, "no box");
            continue;
        };
        eprintln!("{:<26} {:>10.4}", r.label, c.worst);
        worst = worst.min(c.worst);
    }

    if worst.is_finite() {
        eprintln!(
            "\nworst live falloff CV: {worst:.4}\n\
             A flatness floor must sit strictly below this to cost zero false rejects.\n\
             It bounds nothing about prints until a spoof_print_* recording exists."
        );
        assert!(worst > 0.0, "a live face must produce some falloff at all");
    }
}

#[test]
fn class_separation_report() {
    let replays = replay_corpus();
    if replays.is_empty() {
        return;
    }

    let class_of = |label: &str| label.split('_').next().unwrap_or("other").to_string();
    let metrics = [
        (Check::DarkFrame, "dark OFF/ON", "lower is cleaner"),
        (Check::Response, "response D", "higher is lit"),
        (Check::Falloff, "falloff CV", "higher is curved"),
        (Check::Motion, "motion", "lower is still"),
    ];

    eprintln!("\nclass separation — the gap between rows is the signal a check provides");

    for (check, name, sense) in metrics {
        eprintln!("\n  {name}  ({sense})");
        eprintln!("  {:<12} {:>5} {:>14} {:>14}", "class", "n", "min", "max");

        let mut classes: Vec<String> =
            replays.iter().map(|r| class_of(&r.label)).collect();
        classes.sort();
        classes.dedup();

        for class in classes {
            let vals: Vec<f64> = replays
                .iter()
                .filter(|r| class_of(&r.label) == class)
                .filter_map(|r| r.report.as_ref().and_then(|rep| rep.get(check)))
                .map(|c| c.worst)
                .collect();
            if vals.is_empty() {
                eprintln!("  {class:<12} {:>5} {:>14} {:>14}", 0, "-", "-");
                continue;
            }
            let lo = vals.iter().cloned().fold(f64::INFINITY, f64::min);
            let hi = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            eprintln!("  {class:<12} {:>5} {lo:>14.5} {hi:>14.5}", vals.len());
        }
    }

    let have: Vec<&str> = replays
        .iter()
        .map(|r| r.label.as_str())
        .filter(|l| l.starts_with("spoof_"))
        .collect();
    if have.is_empty() {
        eprintln!(
            "\n  No spoof_* captures. The falloff and response checks therefore have no\n\
             negative class at all, which is why both are limited: falloff enforces only a\n\
             degenerate-flatness floor and response is advisory. See wiki/09."
        );
    } else {
        eprintln!("\n  spoof captures present: {}", have.join(", "));
        eprintln!(
            "  Compare the live and spoof rows above. A metric whose ranges do not overlap\n\
             can carry an enforced threshold; one whose ranges overlap cannot, whatever\n\
             value is chosen."
        );
    }
}

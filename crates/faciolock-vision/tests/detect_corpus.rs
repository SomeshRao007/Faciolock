mod common;

use faciolock_vision::config::DetectConfig;
use faciolock_vision::{Detector, Frame};

const SEARCH_FRAMES: usize = 20;

fn brightest_frame(cap: &common::Capture) -> usize {
    let start = 5.min(cap.frame_count().saturating_sub(1));
    let end = SEARCH_FRAMES.min(cap.frame_count());
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

#[test]
fn yunet_loads_and_reports_the_outputs_this_decode_expects() {
    let path = common::yunet();
    if !path.exists() {
        common::skip("YuNet model", &path);
        return;
    }
    let det = Detector::load(&path, &DetectConfig::default());
    assert!(det.is_ok(), "{:?}", det.err());
}

#[test]
fn a_face_is_found_on_live_captures_and_not_in_an_empty_room() {
    let path = common::yunet();
    if !path.exists() {
        common::skip("YuNet model", &path);
        return;
    }
    let captures = common::captures();
    if captures.is_empty() {
        return;
    }

    let cfg = DetectConfig::default();
    let det = Detector::load(&path, &cfg).expect("model loads");

    eprintln!(
        "\ndetection @ {}x{}, score_min {}\n{:<34} {:>6} {:>26} {:>7}",
        cfg.input_size, cfg.input_size, cfg.score_min, "capture", "faces", "box (x,y,w,h)", "score"
    );

    let mut checked = 0;
    for cap in &captures {
        let px = cap.frame(brightest_frame(cap));
        let frame = Frame::new(px, cap.width, cap.height).expect("geometry matches the sidecar");
        let found = det.detect(&frame).expect("inference runs");

        let summary = match found.first() {
            None => format!("{:>26} {:>7}", "-", "-"),
            Some(d) => format!(
                "{:>26} {:>7.3}",
                format!(
                    "{:.0},{:.0},{:.0},{:.0}",
                    d.bounds.x, d.bounds.y, d.bounds.w, d.bounds.h
                ),
                d.score
            ),
        };
        eprintln!("{:<34} {:>6} {summary}", cap.label, found.len());

        if cap.label.starts_with("live_") {
            assert!(
                !found.is_empty(),
                "{}: no face detected in a live capture — the whole pipeline rests on this",
                cap.label
            );
            let d = found[0];
            assert!(
                d.bounds.x >= -d.bounds.w
                    && d.bounds.y >= -d.bounds.h
                    && d.bounds.x < cap.width as f32
                    && d.bounds.y < cap.height as f32,
                "{}: box {:?} is not in full-frame coordinates",
                cap.label,
                d.bounds
            );
            for (i, (lx, ly)) in d.landmarks.iter().enumerate() {
                assert!(
                    *lx > -50.0
                        && *ly > -50.0
                        && *lx < cap.width as f32 + 50.0
                        && *ly < cap.height as f32 + 50.0,
                    "{}: landmark {i} at ({lx},{ly}) is off the frame — check the letterbox inverse",
                    cap.label
                );
            }
            checked += 1;
        }

        if cap.label.starts_with("null_") {
            assert!(found.is_empty(), "{}: found a face in an empty room", cap.label);
            checked += 1;
        }
    }

    assert!(checked > 0, "no live_* or null_* captures were available to check");
}

#[test]
fn landmark_convergence_across_detector_input_sizes() {
    let path = common::yunet();
    if !path.exists() {
        common::skip("YuNet model", &path);
        return;
    }
    let captures = common::captures();
    let live: Vec<_> = captures.iter().filter(|c| c.label.starts_with("live_")).collect();
    if live.is_empty() {
        return;
    }

    const SIZES: [usize; 4] = [160, 192, 320, 448];
    let detectors: Vec<(usize, Detector)> = SIZES
        .iter()
        .filter_map(|&s| {
            Detector::load(&path, &DetectConfig { input_size: s, ..Default::default() })
                .ok()
                .map(|d| (s, d))
        })
        .collect();
    let Some((ref_size, reference)) = detectors.last() else { return };

    eprintln!("\nlandmark displacement from the {ref_size}x{ref_size} reference, full-frame px");
    eprintln!(
        "{:<26} {:>6} {:>8} {:>8} {:>8} {:>9}",
        "capture", "size", "mean", "max", "box IoU", "infer ms"
    );

    for cap in &live {
        let px = cap.frame(brightest_frame(cap));
        let frame = Frame::new(px, cap.width, cap.height).expect("geometry matches");
        let Ok(Some(gold)) = reference.detect_best(&frame) else {
            eprintln!("{:<26} {:>6}", cap.label, "no reference detection");
            continue;
        };

        for (size, det) in &detectors {
            let start = std::time::Instant::now();
            let got = det.detect_best(&frame);
            let ms = start.elapsed().as_secs_f64() * 1000.0;

            match got {
                Ok(Some(d)) => {
                    let deltas: Vec<f32> = d
                        .landmarks
                        .iter()
                        .zip(&gold.landmarks)
                        .map(|((ax, ay), (bx, by))| ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt())
                        .collect();
                    let mean = deltas.iter().sum::<f32>() / deltas.len() as f32;
                    let max = deltas.iter().cloned().fold(0.0f32, f32::max);
                    eprintln!(
                        "{:<26} {size:>6} {mean:>8.2} {max:>8.2} {:>8.3} {ms:>9.1}",
                        cap.label,
                        d.bounds.iou(&gold.bounds)
                    );
                }
                _ => eprintln!("{:<26} {size:>6} {:>8}", cap.label, "none"),
            }
        }
    }
}

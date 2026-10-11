use faciolock_vision::config::Threshold;
use faciolock_vision::liveness::{self, Check};
use faciolock_vision::testing::{blank, domed, flat, strobe, strobe_one_contaminated};
use faciolock_vision::{Burst, BurstError, Config, Observed, Rect};

const W: usize = 96;
const H: usize = 96;

fn cfg() -> Config {
    Config { agc_settle_frames: 0, ..Config::default() }
}

fn whole() -> Rect {
    Rect { x: 0.0, y: 0.0, w: W as f32, h: H as f32 }
}

fn live_burst() -> Vec<u8> {
    strobe(&domed(W, H, 60, 8), &blank(W, H, 0), 6)
}

#[test]
fn a_live_like_capture_is_allowed() {
    let c = cfg();
    let bytes = live_burst();
    let burst = Burst::from_planar(&bytes, W, H, &c).expect("forms a burst");
    let report = liveness::evaluate(&burst, &whole(), &c);
    assert!(report.rejection().is_none(), "{report:?}");
    assert_eq!(burst.pairs().len(), 6);
}

#[test]
fn daylight_requires_both_absolute_and_relative_emitter_response() -> Result<(), String> {
    let c = cfg();
    let on: Vec<u8> = domed(W, H, 60, 8).iter().map(|p| p + 100).collect();
    let bytes = strobe(&on, &blank(W, H, 100), 3);
    let burst = Burst::from_planar(&bytes, W, H, &c).map_err(|e| format!("daylight: {e:?}"))?;
    let report = liveness::evaluate(&burst, &whole(), &c);
    if report.rejection().is_some() {
        return Err(format!("ambient light hid a usable curved response: {report:?}"));
    }

    for (on, off, expected) in [(4, 0, Check::Response), (100, 95, Check::DarkFrame)] {
        let bytes = strobe(&blank(W, H, on), &blank(W, H, off), 3);
        let burst = Burst::from_planar(&bytes, W, H, &c).map_err(|e| format!("weak IR: {e:?}"))?;
        let rejected = liveness::pre_box(&burst, &c).rejection();
        if rejected.map(|r| r.check) != Some(expected) {
            return Err(format!("ON={on}, OFF={off}: expected {expected:?}, got {rejected:?}"));
        }
    }

    let bytes = strobe(&blank(W, H, 255), &blank(W, H, 240), 3);
    if !matches!(Burst::from_planar(&bytes, W, H, &c), Err(BurstError::AmbientTooBright { .. })) {
        return Err("near-saturated ambient must not yield a usable burst".into());
    }
    Ok(())
}

#[test]
fn a_single_contaminated_pair_rejects_the_burst() {
    let c = cfg();
    let bytes = strobe_one_contaminated(
        &flat(W, H, 60),
        &blank(W, H, 0),
        &blank(W, H, 57),
        6,
        3,
    );
    let burst = Burst::from_planar(&bytes, W, H, &c).expect("alternation survives");
    let report = liveness::evaluate(&burst, &whole(), &c);

    let r = report.rejection().expect("one bad pair must sink the burst");
    assert_eq!(r.check, Check::DarkFrame);
    assert_eq!(r.failing_pairs, 1);
    assert!(
        r.failing_pairs * 2 < report.pairs,
        "the failing pairs are a minority — which is exactly why voting fails here"
    );
}

#[test]
fn a_degenerate_flat_presentation_is_rejected() {
    let c = cfg();
    let bytes = strobe(&flat(W, H, 60), &blank(W, H, 0), 6);
    let burst = Burst::from_planar(&bytes, W, H, &c).expect("forms a burst");
    let r = liveness::evaluate(&burst, &whole(), &c).rejection().expect("flat must reject");
    assert_eq!(r.check, Check::Falloff);
}

#[test]
fn constant_illumination_is_refused_rather_than_downgraded() {
    let c = cfg();
    match Burst::from_planar(&blank(W, H, 60).repeat(12), W, H, &c) {
        Err(BurstError::TierMismatch { observed: Observed::Constant, .. }) => {}
        other => panic!("expected constant illumination rejection, got {:?}", other.map(|_| "a burst")),
    }

    for level in [0u8, 1, 30, 60, 200, 255] {
        assert!(
            Burst::from_planar(&blank(W, H, level).repeat(12), W, H, &c).is_err(),
            "constant illumination at {level} produced a burst"
        );
    }
}

#[test]
fn a_burst_below_the_pair_floor_is_refused() {
    let c = cfg();
    let on = domed(W, H, 60, 8);
    let off = blank(W, H, 0);
    let bytes = [on.as_slice(), off.as_slice(), on.as_slice()].concat();
    assert!(matches!(
        Burst::from_planar(&bytes, W, H, &c),
        Err(BurstError::TooFewPairs { got: 1, need: 2 })
    ));
}

#[test]
fn a_capture_too_short_to_measure_is_not_a_strobe() {
    let c = cfg();
    let bytes = strobe(&domed(W, H, 60, 8), &blank(W, H, 0), 1);
    assert!(matches!(
        Burst::from_planar(&bytes, W, H, &c),
        Err(BurstError::TierMismatch { observed: Observed::Irregular, .. })
    ));
}

#[test]
fn a_malformed_capture_is_refused() {
    let c = cfg();
    assert!(matches!(
        Burst::from_planar(&vec![0u8; W * H + 3], W, H, &c),
        Err(BurstError::Malformed)
    ));
}

#[test]
fn advisory_checks_report_but_never_reject() {
    let mut c = cfg();
    c.liveness.falloff_cv_min = Threshold::advisory(0.5);
    let bytes = live_burst();
    let burst = Burst::from_planar(&bytes, W, H, &c).unwrap();
    let report = liveness::evaluate(&burst, &whole(), &c);

    assert!(report.rejection().is_none(), "advisory must not reject");
    assert_eq!(report.advisory_failures().count(), 1);
}

#[test]
fn a_curved_reflector_outscores_a_flat_one() {
    let c = cfg();
    let curved_bytes = strobe(&domed(W, H, 60, 10), &blank(W, H, 0), 4);
    let flat_bytes = strobe(&flat(W, H, 60), &blank(W, H, 0), 4);

    let curved = Burst::from_planar(&curved_bytes, W, H, &c).unwrap();
    let print = Burst::from_planar(&flat_bytes, W, H, &c).unwrap();

    let cv = |b: &Burst<'_>| {
        liveness::evaluate(b, &whole(), &c).get(Check::Falloff).unwrap().worst
    };
    let (curved_cv, flat_cv) = (cv(&curved), cv(&print));
    assert!(flat_cv < 0.01, "flat should be near-uniform, got {flat_cv}");
    assert!(curved_cv > flat_cv * 10.0, "curved {curved_cv} vs flat {flat_cv}");
}

#[test]
fn shipped_models_are_the_ones_the_thresholds_were_measured_against() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let expected: [(&str, u64); 2] = [
        ("face_detection_yunet_2026may.onnx", 229_738),
        ("face_recognition_sface_2021dec.onnx", 38_696_353),
    ];
    for (name, bytes) in expected {
        let path = root.join("models").join(name);
        let Ok(meta) = std::fs::metadata(&path) else {
            eprintln!("  (skipped: {} is absent — models are gitignored)", path.display());
            continue;
        };
        assert_eq!(
            meta.len(),
            bytes,
            "{name} is {} bytes, expected {bytes} — every threshold in config.rs was \
             measured against a different file",
            meta.len()
        );
    }
}

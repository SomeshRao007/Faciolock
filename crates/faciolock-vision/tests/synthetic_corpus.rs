//! The committed CI corpus: generated captures, no camera, no models, no recordings.
//!
//! Everything else in `tests/` needs either the 20 MB `.gray` recordings of the operator's
//! face or the ONNX models, and both are gitignored — the recordings because publishing
//! biometrics to a public git history is irreversible, the models because they are large
//! upstream binaries. **A Debian buildd has no network**, so a test that fetches its own
//! inputs is a test that does not run in the archive, which is where wiki/08 Q6 commits
//! this project to going.
//!
//! So this file is the part that always runs. It proves the *rules*: that liveness
//! aggregates conjunctively, that a burst which cannot be measured is refused rather than
//! passed, that a tier is never downgraded, and that only a match is an allow. It cannot
//! prove a *threshold* — synthetic frames have no sensor noise, no AGC droop and no real
//! reflectance — and it does not try to. Thresholds are calibrated against the local
//! recordings, and the numbers behind each one are recorded in `config.rs`.

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

/// A curved reflector strobed against true dark frames: what a live face looks like.
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

/// The [B9](../../../../wiki/11-bug-log.md) regression, in the corpus that always runs.
///
/// One contaminated pair in six. A median would not see it, a majority vote would not see
/// it, and a per-pair rule would let the burst through on the other five. The real attack
/// read clean on 7 of 42 pairs, which at the two-pair capture budget is a 1-in-36 bypass.
#[test]
fn a_single_contaminated_pair_rejects_the_burst() {
    let c = cfg();
    let bytes = strobe_one_contaminated(
        &domed(W, H, 60, 8),
        &blank(W, H, 0),
        &blank(W, H, 5), // polluted, but still far darker than the ON frame beside it
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

/// A perfectly flat presentation fails the flatness floor. This is the *whole* of what the
/// falloff check currently claims: it does not claim to reject a printed photo, because no
/// printed photo has ever been recorded.
#[test]
fn a_degenerate_flat_presentation_is_rejected() {
    let c = cfg();
    let bytes = strobe(&flat(W, H, 60), &blank(W, H, 0), 6);
    let burst = Burst::from_planar(&bytes, W, H, &c).expect("forms a burst");
    let r = liveness::evaluate(&burst, &whole(), &c).rejection().expect("flat must reject");
    assert_eq!(r.check, Check::Falloff);
}

/// Tier is configuration, never inference. A steadily-lit capture — which any attacker with
/// a bright enough continuous IR source can produce at will — must be refused, never served
/// by the weaker checks a constant-illumination tier would run.
///
/// Since [B18] the *ambient gate* answers first, and more accurately: a uniformly lit frame
/// means a bright room, so "too bright to verify" beats "wrong illumination pattern". The
/// property being defended is unchanged — no constant capture yields a burst — so this
/// asserts that across the brightness range rather than pinning one error.
///
/// [B18]: ../../../../wiki/11-bug-log.md
#[test]
fn constant_illumination_is_refused_rather_than_downgraded() {
    let c = cfg();
    match Burst::from_planar(&blank(W, H, 60).repeat(12), W, H, &c) {
        Err(BurstError::AmbientTooBright { floor }) => {
            assert!((floor - 60.0).abs() < 0.5, "floor {floor} should be the frame mean")
        }
        other => panic!("expected the ambient gate, got {:?}", other.map(|_| "a burst")),
    }

    for level in [0u8, 1, 30, 60, 200, 255] {
        assert!(
            Burst::from_planar(&blank(W, H, level).repeat(12), W, H, &c).is_err(),
            "constant illumination at {level} produced a burst"
        );
    }
}

/// Conjunction over an empty set is true, so a burst that yields too few pairs must be
/// refused outright. Otherwise every check that was meant to inspect it passes vacuously.
///
/// Three frames, not two. Since [B18] the noise floor is measured from same-phase pairs,
/// which a two-frame capture does not contain, so `ON, OFF, ON` is now the shortest capture
/// that classifies as a strobe at all while still yielding fewer pairs than the floor.
///
/// [B18]: ../../../../wiki/11-bug-log.md
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

/// And a capture too short to measure its own noise floor is refused earlier, naming the
/// real problem. "Unmeasurable" must never become a way through.
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

/// An advisory check must report a failure and must not act on it. This is the mechanism
/// that keeps the shipped verdict honest about what the corpus can support: three of the
/// four liveness checks have no spoof recording behind them.
#[test]
fn advisory_checks_report_but_never_reject() {
    let mut c = cfg();
    c.liveness.falloff_cv_min = Threshold::advisory(0.5); // above every live value
    let bytes = live_burst();
    let burst = Burst::from_planar(&bytes, W, H, &c).unwrap();
    let report = liveness::evaluate(&burst, &whole(), &c);

    assert!(report.rejection().is_none(), "advisory must not reject");
    assert_eq!(report.advisory_failures().count(), 1);
}

/// The falloff mechanism, on data where the answer is known by construction. If this ever
/// fails, the print-rejection claim in wiki/05 has no mechanism behind it at all.
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

/// The shipped model files are the ones every measurement in `config.rs` was taken
/// against. A swapped model would invalidate the thresholds silently — a decode validated
/// against `2026may` is not validated against `2023mar`.
///
/// Byte length rather than SHA256 deliberately: it catches a substituted file, needs no
/// dependency, and the real content hash belongs with the `SHA256SUMS` manifest the daemon
/// verifies at startup, which is Phase 2's work (wiki/05, wiki/09). Skips when the models
/// are absent, since they are gitignored.
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

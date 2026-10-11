mod common;

use faciolock_vision::pipeline::{enroll_template, verify_planar, Verdict};
use faciolock_vision::{Burst, BurstError, Config, Observed, Pipeline, Tier};

#[test]
fn production_defaults_unlock_daylight_and_reject_negative_presentations() -> Result<(), String> {
    for model in [common::yunet(), common::sface()] {
        if !model.exists() {
            common::skip("model", &model);
            return Ok(());
        }
    }
    let captures = common::captures();
    if captures.is_empty() {
        common::skip("corpus captures", &common::corpus_dir());
        return Ok(());
    }

    let cfg = Config::default();
    let pipe = Pipeline::load(&common::models_dir(), cfg)?;
    // The daemon discards warmup frames in its camera session, before building a burst.
    let daemon_cfg = Config { agc_settle_frames: 0, ..cfg };
    let daemon = Pipeline::load(&common::models_dir(), daemon_cfg)?;
    for stem in ["live_lit_frontal_20260803T183134Z", "live_daylight_frontal_20260806T060306Z"] {
        let enrol = captures.iter().find(|c| c.stem == stem)
            .ok_or_else(|| format!("missing enrollment capture: {stem}"))?;
        let burst = Burst::from_planar(&enrol.bytes, enrol.width, enrol.height, &cfg)
            .map_err(|e| format!("{stem}: enrollment burst: {e:?}"))?;
        let template = enroll_template(&pipe, &burst)
            .map_err(|o| format!("{stem}: enrollment: {:?}", o.verdict))?;
        let stride = enrol.width * enrol.height;
        let start = enrol.frame_count() / 2;
        let pixels = enrol.bytes.get(start * stride..(start + 6) * stride)
            .ok_or_else(|| format!("{stem}: missing six-frame enrollment window"))?;
        let burst = Burst::from_planar(pixels, enrol.width, enrol.height, &daemon_cfg)
            .map_err(|e| format!("{stem}: daemon enrollment burst: {e:?}"))?;
        let daemon_template = enroll_template(&daemon, &burst)
            .map_err(|o| format!("{stem}: daemon enrollment: {:?}", o.verdict))?;

        let mut live = 0;
        let mut negative = 0;
        let mut daemon_live = 0;
        let mut daemon_negative = 0;
        let mut daemon_unusable = 0;
        let mut mug = false;
        for cap in &captures {
            let pixels = cap.verification_bytes();
            let outcome = verify_planar(&pipe, pixels, cap.width, cap.height, &template);
            if outcome.verdict.is_allow() != cap.has_live_face() {
                return Err(format!("enrolled {stem}, verifying {}: {:?}", cap.stem, outcome.verdict));
            }
            if cap.label == "nonflat_mug_ceramic" {
                if !matches!(outcome.verdict, Verdict::NoFace) || outcome.probes != 0 {
                    return Err(format!("mug reached identity matching: {:?}", outcome.verdict));
                }
                mug = true;
            }
            if !cap.has_live_face() {
                negative += 1;
            } else if cap.label.starts_with("live_") {
                live += 1;
            } else {
                continue;
            }

            let stride = cap.width * cap.height;
            let frames = pixels.len() / stride;
            if frames < 11 {
                return Err(format!("{}: too few frames for daemon windows", cap.stem));
            }
            for start in [5, frames / 2, frames - 6] {
                let bytes = &pixels[start * stride..(start + 6) * stride];
                let outcome = verify_planar(&daemon, bytes, cap.width, cap.height, &daemon_template);
                // This isolated window has temporal SNR 3.801, below the unchanged 10.0 floor.
                if cap.stem == "live_daylight_frontal_20260806T060306Z" && start == 5 {
                    if !matches!(outcome.verdict, Verdict::Unusable(BurstError::TierMismatch {
                        configured: Tier::IrStrobe,
                        observed: Observed::Constant,
                    })) || outcome.probes != 0 {
                        return Err(format!("known low-SNR window changed: {:?}", outcome.verdict));
                    }
                    daemon_unusable += 1;
                    continue;
                }
                if outcome.verdict.is_allow() != cap.has_live_face() {
                    return Err(format!("enrolled {stem}, {} at {start}: {:?}", cap.stem, outcome.verdict));
                }
                if !cap.has_live_face() {
                    if cap.label == "nonflat_mug_ceramic" && outcome.probes != 0 {
                        return Err(format!("mug at {start} reached identity matching"));
                    }
                    daemon_negative += 1;
                    continue;
                }
                daemon_live += 1;
                if start != 5 {
                    continue;
                }
                let burst = Burst::from_planar(bytes, cap.width, cap.height, &daemon_cfg)
                    .map_err(|e| format!("{} at {start}: daemon burst: {e:?}", cap.stem))?;
                enroll_template(&daemon, &burst)
                    .map_err(|o| format!("{}: six-frame enrollment: {:?}", cap.stem, o.verdict))?;
            }
        }
        if live < 12 || negative < 14 || !mug || daemon_unusable != 1 {
            return Err(format!("incomplete corpus: {live} live, {negative} negatives, mug={mug}, known unusable={daemon_unusable}"));
        }
        eprintln!("{stem}: {live} live recordings + {daemon_live} six-frame bursts matched; {negative} negatives + {daemon_negative} six-frame bursts rejected; {daemon_unusable} known low-SNR six-frame window unusable");
    }
    Ok(())
}

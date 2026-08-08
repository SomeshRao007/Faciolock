//! The pipeline: a burst in, a verdict out.
//!
//! ```text
//! burst ─→ cheap liveness ─→ detect ─→ ROI liveness ─→ align ─→ embed ─→ match
//!          dark-frame,        YuNet    falloff,        112x112   SFace    cosine
//!          response                    motion                            k-of-n
//!          (no box needed)             (needs the box)
//! ```
//!
//! **Nothing is embedded or matched until liveness has passed.** Detection sits in the
//! middle because the falloff test is defined over the detected face region and cannot run
//! without a box ([B8](../../../wiki/11-bug-log.md)); the property that mattered is
//! untouched, because embedding and matching still come last.
//!
//! The checks needing no box run first as an early-out: a screen or a floodlight is
//! rejected for a few `f32` reductions without paying for detection at all.

use std::path::Path;

use crate::align::{self, Chip};
use crate::config::Config;
use crate::detect::{DetectError, Detection, Detector};
use crate::embed::{self, EmbedError, Embedder, Embedding, SFace, Template};
use crate::frame::{Burst, BurstError, Frame, Rect};
use crate::liveness::{self, Check, LivenessReport};

/// Why an authentication attempt ended the way it did.
///
/// A reason-carrying enum rather than a bool, because Phase 3's IPC and Phase 4's audit log
/// both freeze these variants and both need to distinguish "nobody was in front of the
/// camera" from "somebody was, and they failed liveness". Only `Match` is an allow.
///
/// **What crosses the socket is narrower than this.** wiki/04's wire protocol carries
/// `Deny | Allow | Unavailable | NoEnrolment | RateLimited` and nothing else; the detail
/// here is for the journal, not for the caller. Telling an attacker *which* check rejected
/// them is telling them what to fix.
#[derive(Clone, Debug)]
pub enum Verdict {
    /// The enrolled user, alive, agreed on by enough frames.
    Match { similarity: f64, agreeing: usize },
    /// A live face that is not the enrolled user.
    NoMatch { best: f64 },
    /// Liveness rejected the burst, and the check that did it was one we can defend.
    NotLive { check: Check, worst: f64, threshold: f64 },
    /// Nobody was found in the frame.
    NoFace,
    /// The room is too bright in near-infrared for this hardware to verify liveness.
    ///
    /// **Not a failed authentication, and Phase 4 must not count it as one.** Nothing was
    /// rejected, because nothing was measured: the gate fires before detection, so no face
    /// was ever looked for. Feeding this to the rate limiter would let the weather disable
    /// face unlock, and exempting it *after* biometric work had run would turn the
    /// exemption into a free probing oracle. See [`Config::ambient_floor_max`](crate::Config)
    /// and [B18](../../../wiki/11-bug-log.md).
    ///
    /// On the wire this maps to `Unavailable` (wiki/04) — the same thing the caller already
    /// gets when the camera is busy, and for the same reason: the daemon cannot answer the
    /// question right now, so PAM falls through to the password prompt.
    Environment { ambient_floor: f32 },
    /// The capture never became something worth measuring — wrong illumination pattern for
    /// the configured tier, or too few usable pairs.
    Unusable(BurstError),
    /// The pipeline itself failed. **Never an allow**; the caller turns this into
    /// `PAM_AUTHINFO_UNAVAIL` and the user types their password.
    Failed(String),
}

impl Verdict {
    /// The single place a success is recognised.
    ///
    /// wiki/09 wants exactly one construction site for `PAM_SUCCESS` in the PAM module;
    /// this is the same discipline one layer down. Every other variant is a deny, including
    /// every variant added later — the match is exhaustive on purpose, so a new variant
    /// cannot default to allow by being forgotten.
    pub fn is_allow(&self) -> bool {
        match self {
            Verdict::Match { .. } => true,
            Verdict::NoMatch { .. }
            | Verdict::NotLive { .. }
            | Verdict::NoFace
            | Verdict::Environment { .. }
            | Verdict::Unusable(_)
            | Verdict::Failed(_) => false,
        }
    }

    /// Whether this outcome should count against the user's failure budget.
    ///
    /// Phase 4's rate limiter and lockout read this. An attempt that never got as far as
    /// looking at a face is not evidence of anything, and counting it would mean sunshine
    /// — or a camera in use by a video call — disabling face unlock until the user
    /// authenticates by password.
    ///
    /// Exhaustive on purpose: a variant added later must be classified deliberately rather
    /// than inheriting an exemption by being forgotten.
    pub fn counts_as_an_attempt(&self) -> bool {
        match self {
            Verdict::Match { .. }
            | Verdict::NoMatch { .. }
            | Verdict::NotLive { .. }
            | Verdict::NoFace => true,
            // Nothing was measured, so nothing was attempted.
            Verdict::Environment { .. } | Verdict::Unusable(_) | Verdict::Failed(_) => false,
        }
    }
}

impl From<BurstError> for Verdict {
    /// The single place a burst failure becomes a verdict, so the ambient gate cannot be
    /// mapped to a plain `Unusable` by a caller that forgot about it.
    fn from(e: BurstError) -> Verdict {
        match e {
            BurstError::AmbientTooBright { floor } => Verdict::Environment { ambient_floor: floor },
            other => Verdict::Unusable(other),
        }
    }
}

/// Everything an audit log needs about one attempt.
#[derive(Clone, Debug)]
pub struct Outcome {
    pub verdict: Verdict,
    pub liveness: LivenessReport,
    pub face: Option<Rect>,
    /// How many frames produced a usable embedding.
    pub probes: usize,
}

impl Outcome {
    fn deny(verdict: Verdict, liveness: LivenessReport, face: Option<Rect>) -> Outcome {
        Outcome { verdict, liveness, face, probes: 0 }
    }
}

pub struct Pipeline {
    detector: Detector,
    embedder: Box<dyn Embedder>,
    cfg: Config,
}

impl Pipeline {
    pub fn new(detector: Detector, embedder: Box<dyn Embedder>, cfg: Config) -> Pipeline {
        Pipeline { detector, embedder, cfg }
    }

    /// Load the shipped models from a directory.
    ///
    /// Phase 2 adds SHA256 verification against a manifest here and refuses to run on a
    /// mismatch (wiki/05). That belongs with the daemon's startup path, which is where the
    /// manifest will live.
    pub fn load(models: &Path, cfg: Config) -> Result<Pipeline, String> {
        let detector = Detector::load(&models.join("face_detection_yunet_2026may.onnx"), &cfg.detect)
            .map_err(|e: DetectError| e.to_string())?;
        let embedder = SFace::load(&models.join("face_recognition_sface_2021dec.onnx"))
            .map_err(|e: EmbedError| e.to_string())?;
        Ok(Pipeline::new(detector, Box::new(embedder), cfg))
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    pub fn embedder_id(&self) -> &'static str {
        self.embedder.id()
    }

    /// Verify a burst against an enrolled template.
    pub fn verify(&self, burst: &Burst<'_>, template: &Template) -> Outcome {
        let (mut report, detections) = match self.liveness_and_detect(burst) {
            Ok(v) => v,
            Err(outcome) => return outcome,
        };
        let face = detections.first().map(|(_, d)| d.bounds);

        report = report.merge(liveness::post_box(burst, &face.unwrap(), &self.cfg));
        if let Some(c) = report.rejection() {
            return Outcome::deny(
                Verdict::NotLive { check: c.check, worst: c.worst, threshold: c.threshold },
                report,
                face,
            );
        }

        // Only here, with liveness settled, does anything get embedded.
        let probes = match self.embed_all(burst, &detections) {
            Ok(p) => p,
            Err(e) => return Outcome::deny(Verdict::Failed(e), report, face),
        };

        let outcome = embed::match_k_of_n(template, &probes, &self.cfg.matching);
        let verdict = if outcome.matched {
            Verdict::Match { similarity: outcome.best, agreeing: outcome.agreeing }
        } else {
            Verdict::NoMatch { best: outcome.best }
        };
        Outcome { verdict, liveness: report, face, probes: probes.len() }
    }

    /// Turn a burst into enrolment embeddings.
    ///
    /// Runs the same liveness gate as verification, deliberately. Enrolling from a photo
    /// would poison the template store permanently, and it is the one moment an attacker
    /// gets to choose the input.
    pub fn enroll(&self, burst: &Burst<'_>) -> Result<Vec<Embedding>, Outcome> {
        let (mut report, detections) = self.liveness_and_detect(burst)?;
        let face = detections.first().map(|(_, d)| d.bounds);

        report = report.merge(liveness::post_box(burst, &face.unwrap(), &self.cfg));
        if let Some(c) = report.rejection() {
            return Err(Outcome::deny(
                Verdict::NotLive { check: c.check, worst: c.worst, threshold: c.threshold },
                report,
                face,
            ));
        }

        match self.embed_all(burst, &detections) {
            Ok(p) if !p.is_empty() => Ok(p),
            Ok(_) => Err(Outcome::deny(Verdict::NoFace, report, face)),
            Err(e) => Err(Outcome::deny(Verdict::Failed(e), report, face)),
        }
    }

    /// The shared prefix: cheap liveness, then detection on the frames that will be
    /// embedded. Returns the report so far plus the detections, or a finished denial.
    #[allow(clippy::type_complexity)]
    fn liveness_and_detect<'a>(
        &self,
        burst: &Burst<'a>,
    ) -> Result<(LivenessReport, Vec<(Frame<'a>, Detection)>), Outcome> {
        let report = liveness::pre_box(burst, &self.cfg);
        if let Some(c) = report.rejection() {
            return Err(Outcome::deny(
                Verdict::NotLive { check: c.check, worst: c.worst, threshold: c.threshold },
                report,
                None,
            ));
        }

        // **One detection for the burst**, its landmarks shared by every frame that gets
        // embedded. This is wiki/06's "YuNet detection (x1)" line and it is a measured
        // trade, not an assumption:
        //
        //   * detecting per frame at a size cheap enough to afford three passes means
        //     192x192 or below, which costs 0.82 embedding cosine against a high-resolution
        //     reference — worse than 8 px of landmark error;
        //   * detecting once at 320 costs 0.94, and sharing those landmarks across frames
        //     ~66 ms apart costs whatever the face moved in 66 ms. At the measured
        //     sensitivity, 2 px costs 0.98 and 4 px costs 0.96.
        //
        // An order of magnitude apart, so resolution wins and the shared box is the cheap
        // half of the bargain. k-of-n keeps its independence regardless: three *different
        // images* are still embedded, so a blur or a blink in one frame is still outvoted.
        // Only the alignment is shared.
        let mut found = None;
        for pair in burst.pairs().iter().take(self.cfg.matching.agree_n) {
            match self.detector.detect_best(&pair.on) {
                Ok(Some(d)) => {
                    found = Some(d);
                    break;
                }
                Ok(None) => {}
                Err(e) => {
                    return Err(Outcome::deny(Verdict::Failed(e.to_string()), report, None));
                }
            }
        }
        let Some(detection) = found else {
            return Err(Outcome::deny(Verdict::NoFace, report, None));
        };

        let detections: Vec<(Frame<'a>, Detection)> = burst
            .pairs()
            .iter()
            .take(self.cfg.matching.agree_n)
            .map(|p| (p.on, detection))
            .collect();
        Ok((report, detections))
    }

    fn embed_all(
        &self,
        _burst: &Burst<'_>,
        detections: &[(Frame<'_>, Detection)],
    ) -> Result<Vec<Embedding>, String> {
        let mut probes = Vec::with_capacity(detections.len());
        for (frame, d) in detections {
            let Some(chip) = align::align(frame, &d.landmarks) else { continue };
            match self.embedder.embed(&chip) {
                Ok(e) => probes.push(e),
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(probes)
    }

    /// Align a single detection, for callers measuring alignment in isolation.
    pub fn chip(frame: &Frame<'_>, d: &Detection) -> Option<Chip> {
        align::align(frame, &d.landmarks)
    }
}

/// Build a template from a burst, ready for the store.
pub fn enroll_template(pipeline: &Pipeline, burst: &Burst<'_>) -> Result<Template, Outcome> {
    let embeddings = pipeline.enroll(burst)?;
    Template::new(pipeline.embedder_id(), embeddings).ok_or_else(|| {
        Outcome::deny(Verdict::NoFace, LivenessReport::default(), None)
    })
}

/// Build a burst and verify it in one step.
pub fn verify_planar(
    pipeline: &Pipeline,
    bytes: &[u8],
    width: usize,
    height: usize,
    template: &Template,
) -> Outcome {
    match Burst::from_planar(bytes, width, height, pipeline.config()) {
        Ok(b) => pipeline.verify(&b, template),
        Err(e) => Outcome::deny(e.into(), LivenessReport::default(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_match_is_an_allow() {
        assert!(Verdict::Match { similarity: 0.9, agreeing: 2 }.is_allow());
        for deny in [
            Verdict::NoMatch { best: 0.9 },
            Verdict::NotLive { check: Check::DarkFrame, worst: 1.0, threshold: 1e-3 },
            Verdict::NoFace,
            Verdict::Environment { ambient_floor: 51.74 },
            Verdict::Unusable(BurstError::Malformed),
            Verdict::Failed("anything at all".into()),
        ] {
            assert!(!deny.is_allow(), "{deny:?} must not be an allow");
        }
    }

    /// A bright room must not be able to spend the user's failure budget, and a genuine
    /// biometric rejection must still count. Getting this backwards either locks people
    /// out for the weather or hands an attacker unlimited tries.
    #[test]
    fn only_outcomes_that_looked_at_a_face_count_as_attempts() {
        for counted in [
            Verdict::Match { similarity: 0.9, agreeing: 2 },
            Verdict::NoMatch { best: 0.2 },
            Verdict::NotLive { check: Check::DarkFrame, worst: 1.0, threshold: 1e-3 },
            Verdict::NoFace,
        ] {
            assert!(counted.counts_as_an_attempt(), "{counted:?} must count");
        }
        for exempt in [
            Verdict::Environment { ambient_floor: 51.74 },
            Verdict::Unusable(BurstError::Malformed),
            Verdict::Failed("camera vanished".into()),
        ] {
            assert!(!exempt.counts_as_an_attempt(), "{exempt:?} must not count");
        }
    }

    /// The gate's whole value is that it is reported as its own thing. If it collapsed
    /// into `Unusable` the audit log could not tell "too bright to measure" from "the
    /// capture was malformed", and Phase 4 could not exempt one without the other.
    #[test]
    fn a_bright_room_becomes_environment_and_not_unusable() {
        let v: Verdict = BurstError::AmbientTooBright { floor: 51.74 }.into();
        match v {
            Verdict::Environment { ambient_floor } => assert!((ambient_floor - 51.74).abs() < 1e-4),
            other => panic!("expected Environment, got {other:?}"),
        }
        assert!(matches!(
            Verdict::from(BurstError::Malformed),
            Verdict::Unusable(BurstError::Malformed)
        ));
    }

    /// A near-match is still a deny. The threshold is the boundary and nothing rounds up
    /// to success.
    #[test]
    fn a_verdict_carries_its_reason_without_softening_it() {
        let v = Verdict::NoMatch { best: 0.362 };
        assert!(!v.is_allow());
        match v {
            Verdict::NoMatch { best } => assert!(best < 0.363),
            _ => unreachable!(),
        }
    }
}

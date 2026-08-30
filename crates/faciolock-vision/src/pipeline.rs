use std::path::Path;

use crate::align::{self, Chip};
use crate::config::Config;
use crate::detect::{DetectError, Detection, Detector};
use crate::embed::{self, EmbedError, Embedder, Embedding, SFace, Template};
use crate::frame::{Burst, BurstError, Frame, Rect};
use crate::liveness::{self, Check, LivenessReport};

#[derive(Clone, Debug)]
pub enum Verdict {
    Match { similarity: f64, agreeing: usize },
    NoMatch { best: f64 },
    NotLive { check: Check, worst: f64, threshold: f64 },
    NoFace,
    Environment { ambient_floor: f32 },
    Unusable(BurstError),
    Failed(String),
}

impl Verdict {
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

    pub fn counts_as_an_attempt(&self) -> bool {
        match self {
            Verdict::Match { .. }
            | Verdict::NoMatch { .. }
            | Verdict::NotLive { .. }
            | Verdict::NoFace => true,
            Verdict::Environment { .. } | Verdict::Unusable(_) | Verdict::Failed(_) => false,
        }
    }
}

impl From<BurstError> for Verdict {
    fn from(e: BurstError) -> Verdict {
        match e {
            BurstError::AmbientTooBright { floor } => Verdict::Environment { ambient_floor: floor },
            other => Verdict::Unusable(other),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub verdict: Verdict,
    pub liveness: LivenessReport,
    pub face: Option<Rect>,
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

    pub fn chip(frame: &Frame<'_>, d: &Detection) -> Option<Chip> {
        align::align(frame, &d.landmarks)
    }
}

pub fn enroll_template(pipeline: &Pipeline, burst: &Burst<'_>) -> Result<Template, Outcome> {
    let embeddings = pipeline.enroll(burst)?;
    Template::new(pipeline.embedder_id(), embeddings).ok_or_else(|| {
        Outcome::deny(Verdict::NoFace, LivenessReport::default(), None)
    })
}

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

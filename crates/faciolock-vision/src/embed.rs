//! Face embedding and identity matching.
//!
//! **Matching aggregates k-of-n. Liveness aggregates conjunctively. They are opposite
//! rules over the same burst and must never be unified** — one bad frame here is a blink
//! or a blur and *should* be outvoted, whereas one contaminated frame in liveness is
//! evidence of an attack and must sink the burst
//! ([B9](../../../wiki/11-bug-log.md)).
//!
//! The [`Embedder`] trait is not speculative abstraction: it has already paid for itself
//! once. EdgeFace was the decided embedder until its *weights* turned out to be
//! CC BY-NC-SA rather than the repo's BSD-3, and the trait is what made that reversal a
//! configuration change instead of a rewrite ([B10](../../../wiki/11-bug-log.md)). SFace's
//! own training-data provenance is still uncleared, so a second reversal to dlib's CC0
//! model is a live possibility rather than a hypothetical.

use std::path::Path;
use std::sync::Arc;

use tract_onnx::prelude::*;

use crate::align::{Chip, CHIP_SIZE};
use crate::config::MatchConfig;

/// SFace's embedding width.
pub const SFACE_DIMS: usize = 128;

#[derive(Debug)]
pub enum EmbedError {
    Load(String),
    Run(String),
    /// The model produced an embedding of an unexpected width — a different model than the
    /// templates on disk were enrolled with.
    Dimensions { expected: usize, got: usize },
    /// The chip handed in is not the size the model was built for.
    ChipSize { expected: usize, got: usize },
}

impl std::fmt::Display for EmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmbedError::Load(e) => write!(f, "model load failed: {e}"),
            EmbedError::Run(e) => write!(f, "inference failed: {e}"),
            EmbedError::Dimensions { expected, got } => {
                write!(f, "embedding is {got}-d, expected {expected}-d")
            }
            EmbedError::ChipSize { expected, got } => {
                write!(f, "chip is {got}px, model expects {expected}px")
            }
        }
    }
}

/// An L2-normalised face embedding.
///
/// Normalised at construction so cosine similarity is a plain dot product and no caller
/// can forget. An unnormalised embedding compared against a normalised one produces a
/// number that looks like a similarity and is not one.
#[derive(Clone, PartialEq, Debug)]
pub struct Embedding {
    values: Vec<f32>,
}

impl Embedding {
    /// `None` for a zero vector, which has no direction and therefore no similarity to
    /// anything.
    pub fn new(mut values: Vec<f32>) -> Option<Embedding> {
        let norm = values.iter().map(|v| v * v).sum::<f32>().sqrt();
        if !norm.is_finite() || norm <= f32::EPSILON {
            return None;
        }
        for v in &mut values {
            *v /= norm;
        }
        Some(Embedding { values })
    }

    pub fn as_slice(&self) -> &[f32] {
        &self.values
    }

    pub fn dims(&self) -> usize {
        self.values.len()
    }

    /// Cosine similarity. Both operands are unit vectors, so this is their dot product.
    ///
    /// Returns 0 for a dimension mismatch rather than panicking: a template enrolled under
    /// a different embedder must fail to match, not take down the daemon.
    ///
    /// Not constant-time, and deliberately not pretending to be. Timing hygiene on a float
    /// distance is not meaningful; the mitigation against threshold-probing is the rate
    /// limiter in wiki/03, which is Phase 4's job.
    pub fn similarity(&self, other: &Embedding) -> f64 {
        if self.values.len() != other.values.len() {
            return 0.0;
        }
        self.values.iter().zip(&other.values).map(|(a, b)| (a * b) as f64).sum()
    }
}

/// Anything that turns an aligned chip into an embedding.
pub trait Embedder {
    fn embed(&self, chip: &Chip) -> Result<Embedding, EmbedError>;
    fn dims(&self) -> usize;
    /// Short identifier recorded alongside a template, so a template enrolled with one
    /// embedder is never silently compared under another.
    fn id(&self) -> &'static str;
}

pub struct SFace {
    model: Arc<TypedSimplePlan>,
}

impl SFace {
    pub fn load(path: &Path) -> Result<SFace, EmbedError> {
        let model = tract_onnx::onnx()
            .model_for_path(path)
            .map_err(|e| EmbedError::Load(format!("{e:?}")))?
            .with_input_fact(0, f32::fact([1, 3, CHIP_SIZE, CHIP_SIZE]).into())
            .map_err(|e| EmbedError::Load(format!("{e:?}")))?
            .into_optimized()
            .map_err(|e| EmbedError::Load(format!("{e:?}")))?
            .into_runnable()
            .map_err(|e| EmbedError::Load(format!("{e:?}")))?;
        Ok(SFace { model })
    }
}

impl Embedder for SFace {
    fn embed(&self, chip: &Chip) -> Result<Embedding, EmbedError> {
        if chip.size != CHIP_SIZE {
            return Err(EmbedError::ChipSize { expected: CHIP_SIZE, got: chip.size });
        }

        // Raw 0-255, three identical channels. SFace carries its own Sub and Mul
        // normalisation ops at the head of the graph, so scaling here would apply it
        // twice. Greyscale replicated across the channels because the model wants three
        // and an IR sensor gives one — channel order is irrelevant when they are equal,
        // which sidesteps the BGR-versus-RGB question entirely.
        let mut data = Vec::with_capacity(3 * CHIP_SIZE * CHIP_SIZE);
        for _ in 0..3 {
            data.extend(chip.pixels.iter().map(|&p| p as f32));
        }
        let input = Tensor::from_shape(&[1, 3, CHIP_SIZE, CHIP_SIZE], &data)
            .map_err(|e| EmbedError::Run(format!("{e:?}")))?;

        let outputs =
            self.model.run(tvec!(input.into())).map_err(|e| EmbedError::Run(format!("{e:?}")))?;
        let raw = outputs
            .first()
            .ok_or_else(|| EmbedError::Run("model produced no output".into()))?
            .view()
            .as_slice::<f32>()
            .map_err(|e| EmbedError::Run(format!("{e:?}")))?
            .to_vec();

        if raw.len() != SFACE_DIMS {
            return Err(EmbedError::Dimensions { expected: SFACE_DIMS, got: raw.len() });
        }
        Embedding::new(raw).ok_or(EmbedError::Dimensions { expected: SFACE_DIMS, got: 0 })
    }

    fn dims(&self) -> usize {
        SFACE_DIMS
    }

    fn id(&self) -> &'static str {
        "sface-2021dec"
    }
}

/// What a user enrolled: several embeddings, matched best-of.
///
/// Multiple templates per user is not a nicety. Requiring one canonical face — glasses on
/// or off, one lighting condition, one distance — is the main source of day-to-day false
/// rejects, which is the failure mode that makes people switch face unlock off.
#[derive(Clone, PartialEq, Debug)]
pub struct Template {
    pub embedder: String,
    embeddings: Vec<Embedding>,
}

impl Template {
    /// `None` for an empty enrolment: a template that can match nothing must not exist,
    /// because `best_similarity` over no embeddings has no honest answer.
    pub fn new(embedder: &str, embeddings: Vec<Embedding>) -> Option<Template> {
        if embeddings.is_empty() {
            return None;
        }
        Some(Template { embedder: embedder.to_string(), embeddings })
    }

    pub fn len(&self) -> usize {
        self.embeddings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.embeddings.is_empty()
    }

    /// Similarity to the closest enrolled embedding.
    pub fn best_similarity(&self, probe: &Embedding) -> f64 {
        self.embeddings
            .iter()
            .map(|e| e.similarity(probe))
            .fold(f64::NEG_INFINITY, f64::max)
    }
}

/// The result of comparing a burst's embeddings against a template.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MatchOutcome {
    /// How many probes cleared the threshold.
    pub agreeing: usize,
    /// How many probes were compared. May be fewer than `agree_n` if frames were unusable.
    pub compared: usize,
    /// The strongest similarity seen, for the audit log.
    pub best: f64,
    pub matched: bool,
}

/// k-of-n agreement against an enrolled template.
///
/// Requires `agree_k` of the probes to clear the threshold independently. A probe set
/// smaller than `agree_k` cannot match at all — the same vacuous-aggregation trap liveness
/// guards against, arriving from the other direction: "2 of the 0 frames we managed to
/// embed agreed" must not be a success.
pub fn match_k_of_n(template: &Template, probes: &[Embedding], cfg: &MatchConfig) -> MatchOutcome {
    let mut agreeing = 0usize;
    let mut best = f64::NEG_INFINITY;

    for probe in probes {
        let s = template.best_similarity(probe);
        best = best.max(s);
        if s >= cfg.cosine_min {
            agreeing += 1;
        }
    }

    MatchOutcome {
        agreeing,
        compared: probes.len(),
        best: if best.is_finite() { best } else { 0.0 },
        matched: probes.len() >= cfg.agree_k && agreeing >= cfg.agree_k,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emb(v: &[f32]) -> Embedding {
        Embedding::new(v.to_vec()).expect("non-zero")
    }

    fn cfg() -> MatchConfig {
        MatchConfig::default()
    }

    #[test]
    fn embeddings_are_unit_length_and_cosine_is_a_dot_product() {
        let a = emb(&[3.0, 4.0, 0.0]);
        assert!((a.as_slice().iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-6);
        assert!((a.similarity(&a) - 1.0).abs() < 1e-6);

        let b = emb(&[0.0, 0.0, 5.0]);
        assert!(a.similarity(&b).abs() < 1e-6, "orthogonal vectors are dissimilar");

        let c = emb(&[-3.0, -4.0, 0.0]);
        assert!((a.similarity(&c) + 1.0).abs() < 1e-6, "opposites sit at -1");
    }

    #[test]
    fn a_zero_vector_is_not_an_embedding() {
        assert!(Embedding::new(vec![0.0; 128]).is_none());
        assert!(Embedding::new(vec![f32::NAN, 1.0]).is_none());
    }

    /// A template enrolled under one embedder must not produce a plausible-looking score
    /// against another. Returning 0 rather than panicking keeps the daemon alive and the
    /// verdict a deny.
    #[test]
    fn embeddings_of_different_widths_never_match() {
        assert_eq!(emb(&[1.0, 0.0]).similarity(&emb(&[1.0, 0.0, 0.0])), 0.0);
    }

    #[test]
    fn an_empty_enrolment_cannot_become_a_template() {
        assert!(Template::new("sface", Vec::new()).is_none());
    }

    #[test]
    fn best_of_n_templates_picks_the_closest_enrolled_face() {
        // Glasses on and glasses off, say.
        let t = Template::new("sface", vec![emb(&[1.0, 0.0, 0.0]), emb(&[0.0, 1.0, 0.0])]).unwrap();
        assert!((t.best_similarity(&emb(&[0.0, 1.0, 0.0])) - 1.0).abs() < 1e-6);
        assert!(t.best_similarity(&emb(&[0.0, 0.0, 1.0])).abs() < 1e-6);
    }

    /// The point of k-of-n: one bad frame must be outvoted, not fatal.
    #[test]
    fn one_bad_frame_out_of_three_still_matches() {
        let t = Template::new("sface", vec![emb(&[1.0, 0.0, 0.0])]).unwrap();
        let probes = vec![
            emb(&[1.0, 0.02, 0.0]),  // good
            emb(&[0.0, 1.0, 0.0]),   // a blink: nothing like the template
            emb(&[0.99, 0.05, 0.0]), // good
        ];
        let out = match_k_of_n(&t, &probes, &cfg());
        assert!(out.matched, "{out:?}");
        assert_eq!(out.agreeing, 2);
        assert_eq!(out.compared, 3);
    }

    #[test]
    fn two_bad_frames_out_of_three_do_not_match() {
        let t = Template::new("sface", vec![emb(&[1.0, 0.0, 0.0])]).unwrap();
        let probes =
            vec![emb(&[1.0, 0.02, 0.0]), emb(&[0.0, 1.0, 0.0]), emb(&[0.0, 0.0, 1.0])];
        let out = match_k_of_n(&t, &probes, &cfg());
        assert!(!out.matched, "{out:?}");
        assert_eq!(out.agreeing, 1);
    }

    /// The vacuous-aggregation trap from the matching side. "k of the zero frames we
    /// embedded agreed" is not a success, and neither is one frame agreeing when the
    /// policy asks for two.
    #[test]
    fn fewer_probes_than_k_can_never_match() {
        let t = Template::new("sface", vec![emb(&[1.0, 0.0, 0.0])]).unwrap();
        let none: Vec<Embedding> = Vec::new();
        assert!(!match_k_of_n(&t, &none, &cfg()).matched);

        let one = vec![emb(&[1.0, 0.0, 0.0])];
        let out = match_k_of_n(&t, &one, &cfg());
        assert_eq!(out.agreeing, 1, "it did agree");
        assert!(!out.matched, "but one agreeing probe is below k={}", cfg().agree_k);
    }

    #[test]
    fn a_stranger_does_not_match_however_many_frames_agree() {
        let t = Template::new("sface", vec![emb(&[1.0, 0.0, 0.0])]).unwrap();
        let probes = vec![emb(&[0.0, 1.0, 0.0]); 3];
        let out = match_k_of_n(&t, &probes, &cfg());
        assert!(!out.matched);
        assert_eq!(out.agreeing, 0);
        assert!(out.best < cfg().cosine_min);
    }
}

use std::path::Path;
use std::sync::Arc;

use tract_onnx::prelude::*;

use crate::align::{Chip, CHIP_SIZE};
use crate::config::MatchConfig;

pub const SFACE_DIMS: usize = 128;

#[derive(Debug)]
pub enum EmbedError {
    Load(String),
    Run(String),
    Dimensions { expected: usize, got: usize },
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

#[derive(Clone, PartialEq, Debug)]
pub struct Embedding {
    values: Vec<f32>,
}

impl Embedding {
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

    pub fn similarity(&self, other: &Embedding) -> f64 {
        if self.values.len() != other.values.len() {
            return 0.0;
        }
        self.values.iter().zip(&other.values).map(|(a, b)| (a * b) as f64).sum()
    }
}

pub trait Embedder {
    fn embed(&self, chip: &Chip) -> Result<Embedding, EmbedError>;
    fn dims(&self) -> usize;
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

#[derive(Clone, PartialEq, Debug)]
pub struct Template {
    pub embedder: String,
    embeddings: Vec<Embedding>,
}

impl Template {
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

    pub fn embeddings(&self) -> &[Embedding] {
        &self.embeddings
    }

    pub fn best_similarity(&self, probe: &Embedding) -> f64 {
        self.embeddings
            .iter()
            .map(|e| e.similarity(probe))
            .fold(f64::NEG_INFINITY, f64::max)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MatchOutcome {
    pub agreeing: usize,
    pub compared: usize,
    pub best: f64,
    pub matched: bool,
}

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
        let t = Template::new("sface", vec![emb(&[1.0, 0.0, 0.0]), emb(&[0.0, 1.0, 0.0])]).unwrap();
        assert!((t.best_similarity(&emb(&[0.0, 1.0, 0.0])) - 1.0).abs() < 1e-6);
        assert!(t.best_similarity(&emb(&[0.0, 0.0, 1.0])).abs() < 1e-6);
    }

    #[test]
    fn one_bad_frame_out_of_three_still_matches() {
        let t = Template::new("sface", vec![emb(&[1.0, 0.0, 0.0])]).unwrap();
        let probes = vec![
            emb(&[1.0, 0.02, 0.0]),
            emb(&[0.0, 1.0, 0.0]),
            emb(&[0.99, 0.05, 0.0]),
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

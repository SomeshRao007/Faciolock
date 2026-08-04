//! Frames in, verdict out.
//!
//! This crate knows nothing about V4L2, sockets, or PAM. That boundary is the reason the
//! security claims in `wiki/05` are falsifiable at all: the entire security-relevant path
//! can be replayed headlessly against recorded and synthetic captures, with no camera
//! attached. Unfalsifiable security claims are worth nothing (`wiki/04`).
//!
//! Two rules run in opposite directions over the same burst and **must not be unified**
//! ([B9](../../../wiki/11-bug-log.md)):
//!
//! | | rule | why |
//! |---|---|---|
//! | Liveness | **every** pair must pass | one contaminated frame is evidence of an attack, and evidence does not average out |
//! | Matching | k of n must agree | one bad frame is a blink or a blur, and flukes *should* average out |

pub mod align;
pub mod config;
pub mod detect;
pub mod embed;
pub mod frame;
pub mod liveness;
pub mod pipeline;
pub mod testing;

pub use align::{Chip, Similarity};
pub use config::{Config, Enforcement, Threshold, Tier};
pub use detect::{DetectError, Detection, Detector};
pub use embed::{Embedder, Embedding, EmbedError, MatchOutcome, SFace, Template};
pub use frame::{Burst, BurstError, Frame, Observed, Pair, Phase, PixelRect, Rect};
pub use liveness::{Check, CheckResult, LivenessReport};
pub use pipeline::{Outcome, Pipeline, Verdict};

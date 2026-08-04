//! Locating the corpus and the models, and being loud when they are missing.
//!
//! The real Phase 0 recordings are the operator's face and are gitignored; the ONNX
//! models are large binaries fetched from upstream and are gitignored too. So a test
//! that needs either of them can be run in an environment that has neither.
//!
//! **A test that silently passes when its input is absent is worse than no test at all.**
//! That is [B3](../../../../wiki/11-bug-log.md) exactly — a redirect turned a missing
//! binary into "no evidence", and "no evidence" reads like "nothing wrong". Every skip
//! here prints a banner naming what was missing, and setting `FACIOLOCK_CORPUS_REQUIRED=1`
//! turns a skip into a failure, which is how CI will run once corpus provisioning is
//! settled.

// Each integration-test binary compiles this module separately and uses a different
// subset of it, so anything one binary does not call reads as dead there.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/faciolock-vision.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

pub fn models_dir() -> PathBuf {
    repo_root().join("models")
}

pub fn corpus_dir() -> PathBuf {
    repo_root().join("corpus")
}

pub fn yunet() -> PathBuf {
    models_dir().join("face_detection_yunet_2026may.onnx")
}

pub fn sface() -> PathBuf {
    models_dir().join("face_recognition_sface_2021dec.onnx")
}

fn required() -> bool {
    std::env::var("FACIOLOCK_CORPUS_REQUIRED").is_ok_and(|v| v == "1")
}

/// Announce that a test could not run, and fail instead if this environment demands the
/// inputs be present.
pub fn skip(what: &str, path: &Path) -> bool {
    let msg = format!(
        "\n  ####  SKIPPED — DEGRADED RUN  ####\n  \
         missing {what}: {}\n  \
         This test asserted nothing. Set FACIOLOCK_CORPUS_REQUIRED=1 to make this fatal.\n",
        path.display()
    );
    if required() {
        panic!("{msg}");
    }
    eprintln!("{msg}");
    false
}

/// A `.gray` capture plus the geometry from its sidecar.
pub struct Capture {
    /// Filename stem, e.g. `live_lit_frontal_20260803T183134Z`. **Unique**; `label` is
    /// not — two takes of the same scene share a label and differ only by timestamp.
    pub stem: String,
    pub label: String,
    pub bytes: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

impl Capture {
    pub fn frame(&self, i: usize) -> &[u8] {
        let stride = self.width * self.height;
        &self.bytes[i * stride..(i + 1) * stride]
    }

    pub fn frame_count(&self) -> usize {
        self.bytes.len() / (self.width * self.height)
    }
}

/// Every recording in the corpus that has both a sidecar and its pixels.
///
/// Driven by the sidecars, which *are* committed. A sidecar with no `.gray` beside it is
/// reported rather than ignored — [B4](../../../../wiki/11-bug-log.md) is the converse
/// case, a capture with no sidecar, and both directions leave the corpus lying about what
/// it contains.
pub fn captures() -> Vec<Capture> {
    let dir = corpus_dir();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        skip("corpus directory", &dir);
        return out;
    };

    let mut sidecars: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    sidecars.sort();

    for sidecar in sidecars {
        let gray = sidecar.with_extension("gray");
        if !gray.exists() {
            skip("capture pixels", &gray);
            continue;
        }
        let Ok(meta) = std::fs::read_to_string(&sidecar) else { continue };
        let Some(width) = json_usize(&meta, "width") else { continue };
        let Some(height) = json_usize(&meta, "height") else { continue };
        let Ok(bytes) = std::fs::read(&gray) else { continue };
        let label = json_str(&meta, "label")
            .unwrap_or_else(|| gray.file_stem().unwrap().to_string_lossy().to_string());
        let stem = gray.file_stem().unwrap().to_string_lossy().to_string();
        out.push(Capture { stem, label, bytes, width, height });
    }
    out
}

/// Enough JSON to read a flat sidecar. Not a parser, and not pretending to be one — the
/// sidecars are written by `tools/record.sh` and have three levels of nothing.
fn json_usize(s: &str, key: &str) -> Option<usize> {
    let at = s.find(&format!("\"{key}\""))?;
    let rest = &s[at + key.len() + 2..];
    let colon = rest.find(':')?;
    rest[colon + 1..]
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

fn json_str(s: &str, key: &str) -> Option<String> {
    let at = s.find(&format!("\"{key}\""))?;
    let rest = &s[at + key.len() + 2..];
    let colon = rest.find(':')?;
    let open = rest[colon..].find('"')? + colon + 1;
    let close = rest[open..].find('"')? + open;
    Some(rest[open..close].to_string())
}

//! Model integrity, checked once at startup.
//!
//! The daemon feeds these files to `tract`, which is a parser reading attacker-adjacent
//! bytes on behalf of every authentication on the machine. wiki/09's Phase 2 line asks for
//! SHA256 verification at startup; this is it.
//!
//! **What this does and does not buy.** It detects a model swapped after install — the
//! interesting case, since a substituted embedder is an unlock for whoever substituted it.
//! It says nothing about provenance: the manifest pins the bytes we shipped, and the
//! argument that *those* bytes are the right ones is licensing and packaging work
//! (wiki/05, wiki/07), not a hash.
//!
//! **Fail closed.** A missing manifest, an unreadable model or a mismatch all refuse to
//! start. A daemon that shrugs at an unverifiable model is worse than no verification,
//! because the log then says it checked.

use std::path::Path;

use sha2::{Digest, Sha256};

/// The manifest lives beside the models, in `sha256sum` format: `<64 hex>  <filename>`.
pub const MANIFEST: &str = "MANIFEST.sha256";

/// The two files `faciolock_vision::Pipeline::load` actually opens.
///
/// Listed rather than "every `.onnx` in the directory" so the manifest cannot pass by
/// covering files nobody loads while missing one we do.
pub const REQUIRED: [&str; 2] =
    ["face_detection_yunet_2026may.onnx", "face_recognition_sface_2021dec.onnx"];

pub fn verify(dir: &Path) -> Result<(), String> {
    let manifest_path = dir.join(MANIFEST);
    let manifest = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;

    let mut checked = Vec::new();
    for (n, line) in manifest.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let (Some(want), Some(name)) = (parts.next(), parts.next()) else {
            return Err(format!("{}:{}: not a sha256sum line", MANIFEST, n + 1));
        };

        // A manifest entry naming a path rather than a filename would let one reach outside
        // the model directory, so the name is a name.
        if name.contains('/') {
            return Err(format!("{}:{}: entry names a path, not a file", MANIFEST, n + 1));
        }

        let path = dir.join(name);
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let got = hex(&Sha256::digest(&bytes));
        if !got.eq_ignore_ascii_case(want) {
            return Err(format!("{name}: sha256 {got}, manifest says {want}"));
        }
        checked.push(name.to_string());
    }

    for required in REQUIRED {
        if !checked.iter().any(|c| c == required) {
            return Err(format!("{MANIFEST} does not cover {required}"));
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property worth a test is the one that fails closed: a flipped byte must stop the
    /// daemon, and a manifest that does not mention a model we load must too.
    #[test]
    fn a_modified_model_and_an_incomplete_manifest_both_refuse() {
        let dir = std::env::temp_dir().join("faciolockd-models-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmpdir");

        for name in REQUIRED {
            std::fs::write(dir.join(name), name.as_bytes()).expect("stub model");
        }
        let manifest: String = REQUIRED
            .iter()
            .map(|n| format!("{}  {n}\n", hex(&Sha256::digest(n.as_bytes()))))
            .collect();
        std::fs::write(dir.join(MANIFEST), &manifest).expect("manifest");
        assert!(verify(&dir).is_ok(), "matching hashes must verify");

        std::fs::write(dir.join(REQUIRED[0]), b"tampered").expect("rewrite");
        assert!(verify(&dir).is_err(), "a changed model must refuse to start");

        std::fs::write(dir.join(REQUIRED[0]), REQUIRED[0].as_bytes()).expect("restore");
        let partial: String =
            format!("{}  {}\n", hex(&Sha256::digest(REQUIRED[1].as_bytes())), REQUIRED[1]);
        std::fs::write(dir.join(MANIFEST), partial).expect("manifest");
        assert!(verify(&dir).is_err(), "a manifest missing a loaded model must refuse");
    }
}

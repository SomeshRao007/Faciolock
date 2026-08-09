//! Enrolled templates on disk.
//!
//! ponytail: one file per username, 0600, plaintext. **Phase 4 replaces all of it** —
//! keyed by uid rather than by name, sealed to the TPM, with the non-TPM fallback and the
//! documented re-enrol path (wiki/09). Nothing here should be mistaken for that: a
//! plaintext template file copied to another machine works there, which is precisely the
//! property Phase 4's exit criterion says must not hold.
//!
//! The format is written by hand rather than through `serde` because
//! [`Embedding`](faciolock_vision::Embedding) keeps its values private and normalises at
//! construction, and going in through the public constructor is what keeps a hand-edited
//! file from producing an "embedding" that is not a unit vector.
//!
//! ```text
//! "FLT1" | embedder_len u16 | embedder utf8 | count u16 | dims u16 | f32le * dims * count
//! ```

use std::io;
use std::path::{Path, PathBuf};

use faciolock_vision::{Embedding, Template};

const MAGIC: &[u8; 4] = b"FLT1";

fn path_for(dir: &Path, username: &str) -> PathBuf {
    dir.join(format!("{username}.tpl"))
}

/// `Ok(None)` for a user who has not enrolled — a normal state, not an error.
///
/// A file that exists and does not parse **is** an error: it means the store is damaged or
/// has been tampered with, and answering "not enrolled" would quietly downgrade that to a
/// password prompt with nothing in the journal.
pub fn load(dir: &Path, username: &str) -> Result<Option<Template>, String> {
    let path = path_for(dir, username);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    decode(&bytes).map(Some).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn save(dir: &Path, username: &str, template: &Template) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = path_for(dir, username);
    std::fs::write(&path, encode(template)).map_err(|e| format!("{}: {e}", path.display()))?;
    restrict(&path)?;
    Ok(path)
}

/// 0600 after the write. A template is not a password, but it is a biometric identifier and
/// it is the input to the only comparison that returns `Allow`.
fn restrict(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn encode(template: &Template) -> Vec<u8> {
    let embeddings = template.embeddings();
    let dims = embeddings.first().map_or(0, |e| e.dims());
    let name = template.embedder.as_bytes();

    let mut out = Vec::with_capacity(10 + name.len() + embeddings.len() * dims * 4);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(&(embeddings.len() as u16).to_le_bytes());
    out.extend_from_slice(&(dims as u16).to_le_bytes());
    for e in embeddings {
        for v in e.as_slice() {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

/// Every length is bounds-checked against the buffer before it is used to slice it. The
/// store is not peer-controlled the way the socket is, but it is a file, and a file is
/// something a bug elsewhere can truncate.
fn decode(bytes: &[u8]) -> Result<Template, String> {
    let mut at = 0usize;
    let mut take = |n: usize| -> Result<&[u8], String> {
        let end = at.checked_add(n).ok_or("length overflow")?;
        let slice = bytes.get(at..end).ok_or("truncated template")?;
        at = end;
        Ok(slice)
    };

    if take(4)? != MAGIC {
        return Err("not a faciolock template".into());
    }
    let name_len = u16::from_le_bytes(take(2)?.try_into().expect("2 bytes")) as usize;
    let embedder = std::str::from_utf8(take(name_len)?)
        .map_err(|_| "embedder id is not utf-8")?
        .to_string();
    let count = u16::from_le_bytes(take(2)?.try_into().expect("2 bytes")) as usize;
    let dims = u16::from_le_bytes(take(2)?.try_into().expect("2 bytes")) as usize;

    let mut embeddings = Vec::with_capacity(count);
    for _ in 0..count {
        let raw = take(dims * 4)?;
        let values: Vec<f32> = raw
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes(c.try_into().expect("4 bytes")))
            .collect();
        embeddings.push(Embedding::new(values).ok_or("template holds a zero embedding")?);
    }
    if at != bytes.len() {
        return Err("trailing bytes after the template".into());
    }
    Template::new(&embedder, embeddings).ok_or_else(|| "template holds no embeddings".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template() -> Template {
        let a = Embedding::new((0..128).map(|i| i as f32 - 60.0).collect()).expect("nonzero");
        let b = Embedding::new((0..128).map(|i| (i % 7) as f32 + 1.0).collect()).expect("nonzero");
        Template::new("sface-2021dec", vec![a, b]).expect("nonempty")
    }

    /// Round-trip on *similarity*, not on bytes. `Embedding::new` re-normalises on load and
    /// an f32 unit vector's norm is not exactly 1.0, so an equality assertion here would be
    /// testing float bit patterns rather than the property that matters: the template that
    /// comes back matches the one that went in.
    #[test]
    fn a_template_survives_the_disk() {
        let dir = std::env::temp_dir().join("faciolockd-store-test");
        let _ = std::fs::remove_dir_all(&dir);
        let original = template();

        assert!(load(&dir, "csr").expect("readable").is_none(), "no file is not enrolled");
        save(&dir, "csr", &original).expect("saves");
        let back = load(&dir, "csr").expect("readable").expect("enrolled");

        assert_eq!(back.len(), original.len());
        assert_eq!(back.embedder, original.embedder);
        for e in original.embeddings() {
            assert!(
                (back.best_similarity(e) - 1.0).abs() < 1e-6,
                "an embedding did not survive the round trip"
            );
        }
    }

    /// A damaged store must be an error, never "not enrolled" — the latter falls through to
    /// a password prompt with nothing said about why.
    #[test]
    fn a_truncated_file_is_an_error_not_a_missing_enrolment() {
        let dir = std::env::temp_dir().join("faciolockd-store-trunc");
        let _ = std::fs::remove_dir_all(&dir);
        save(&dir, "csr", &template()).expect("saves");

        let path = path_for(&dir, "csr");
        let full = std::fs::read(&path).expect("read");
        std::fs::write(&path, &full[..full.len() - 9]).expect("truncate");
        assert!(load(&dir, "csr").is_err(), "a truncated template must not read as absent");

        std::fs::write(&path, [full.clone(), vec![0u8]].concat()).expect("extend");
        assert!(load(&dir, "csr").is_err(), "trailing bytes must not be ignored");
    }
}

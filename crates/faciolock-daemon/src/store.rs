//! Enrolled templates on disk: encrypted, authenticated, and keyed by uid.
//!
//! Phase 4 replaced the Phase 2 store wholesale. What it was — one plaintext file per
//! *username* — failed wiki/03's A3 (offline disk attacker) completely: a copied `.tpl`
//! worked on any machine, and an attacker who could write one could implant their own face.
//!
//! ```text
//! "FLT2" | key_provider u8 | nonce[24] | XChaCha20-Poly1305(inner, aad)
//!   aad   = "FLT2" || key_provider || uid as u32 le
//!   inner = "FLT1" | embedder_len u16 | embedder utf8 | count u16 | dims u16 | f32le * dims * count
//! ```
//!
//! Three properties, and each is a test below:
//!
//! - **A file alone is worthless.** Without the key it does not decrypt, which is the whole
//!   of Phase 4's first exit criterion.
//! - **A file cannot be moved between users.** The uid is authenticated as associated data,
//!   not merely stored in the filename, so renaming `1000.tpl` to `0.tpl` produces a
//!   decryption failure rather than root's enrolment.
//! - **A file cannot be edited.** AEAD means a single flipped bit is a hard error, not a
//!   subtly different face.
//!
//! **The key is a file, not the TPM** (decision P4-a, wiki/08 Q5). The reduced guarantee is
//! stated plainly in wiki/03 R6: this stops someone who copied the template, and does not
//! stop someone who imaged the whole disk. [`KeyProvider`] carries a discriminant on the
//! wire format so a TPM-sealed variant is a new value rather than a new format.
//!
//! The inner encoding is written by hand rather than through `serde` because
//! [`Embedding`](faciolock_vision::Embedding) keeps its values private and normalises at
//! construction, and going in through the public constructor is what keeps a hand-edited
//! file from producing an "embedding" that is not a unit vector.

use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use faciolock_vision::{Embedding, Template};

const MAGIC: &[u8; 4] = b"FLT2";
const INNER_MAGIC: &[u8; 4] = b"FLT1";
const NONCE_LEN: usize = 24;
const KEY_LEN: usize = 32;

/// Where the file-backed key lives, relative to the template directory.
const KEY_FILE: &str = "key";

/// Which safe holds the key that decrypts a template.
///
/// One variant today. It is an enum rather than a bare constant because the discriminant is
/// on disk and inside the AEAD's associated data: adding `Tpm = 1` later is a new file that
/// old code refuses, rather than a format change that old code silently misreads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyProvider {
    /// A 32-byte file next to the templates, 0600. wiki/03 R6's documented fallback,
    /// promoted to the default for v1 by P4-a.
    File = 0,
}

impl KeyProvider {
    /// Load the key, creating it on first use.
    ///
    /// `create_new` + `mode` rather than write-then-`chmod` or a `umask`: the key must never
    /// exist at a looser mode, not even for the width of one syscall, and B23 is what
    /// happens when `umask` is used to buy that — it is *process* state, so it silently
    /// applies to every other thread's file creation too.
    ///
    /// `create_new` also makes the creation the atomic step, so two daemons starting at once
    /// cannot each generate a key and leave one of them holding a template nothing can read.
    fn key(&self, dir: &Path) -> Result<[u8; KEY_LEN], String> {
        let path = dir.join(KEY_FILE);
        match std::fs::read(&path) {
            Ok(bytes) => bytes
                .try_into()
                .map_err(|_| format!("{}: not a {KEY_LEN}-byte key", path.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                let fresh = random_bytes::<KEY_LEN>()?;
                match std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&path)
                {
                    Ok(mut f) => {
                        use std::io::Write;
                        f.write_all(&fresh).map_err(|e| format!("{}: {e}", path.display()))?;
                        f.sync_all().map_err(|e| format!("{}: {e}", path.display()))?;
                        Ok(fresh)
                    }
                    // Lost the race. The winner's key is the real one, so read it back
                    // rather than overwriting and orphaning whatever they just sealed.
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        std::fs::read(&path)
                            .map_err(|e| format!("{}: {e}", path.display()))?
                            .try_into()
                            .map_err(|_| format!("{}: not a {KEY_LEN}-byte key", path.display()))
                    }
                    Err(e) => Err(format!("{}: {e}", path.display())),
                }
            }
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

/// `getrandom(2)` straight through `libc`.
///
/// The `getrandom` crate would be the ladder's answer, but the daemon already links `libc`
/// and this is the whole of what that crate would be used for. The loop is not decoration:
/// the syscall is documented to return short on a signal, and a short read here would mean a
/// key with predictable trailing bytes.
fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut out = [0u8; N];
    let mut filled = 0;
    while filled < N {
        let got = unsafe {
            libc::getrandom(out[filled..].as_mut_ptr() as *mut libc::c_void, N - filled, 0)
        };
        if got < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("getrandom: {e}"));
        }
        filled += got as usize;
    }
    Ok(out)
}

fn path_for(dir: &Path, uid: u32) -> PathBuf {
    dir.join(format!("{uid}.tpl"))
}

/// What the ciphertext is bound to. Changing any of it makes the file undecryptable.
fn aad(provider: KeyProvider, uid: u32) -> [u8; 9] {
    let mut out = [0u8; 9];
    out[..4].copy_from_slice(MAGIC);
    out[4] = provider as u8;
    out[5..].copy_from_slice(&uid.to_le_bytes());
    out
}

/// `Ok(None)` for a user who has not enrolled — a normal state, not an error.
///
/// A file that exists and does not decrypt **is** an error: it means the store is damaged or
/// has been tampered with, and answering "not enrolled" would quietly downgrade that to a
/// password prompt with nothing in the journal.
pub fn load(dir: &Path, uid: u32) -> Result<Option<Template>, String> {
    let path = path_for(dir, uid);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    open(dir, uid, &bytes).map(Some).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn save(dir: &Path, uid: u32, template: &Template) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = path_for(dir, uid);
    let sealed = seal(dir, uid, template)?;

    // 0600 at creation, and the file is written under a private name and renamed into place
    // so a reader never sees a half-written template — the same discipline `serve::listen`
    // uses for the socket, and for the same reason.
    let staging = dir.join(format!("{uid}.tpl.new.{}", std::process::id()));
    let write = || -> Result<(), String> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&staging)
            .map_err(|e| format!("{}: {e}", staging.display()))?;
        f.write_all(&sealed).map_err(|e| format!("{}: {e}", staging.display()))?;
        f.sync_all().map_err(|e| format!("{}: {e}", staging.display()))
    };
    if let Err(e) = write().and_then(|()| {
        std::fs::rename(&staging, &path).map_err(|e| format!("rename: {e}"))
    }) {
        let _ = std::fs::remove_file(&staging);
        return Err(e);
    }
    Ok(path)
}

/// Encrypt a template for one uid. Split out from [`save`] so the crypto is testable
/// without the filesystem dance around it.
fn seal(dir: &Path, uid: u32, template: &Template) -> Result<Vec<u8>, String> {
    let provider = KeyProvider::File;
    let key = provider.key(dir)?;
    let nonce = random_bytes::<NONCE_LEN>()?;
    let plaintext = encode(template);

    let cipher = XChaCha20Poly1305::new((&key).into());
    let aad = aad(provider, uid);
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: &plaintext, aad: &aad })
        // The error type is deliberately opaque upstream and there is nothing here a
        // message could add: encryption fails only on a length no template can reach.
        .map_err(|_| "could not encrypt the template".to_string())?;

    let mut out = Vec::with_capacity(MAGIC.len() + 1 + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.push(provider as u8);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypt and decode. Every failure is one message: a decryption that failed says nothing
/// about *why*, deliberately, because the reasons are "wrong key", "wrong user" and
/// "tampered with" and none of them is safe to distinguish for a caller.
fn open(dir: &Path, uid: u32, bytes: &[u8]) -> Result<Template, String> {
    let head = MAGIC.len() + 1 + NONCE_LEN;
    if bytes.len() < head {
        return Err("truncated template".into());
    }
    if &bytes[..4] != MAGIC {
        return Err("not a faciolock template".into());
    }
    let provider = match bytes[4] {
        0 => KeyProvider::File,
        other => return Err(format!("unknown key provider {other}")),
    };
    let key = provider.key(dir)?;

    let cipher = XChaCha20Poly1305::new((&key).into());
    let aad = aad(provider, uid);
    let plaintext = cipher
        .decrypt(
            XNonce::from_slice(&bytes[5..head]),
            Payload { msg: &bytes[head..], aad: &aad },
        )
        .map_err(|_| "the template does not decrypt for this machine and user".to_string())?;
    decode(&plaintext)
}

fn encode(template: &Template) -> Vec<u8> {
    let embeddings = template.embeddings();
    let dims = embeddings.first().map_or(0, |e| e.dims());
    let name = template.embedder.as_bytes();

    let mut out = Vec::with_capacity(10 + name.len() + embeddings.len() * dims * 4);
    out.extend_from_slice(INNER_MAGIC);
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

/// Every length is bounds-checked against the buffer before it is used to slice it.
///
/// The AEAD above means this input is authenticated by the time it arrives, so the checks
/// are no longer the security boundary they were in Phase 2 — but they are what turns a bug
/// elsewhere into an error message instead of a panic in a process holding a camera.
fn decode(bytes: &[u8]) -> Result<Template, String> {
    let mut at = 0usize;
    let mut take = |n: usize| -> Result<&[u8], String> {
        let end = at.checked_add(n).ok_or("length overflow")?;
        let slice = bytes.get(at..end).ok_or("truncated template")?;
        at = end;
        Ok(slice)
    };

    if take(4)? != INNER_MAGIC {
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

    const CSR: u32 = 1000;

    fn template() -> Template {
        let a = Embedding::new((0..128).map(|i| i as f32 - 60.0).collect()).expect("nonzero");
        let b = Embedding::new((0..128).map(|i| (i % 7) as f32 + 1.0).collect()).expect("nonzero");
        Template::new("sface-2021dec", vec![a, b]).expect("nonempty")
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("faciolockd-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Round-trip on *similarity*, not on bytes. `Embedding::new` re-normalises on load and
    /// an f32 unit vector's norm is not exactly 1.0, so an equality assertion here would be
    /// testing float bit patterns rather than the property that matters: the template that
    /// comes back matches the one that went in.
    #[test]
    fn a_template_survives_the_disk() {
        let dir = scratch("store-roundtrip");
        let original = template();

        assert!(load(&dir, CSR).expect("readable").is_none(), "no file is not enrolled");
        save(&dir, CSR, &original).expect("saves");
        let back = load(&dir, CSR).expect("readable").expect("enrolled");

        assert_eq!(back.len(), original.len());
        assert_eq!(back.embedder, original.embedder);
        for e in original.embeddings() {
            assert!(
                (back.best_similarity(e) - 1.0).abs() < 1e-6,
                "an embedding did not survive the round trip"
            );
        }
    }

    /// The store is not plaintext, which is the whole reason FLT1 was replaced. Asserted on
    /// the raw bytes rather than trusted from the code above it: the embedder id is a known
    /// string that a plaintext file would contain verbatim.
    #[test]
    fn nothing_recognisable_is_left_on_disk() {
        let dir = scratch("store-opaque");
        save(&dir, CSR, &template()).expect("saves");
        let raw = std::fs::read(path_for(&dir, CSR)).expect("read");

        assert!(raw.starts_with(MAGIC), "the envelope is identifiable");
        assert!(
            !raw.windows(4).any(|w| w == INNER_MAGIC),
            "the inner header must not be readable on disk"
        );
        assert!(
            !raw.windows(13).any(|w| w == b"sface-2021dec"),
            "the embedder id must not be readable on disk"
        );
    }

    /// **Phase 4's first exit criterion.** There is no second machine here, so "another
    /// machine" is modelled as what a second machine actually is to this format: a different
    /// key. Copying the template alone gets you nothing.
    #[test]
    fn a_template_copied_without_its_key_is_unusable() {
        let here = scratch("store-here");
        let elsewhere = scratch("store-elsewhere");
        save(&here, CSR, &template()).expect("saves");

        // The attacker's machine: same template bytes, its own key.
        std::fs::create_dir_all(&elsewhere).expect("mkdir");
        std::fs::copy(path_for(&here, CSR), path_for(&elsewhere, CSR)).expect("copy");
        assert!(
            load(&elsewhere, CSR).is_err(),
            "a template must not decrypt under a different key"
        );

        // And the key file is genuinely different, so the assertion above is not passing
        // for some other reason.
        assert_ne!(
            std::fs::read(here.join(KEY_FILE)).expect("key"),
            std::fs::read(elsewhere.join(KEY_FILE)).expect("key"),
            "each store must generate its own key"
        );
    }

    /// The uid is authenticated, not merely the filename. Renaming `1000.tpl` to `0.tpl`
    /// must not hand an attacker root's enrolment — this is A3's "plant a template" half.
    #[test]
    fn a_template_cannot_be_moved_to_another_uid() {
        let dir = scratch("store-uid-aad");
        save(&dir, CSR, &template()).expect("saves");
        std::fs::rename(path_for(&dir, CSR), path_for(&dir, 0)).expect("rename");

        assert!(load(&dir, 0).is_err(), "the uid must be bound into the ciphertext");
        assert!(load(&dir, CSR).expect("readable").is_none(), "and it moved, so 1000 is gone");
    }

    /// AEAD's whole point: an edited template is an error, not a different face. Checked at
    /// three offsets because a tag-only check would pass if the nonce or ciphertext were
    /// somehow outside the authenticated span.
    #[test]
    fn a_single_flipped_bit_is_an_error_not_a_different_face() {
        let dir = scratch("store-tamper");
        save(&dir, CSR, &template()).expect("saves");
        let path = path_for(&dir, CSR);
        let good = std::fs::read(&path).expect("read");

        for at in [5, good.len() / 2, good.len() - 1] {
            let mut bad = good.clone();
            bad[at] ^= 0x01;
            std::fs::write(&path, &bad).expect("write");
            assert!(load(&dir, CSR).is_err(), "a flipped bit at {at} must not decrypt");
        }
    }

    /// A damaged store must be an error, never "not enrolled" — the latter falls through to
    /// a password prompt with nothing said about why.
    #[test]
    fn a_truncated_file_is_an_error_not_a_missing_enrolment() {
        let dir = scratch("store-trunc");
        save(&dir, CSR, &template()).expect("saves");

        let path = path_for(&dir, CSR);
        let full = std::fs::read(&path).expect("read");
        for cut in [0, 4, 20, full.len() - 1] {
            std::fs::write(&path, &full[..cut]).expect("truncate");
            assert!(load(&dir, CSR).is_err(), "{cut} bytes must not read as absent");
        }

        std::fs::write(&path, [full.clone(), vec![0u8]].concat()).expect("extend");
        assert!(load(&dir, CSR).is_err(), "trailing bytes must not be ignored");
    }

    /// The inner codec's own bounds checks, reached directly. They are behind an AEAD now,
    /// so nothing hostile gets here — but the checks are what keep a bug elsewhere from
    /// panicking a process that is holding a camera.
    #[test]
    fn the_inner_codec_refuses_lengths_that_do_not_fit() {
        let good = encode(&template());
        assert!(decode(&good).is_ok());

        assert!(decode(&good[..good.len() - 9]).is_err(), "truncated");
        assert!(decode(&[good.clone(), vec![0u8]].concat()).is_err(), "trailing");
        assert!(decode(b"NOPE").is_err(), "wrong magic");

        // A count field that promises far more than the buffer holds.
        let mut lying = good.clone();
        lying[4 + 2 + 13] = 0xff;
        assert!(decode(&lying).is_err(), "a lying count must not read past the buffer");
    }

    /// The key is a secret sitting next to the thing it protects, so its mode is not a
    /// detail. B23 is why this asserts the mode rather than trusting the call that set it.
    #[test]
    fn the_key_file_is_owner_only_from_the_moment_it_exists() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("store-keymode");
        save(&dir, CSR, &template()).expect("saves");

        let key = dir.join(KEY_FILE);
        let mode = std::fs::metadata(&key).expect("key exists").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the key must never be group- or world-readable");
        assert_eq!(std::fs::read(&key).expect("read").len(), KEY_LEN);

        let tpl = std::fs::metadata(path_for(&dir, CSR)).expect("template").permissions().mode();
        assert_eq!(tpl & 0o777, 0o600, "and neither may the template");
    }

    /// Two saves must not produce the same ciphertext for the same template — a repeated
    /// nonce under one key is the failure mode this construction has to avoid, and it is
    /// cheap to notice if `random_bytes` ever stops being random.
    #[test]
    fn each_save_uses_a_fresh_nonce() {
        let dir = scratch("store-nonce");
        let t = template();
        let a = seal(&dir, CSR, &t).expect("seals");
        let b = seal(&dir, CSR, &t).expect("seals");
        assert_ne!(a[5..29], b[5..29], "the nonce must not repeat");
        assert_ne!(a, b);
    }
}

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

const KEY_FILE: &str = "key";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyProvider {
    File = 0,
}

impl KeyProvider {
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

fn aad(provider: KeyProvider, uid: u32) -> [u8; 9] {
    let mut out = [0u8; 9];
    out[..4].copy_from_slice(MAGIC);
    out[4] = provider as u8;
    out[5..].copy_from_slice(&uid.to_le_bytes());
    out
}

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

fn seal(dir: &Path, uid: u32, template: &Template) -> Result<Vec<u8>, String> {
    let provider = KeyProvider::File;
    let key = provider.key(dir)?;
    let nonce = random_bytes::<NONCE_LEN>()?;
    let plaintext = encode(template);

    let cipher = XChaCha20Poly1305::new((&key).into());
    let aad = aad(provider, uid);
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: &plaintext, aad: &aad })
        .map_err(|_| "could not encrypt the template".to_string())?;

    let mut out = Vec::with_capacity(MAGIC.len() + 1 + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.push(provider as u8);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

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

    #[test]
    fn a_template_copied_without_its_key_is_unusable() {
        let here = scratch("store-here");
        let elsewhere = scratch("store-elsewhere");
        save(&here, CSR, &template()).expect("saves");

        std::fs::create_dir_all(&elsewhere).expect("mkdir");
        std::fs::copy(path_for(&here, CSR), path_for(&elsewhere, CSR)).expect("copy");
        assert!(
            load(&elsewhere, CSR).is_err(),
            "a template must not decrypt under a different key"
        );

        assert_ne!(
            std::fs::read(here.join(KEY_FILE)).expect("key"),
            std::fs::read(elsewhere.join(KEY_FILE)).expect("key"),
            "each store must generate its own key"
        );
    }

    #[test]
    fn a_template_cannot_be_moved_to_another_uid() {
        let dir = scratch("store-uid-aad");
        save(&dir, CSR, &template()).expect("saves");
        std::fs::rename(path_for(&dir, CSR), path_for(&dir, 0)).expect("rename");

        assert!(load(&dir, 0).is_err(), "the uid must be bound into the ciphertext");
        assert!(load(&dir, CSR).expect("readable").is_none(), "and it moved, so 1000 is gone");
    }

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

    #[test]
    fn the_inner_codec_refuses_lengths_that_do_not_fit() {
        let good = encode(&template());
        assert!(decode(&good).is_ok());

        assert!(decode(&good[..good.len() - 9]).is_err(), "truncated");
        assert!(decode(&[good.clone(), vec![0u8]].concat()).is_err(), "trailing");
        assert!(decode(b"NOPE").is_err(), "wrong magic");

        let mut lying = good.clone();
        lying[4 + 2 + 13] = 0xff;
        assert!(decode(&lying).is_err(), "a lying count must not read past the buffer");
    }

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

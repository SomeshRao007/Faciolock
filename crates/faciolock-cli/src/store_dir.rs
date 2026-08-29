//! `list` and `remove`, straight against the template directory.
//!
//! ## Why not over the socket
//!
//! wiki/04's P4-d says `faciolockctl` reaches the store "through the socket like everything
//! else". Phase 5 softens that to: **the CLI reads the store's directory and never its
//! contents.** The reason is the response format. A `list` answer is a variable-length
//! payload and [`faciolock_ipc::Response`] is four fixed bytes parsed by a PAM module inside
//! a root process — `sudo`, `login`, `sddm`. Growing that parser to carry a list of uids
//! would put the least security-sensitive feature in the project through the most
//! security-sensitive code in it, to save a `read_dir`.
//!
//! What P4-d was actually protecting is untouched: **nothing here links the store, decrypts
//! anything, or reads the key.** A filename is a uid and a mtime is a date. The encrypted
//! bytes are never opened, so the CLI cannot learn a template even by accident, and
//! `store.rs` stays a private module of the daemon with exactly one consumer.
//!
//! `remove` unlinks. That is a strictly weaker capability than the caller already has —
//! reaching this code at all requires being root, and root can unlink the file with `rm`.

use std::path::{Path, PathBuf};

use crate::passwd;

/// One enrolled user, as the directory describes them.
pub struct Enrolment {
    pub uid: u32,
    /// `None` when the uid has no passwd entry — a template that outlived its user, which is
    /// the interesting thing `list` exists to surface.
    pub name: Option<String>,
    pub bytes: u64,
    pub modified: Option<std::time::SystemTime>,
}

fn path_for(dir: &Path, uid: u32) -> PathBuf {
    // Must agree with `faciolock-daemon`'s `store::path_for`. Asserted below against a name
    // the daemon's own tests use, which is the cheapest check available across two crates.
    dir.join(format!("{uid}.tpl"))
}

/// Every `<uid>.tpl` in the directory, sorted by uid so repeated runs are diffable.
///
/// A missing directory is an empty list, not an error: nobody has enrolled yet, and that is
/// the state a fresh install is in rather than a fault to report.
pub fn list(dir: &Path) -> Result<Vec<Enrolment>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };

    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(uid) = uid_from_filename(&name.to_string_lossy()) else { continue };
        let meta = entry.metadata().map_err(|e| format!("{}: {e}", entry.path().display()))?;
        found.push(Enrolment {
            uid,
            name: passwd::name_of(uid),
            bytes: meta.len(),
            modified: meta.modified().ok(),
        });
    }
    found.sort_by_key(|e| e.uid);
    Ok(found)
}

/// `1000.tpl` → `1000`. Anything else in the directory — the `key` file, a stray editor
/// backup, a subdirectory — is not an enrolment and is skipped rather than guessed at.
fn uid_from_filename(name: &str) -> Option<u32> {
    name.strip_suffix(".tpl")?.parse().ok()
}

/// Delete one enrolment. `Ok(false)` when there was nothing there.
pub fn remove(dir: &Path, uid: u32) -> Result<bool, String> {
    let path = path_for(dir, uid);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("faciolockctl-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("tmpdir");
        dir
    }

    /// The whole of `list`'s parsing, including everything it must *not* treat as an
    /// enrolment. The `key` file sits in this directory by design (wiki/04: same trust
    /// domain), so listing it as a user would be the obvious way to get this wrong.
    #[test]
    fn only_uid_named_templates_are_enrolments() {
        assert_eq!(uid_from_filename("1000.tpl"), Some(1000));
        assert_eq!(uid_from_filename("0.tpl"), Some(0));
        for not_one in ["key", "1000.tpl.bak", "csr.tpl", ".tpl", "-1.tpl", "1000", "1e3.tpl"] {
            assert_eq!(uid_from_filename(not_one), None, "{not_one} is not an enrolment");
        }
    }

    /// The filename shape is shared with the daemon's `store::path_for` and neither crate can
    /// see the other's. This is the seam, so it is spelled out here on purpose.
    #[test]
    fn the_filename_matches_what_the_daemon_writes() {
        assert_eq!(path_for(Path::new("/var/lib/faciolock/templates"), 1000).to_str(),
            Some("/var/lib/faciolock/templates/1000.tpl"));
    }

    #[test]
    fn listing_finds_templates_sorted_and_skips_the_key() {
        let dir = tmpdir("list");
        std::fs::write(dir.join("1000.tpl"), b"encrypted").expect("write");
        std::fs::write(dir.join("0.tpl"), b"encrypted too").expect("write");
        std::fs::write(dir.join("key"), [0u8; 32]).expect("write");

        let found = list(&dir).expect("lists");
        assert_eq!(found.iter().map(|e| e.uid).collect::<Vec<_>>(), vec![0, 1000]);
        assert_eq!(found[0].name.as_deref(), Some("root"));
        assert_eq!(found[0].bytes, 13);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fresh install has no directory yet, and `list` on it is "nobody is enrolled" rather
    /// than a fault. Getting this wrong makes the first command a new user runs look broken.
    #[test]
    fn a_missing_directory_is_an_empty_list_not_an_error() {
        assert!(list(Path::new("/nonexistent/faciolock/templates")).expect("empty").is_empty());
    }

    /// `remove` distinguishes "deleted it" from "there was nothing there", because the CLI
    /// prints different things and a silent success on a typo'd username is a lie.
    #[test]
    fn removing_reports_whether_anything_was_there() {
        let dir = tmpdir("remove");
        std::fs::write(dir.join("1000.tpl"), b"encrypted").expect("write");

        assert_eq!(remove(&dir, 1000), Ok(true));
        assert!(!dir.join("1000.tpl").exists(), "the file is gone");
        assert_eq!(remove(&dir, 1000), Ok(false), "a second remove finds nothing");
        assert_eq!(remove(&dir, 4242), Ok(false));

        let _ = std::fs::remove_dir_all(&dir);
    }
}

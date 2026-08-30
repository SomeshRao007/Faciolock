use std::path::{Path, PathBuf};

use crate::passwd;

pub struct Enrolment {
    pub uid: u32,
    pub name: Option<String>,
    pub bytes: u64,
    pub modified: Option<std::time::SystemTime>,
}

fn path_for(dir: &Path, uid: u32) -> PathBuf {
    dir.join(format!("{uid}.tpl"))
}

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

fn uid_from_filename(name: &str) -> Option<u32> {
    name.strip_suffix(".tpl")?.parse().ok()
}

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

    #[test]
    fn only_uid_named_templates_are_enrolments() {
        assert_eq!(uid_from_filename("1000.tpl"), Some(1000));
        assert_eq!(uid_from_filename("0.tpl"), Some(0));
        for not_one in ["key", "1000.tpl.bak", "csr.tpl", ".tpl", "-1.tpl", "1000", "1e3.tpl"] {
            assert_eq!(uid_from_filename(not_one), None, "{not_one} is not an enrolment");
        }
    }

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

    #[test]
    fn a_missing_directory_is_an_empty_list_not_an_error() {
        assert!(list(Path::new("/nonexistent/faciolock/templates")).expect("empty").is_empty());
    }

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

use std::collections::HashMap;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const LOCKOUT_AFTER: u32 = 3;

const BACKOFF: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(2)];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Entry {
    failures: u32,
    last_failure: u64,
}

impl Entry {
    fn locked(&self) -> bool {
        self.failures >= LOCKOUT_AFTER
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    Backoff { retry_in: Duration },
    LockedOut,
}

impl Decision {
    pub fn refused(&self) -> bool {
        match self {
            Decision::Allowed => false,
            Decision::Backoff { .. } | Decision::LockedOut => true,
        }
    }

    pub fn reason(&self) -> &'static str {
        match self {
            Decision::Allowed => "allowed",
            Decision::Backoff { .. } => "backoff",
            Decision::LockedOut => "lockout",
        }
    }
}

// ponytail: whole-file rewrite, see wiki/14
pub struct Limiter {
    path: PathBuf,
    entries: HashMap<u32, Entry>,
}

impl Limiter {
    pub fn load(path: &Path) -> Limiter {
        let mut entries = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(path) {
            let now = now();
            for line in text.lines() {
                let mut f = line.split_whitespace();
                let (Some(uid), Some(failures), Some(last)) = (f.next(), f.next(), f.next())
                else {
                    continue;
                };
                let (Ok(uid), Ok(failures), Ok(last)) =
                    (uid.parse::<u32>(), failures.parse::<u32>(), last.parse::<u64>())
                else {
                    continue;
                };
                entries.insert(uid, Entry { failures, last_failure: last.min(now) });
            }
        }
        Limiter { path: path.to_path_buf(), entries }
    }

    pub fn decide(&self, uid: u32) -> Decision {
        let Some(entry) = self.entries.get(&uid) else {
            return Decision::Allowed;
        };
        if entry.locked() {
            return Decision::LockedOut;
        }
        let Some(wait) = BACKOFF.get(entry.failures.saturating_sub(1) as usize) else {
            return Decision::Allowed;
        };
        let elapsed = Duration::from_secs(now().saturating_sub(entry.last_failure));
        match wait.checked_sub(elapsed) {
            Some(retry_in) if !retry_in.is_zero() => Decision::Backoff { retry_in },
            _ => Decision::Allowed,
        }
    }

    pub fn record_failure(&mut self, uid: u32) -> Result<Decision, String> {
        let entry = self.entries.entry(uid).or_default();
        entry.failures = entry.failures.saturating_add(1);
        entry.last_failure = now();
        let locked = entry.locked();
        self.flush()?;
        Ok(if locked { Decision::LockedOut } else { self.decide(uid) })
    }

    pub fn clear(&mut self, uid: u32) -> Result<bool, String> {
        match self.entries.remove(&uid) {
            None => Ok(false),
            Some(_) => self.flush().map(|()| true),
        }
    }

    fn flush(&self) -> Result<(), String> {
        use std::io::Write;

        let mut body = String::new();
        let mut uids: Vec<&u32> = self.entries.keys().collect();
        uids.sort_unstable();
        for uid in uids {
            let e = &self.entries[uid];
            body.push_str(&format!("{uid} {} {}\n", e.failures, e.last_failure));
        }

        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let staging = with_suffix(&self.path, &format!(".new.{}", std::process::id()));
        let write = || -> io::Result<()> {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&staging)?;
            f.write_all(body.as_bytes())?;
            f.sync_all()
        };
        if let Err(e) = write().and_then(|()| std::fs::rename(&staging, &self.path)) {
            let _ = std::fs::remove_file(&staging);
            return Err(format!("{}: {e}", self.path.display()));
        }
        Ok(())
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSR: u32 = 1000;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("faciolockd-rl-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("ratelimit.db")
    }

    fn age(limiter: &mut Limiter, uid: u32, seconds: u64) {
        limiter.entries.get_mut(&uid).expect("entry").last_failure -= seconds;
    }

    #[test]
    fn a_fresh_user_is_allowed_and_the_first_failure_backs_them_off() {
        let path = scratch("basic");
        let mut limiter = Limiter::load(&path);
        assert_eq!(limiter.decide(CSR), Decision::Allowed);

        limiter.record_failure(CSR).expect("persists");
        assert!(matches!(limiter.decide(CSR), Decision::Backoff { .. }));

        age(&mut limiter, CSR, 2);
        assert_eq!(limiter.decide(CSR), Decision::Allowed, "the window expires");
    }

    #[test]
    fn three_failures_lock_the_user_out_and_time_does_not_help() {
        let path = scratch("lockout");
        let mut limiter = Limiter::load(&path);
        for _ in 0..LOCKOUT_AFTER {
            limiter.record_failure(CSR).expect("persists");
        }
        assert_eq!(limiter.decide(CSR), Decision::LockedOut);

        age(&mut limiter, CSR, 86_400);
        assert_eq!(limiter.decide(CSR), Decision::LockedOut, "a day later, still locked");

        limiter.clear(CSR).expect("clears");
        assert_eq!(limiter.decide(CSR), Decision::Allowed);
    }

    #[test]
    fn state_survives_being_dropped_and_reloaded() {
        let path = scratch("persist");
        let mut limiter = Limiter::load(&path);
        for _ in 0..LOCKOUT_AFTER {
            limiter.record_failure(CSR).expect("persists");
        }
        drop(limiter);

        let reloaded = Limiter::load(&path);
        assert_eq!(reloaded.decide(CSR), Decision::LockedOut, "a restart must not reset it");

        assert_eq!(reloaded.decide(CSR + 1), Decision::Allowed);
    }

    #[test]
    fn a_future_timestamp_does_not_grant_an_early_retry() {
        let path = scratch("clock");
        std::fs::create_dir_all(path.parent().expect("dir")).expect("mkdir");
        std::fs::write(&path, format!("{CSR} 1 {}\n", now() + 10_000)).expect("write");

        let limiter = Limiter::load(&path);
        assert!(
            matches!(limiter.decide(CSR), Decision::Backoff { .. }),
            "a clamped future timestamp still backs off"
        );
    }

    #[test]
    fn a_corrupt_line_is_skipped_and_the_rest_of_the_file_survives() {
        let path = scratch("corrupt");
        std::fs::create_dir_all(path.parent().expect("dir")).expect("mkdir");
        std::fs::write(&path, format!("garbage\n{CSR} 3 {}\n\nalso bad 1\n", now()))
            .expect("write");

        let limiter = Limiter::load(&path);
        assert_eq!(limiter.decide(CSR), Decision::LockedOut, "the good line still parses");
        assert_eq!(limiter.decide(4242), Decision::Allowed, "a bad line invents nobody");
    }

    #[test]
    fn the_database_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let path = scratch("mode");
        let mut limiter = Limiter::load(&path);
        limiter.record_failure(CSR).expect("persists");

        let mode = std::fs::metadata(&path).expect("exists").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn clearing_an_unknown_user_is_quiet() {
        let path = scratch("clear-noop");
        let mut limiter = Limiter::load(&path);
        assert_eq!(limiter.clear(CSR), Ok(false));

        limiter.record_failure(CSR).expect("persists");
        assert_eq!(limiter.clear(CSR), Ok(true));
        assert_eq!(limiter.clear(CSR), Ok(false), "and it stays cleared");
    }
}

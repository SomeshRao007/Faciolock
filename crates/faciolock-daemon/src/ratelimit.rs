//! Per-user failure budget, persisted across daemon restarts (wiki/03 R5).
//!
//! ## Why this is load-bearing rather than defence in depth
//!
//! R5 was written as the thing that "turns *a photo might work* from a 200-attempt certainty
//! into a 3-attempt gamble", on the assumption that liveness was the primary control and
//! this was the backstop. [B24] inverted that: a flat printed face clears every enforced
//! liveness check and is detected by YuNet while doing it. Until a check exists that rejects
//! a print, **this file is the only thing between a photograph and unlimited retries.**
//!
//! ## Policy
//!
//! Hardcoded, never configurable (decision P4-c). `config.rs` already carries the reasoning:
//! a threshold reachable from an editable file is a threshold an attacker with one write
//! sets to zero. The same argument applies with more force here, because the file this would
//! live in is read by the process that grants access.
//!
//! | consecutive failures | effect |
//! |---|---|
//! | 1 | next attempt refused for 1 s |
//! | 2 | next attempt refused for 2 s |
//! | 3 | **locked** until an [`Op::Clear`](faciolock_ipc::Op::Clear) arrives |
//!
//! Three is R5's own number. It is small on purpose and it is *cheap to be wrong about*:
//! every refusal here falls through to the password prompt, and typing that password is
//! itself what lifts the lock.
//!
//! ## What counts
//!
//! Only [`Verdict::counts_as_an_attempt`](faciolock_vision::Verdict::counts_as_an_attempt).
//! A bright room, a camera held by a video call, a cancelled request — none of them looked
//! at a face, so none of them is evidence of anything (P2-g). Counting them would let the
//! weather disable face unlock.
//!
//! [B24]: ../../../wiki/11-bug-log.md

use std::collections::HashMap;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Failures before face authentication is disabled for the user (wiki/03 R5).
const LOCKOUT_AFTER: u32 = 3;

/// Backoff after 1 and 2 failures. Indexed by `failures - 1`; at `LOCKOUT_AFTER` the entry
/// is locked and the delay stops being consulted.
const BACKOFF: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(2)];

/// One user's state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Entry {
    failures: u32,
    /// Unix seconds. Wall clock rather than a monotonic one because the whole point is to
    /// survive a reboot, and `CLOCK_BOOTTIME` does not. [`Limiter::decide`] is what makes a
    /// clock moved backwards useless.
    last_failure: u64,
}

impl Entry {
    fn locked(&self) -> bool {
        self.failures >= LOCKOUT_AFTER
    }
}

/// What the daemon should do with this request, before it touches a template or a camera.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allowed,
    /// Inside the exponential backoff window.
    Backoff { retry_in: Duration },
    /// [`LOCKOUT_AFTER`] consecutive failures. Only a successful authentication clears it.
    LockedOut,
}

impl Decision {
    pub fn refused(&self) -> bool {
        match self {
            Decision::Allowed => false,
            Decision::Backoff { .. } | Decision::LockedOut => true,
        }
    }

    /// For the audit line, so the journal says which of the two refusals it was.
    pub fn reason(&self) -> &'static str {
        match self {
            Decision::Allowed => "allowed",
            Decision::Backoff { .. } => "backoff",
            Decision::LockedOut => "lockout",
        }
    }
}

/// The whole store, held in memory and written through on every change.
///
/// ponytail: the entire file is rewritten on each update. It holds one line per enrolled
/// user on a personal machine, so this is a few hundred bytes; a keyed database would be
/// more code and one more thing to corrupt. Revisit if a multi-user host ever appears, which
/// is not a machine this project targets.
pub struct Limiter {
    path: PathBuf,
    entries: HashMap<u32, Entry>,
}

impl Limiter {
    /// Read what is on disk. A missing file is a fresh install; a corrupt *line* is skipped
    /// rather than fatal.
    ///
    /// Skipping is the conservative direction only because of what a lost line means: the
    /// user starts from zero failures, which is where they would be anyway on a machine
    /// where this file had never existed. Refusing to start would take face unlock down for
    /// everybody because one line got mangled, and refusing every *authentication* would be
    /// worse. What must not happen is a corrupt line reading as "locked" for someone who is
    /// not, and it cannot: the field either parses or the line is dropped.
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
                // **Clock-rollback defence.** A `last_failure` in the future would make
                // every backoff comparison below succeed forever if the clock were then
                // moved back, so it is clamped on the way in. Setting the clock needs root,
                // which is not the attacker R5 is aimed at — but the clamp is one line and
                // the alternative is reasoning about who can call `settimeofday`.
                entries.insert(uid, Entry { failures, last_failure: last.min(now) });
            }
        }
        Limiter { path: path.to_path_buf(), entries }
    }

    /// May this user be looked at right now?
    ///
    /// Called **before** the template load and before the camera, so a locked-out user costs
    /// nothing and lights no emitter. `Decision` is not itself an attempt and never extends
    /// the backoff — otherwise a caller retrying in a loop could keep themselves locked out
    /// forever without ever presenting a face.
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
        // Saturating, so a clock that jumped forward reads as "the window has passed"
        // rather than underflowing into a very long one.
        let elapsed = Duration::from_secs(now().saturating_sub(entry.last_failure));
        match wait.checked_sub(elapsed) {
            Some(retry_in) if !retry_in.is_zero() => Decision::Backoff { retry_in },
            _ => Decision::Allowed,
        }
    }

    /// Record a failed attempt.
    ///
    /// Persisted **before the daemon replies**, so `kill -9` between the decision and the
    /// answer cannot roll the counter back. That is Phase 4's second exit criterion, and it
    /// is the reason this returns a write error rather than swallowing one: a limiter that
    /// silently stopped persisting would look exactly like a working one.
    pub fn record_failure(&mut self, uid: u32) -> Result<Decision, String> {
        let entry = self.entries.entry(uid).or_default();
        entry.failures = entry.failures.saturating_add(1);
        entry.last_failure = now();
        let locked = entry.locked();
        self.flush()?;
        Ok(if locked { Decision::LockedOut } else { self.decide(uid) })
    }

    /// Forget this user's failures. A successful face authentication and an `Op::Clear` from
    /// `pam_sm_acct_mgmt` are the same event as far as this file is concerned: somebody
    /// proved who they were.
    ///
    /// Returns whether anything changed, so the audit log can stay quiet about the clears
    /// that had nothing to clear — `acct_mgmt` runs on every successful login, and a journal
    /// line per login saying "nothing to do" is noise that hides the ones that matter.
    pub fn clear(&mut self, uid: u32) -> Result<bool, String> {
        match self.entries.remove(&uid) {
            None => Ok(false),
            Some(_) => self.flush().map(|()| true),
        }
    }

    /// Whole-file rewrite through a staging name, so a crash mid-write leaves the previous
    /// state rather than half of the new one. `rename` is atomic; a truncate-and-write is
    /// not, and the failure it would produce is a limiter that forgot somebody's failures.
    fn flush(&self) -> Result<(), String> {
        use std::io::Write;

        let mut body = String::new();
        // Sorted so the file is stable across runs and a diff of it means something.
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
            // Without this the rename can land before the data does, and a power loss
            // between them leaves an empty file — which reads as "nobody has ever failed".
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

    /// Ages an entry by rewriting its timestamp, so the backoff can be tested without the
    /// test sleeping for it.
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

    /// R5's three-strike rule, and the fact that waiting does not lift it. If the lockout
    /// expired on its own it would be a longer backoff, not a lockout.
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

    /// **Phase 4's second exit criterion, minus the kill.** A limiter rebuilt from the same
    /// path must remember. The `kill -9` half needs a real process and lives in
    /// `tools/ratelimit-try.sh`; this is the half a test can hold.
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

        // And a second user in the same file is unaffected by the first's lockout.
        assert_eq!(reloaded.decide(CSR + 1), Decision::Allowed);
    }

    /// Moving the clock backwards must not shorten a backoff. Written as a future timestamp
    /// on disk, which is what a backwards clock change leaves behind.
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

    /// A mangled line must not take the file down with it, and must not invent a lockout.
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

    /// The file holds a failure budget for a security control, so it is not world-readable
    /// — and, more usefully, it is not world-*writable*, which would make the lockout a
    /// suggestion.
    #[test]
    fn the_database_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let path = scratch("mode");
        let mut limiter = Limiter::load(&path);
        limiter.record_failure(CSR).expect("persists");

        let mode = std::fs::metadata(&path).expect("exists").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    /// Clearing a user who has no entry is not an error and reports that nothing happened —
    /// `acct_mgmt` fires on every successful login and most of them have nothing to clear.
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

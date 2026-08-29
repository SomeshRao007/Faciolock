//! `faciolockd` — the unprivileged daemon that owns the camera and the ML.
//!
//! wiki/04's privilege split lives here by *absence*: this process holds the V4L2 device,
//! two ONNX models and the template store, and the PAM module holds a socket. Nothing in
//! this binary runs as root.
//!
//! ## What lives here
//!
//! Phase 2 built the answer path, Phase 3 the access control on the socket, Phase 4 the
//! policy: a uid-keyed encrypted [`store`], a persistent failure budget in [`ratelimit`],
//! and one journal line per attempt in [`audit`]. Deliberately still **not** here:
//!
//! - TPM sealing — deferred by P4-a with the seam in place ([`store::KeyProvider`]); wiki/08
//!   Q5 records what would re-open it.
//! - Socket activation and idle-exit — Phase 6.
//!
//! The failure-budget *policy* is not on the wire, on purpose: [B20] reopened the
//! `Environment` exemption once already and it has to stay re-decidable without a protocol
//! break (wiki/13). What crosses the socket is a category; what to do about it is decided
//! here.
//!
//! [B20]: ../../../wiki/11-bug-log.md

mod audit;
mod config;
mod models;
mod ratelimit;
mod serve;
mod session;
mod store;
mod users;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use faciolock_ipc::{ENROL_POSES, Op, Request, Verdict};
use faciolock_vision::{Burst, Config as VisionConfig, Embedding, Pipeline, Template};

use config::Config;
use ratelimit::{Decision, Limiter};
use serve::Peer;
use session::Session;

const DEFAULT_CONFIG: &str = "/etc/faciolock/faciolockd.toml";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut config_path = PathBuf::from(DEFAULT_CONFIG);

    // `--enroll USERNAME` was here and is gone (Phase 5). It existed to make `Op::Auth`
    // testable before there was any way to enrol, and its own comment said so; enrolment is
    // now `faciolockctl enroll`, over the socket, guided, and audited. Keeping both would
    // mean two capture-and-store paths to hold in sync, one of which nobody runs.
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => match it.next() {
                Some(v) => config_path = PathBuf::from(v),
                None => return fail("--config needs a path"),
            },
            "--help" | "-h" => {
                println!("faciolockd [--config PATH]");
                return ExitCode::SUCCESS;
            }
            other => return fail(&format!("unknown argument {other}")),
        }
    }

    match start(&config_path) {
        Ok(daemon) => daemon.serve(),
        Err(e) => fail(&e),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("faciolockd: {message}");
    ExitCode::FAILURE
}

/// Everything that must be true before the socket is bound.
///
/// Model verification comes first and refuses to continue on a mismatch. A daemon that
/// bound the socket and *then* discovered a tampered embedder would be answering
/// authentication requests with a model it could not vouch for.
fn start(config_path: &Path) -> Result<Daemon, String> {
    let cfg = Config::load(config_path)?;
    models::verify(&cfg.models)?;

    let mut vision = VisionConfig::default();

    // P1-e: the capability tier is enrolment-time configuration, never inferred from the
    // burst being authenticated — inferring it per burst would let an attacker with a bright
    // enough continuous IR source *demote* the machine to a tier whose checks they can pass.
    // `faciolockctl diagnose` observes it once and writes it here.
    vision.tier = cfg.tier()?;

    // **The settle discard belongs to exactly one layer, and for live capture it is the
    // camera's.** `VisionConfig::agc_settle_frames` defaults to 5 because the corpus
    // arrives as 90-frame `.gray` recordings whose head is unsettled (wiki/10 Probe 3).
    // A live burst is not that: `Capture::start` already burned `warmup_frames`, and
    // `Capture::burst` flushes everything the driver had ready before taking a frame
    // (P2-c), so every frame handed over here is post-warmup and post-flush. Leaving the
    // default on top of that discards the settle frames twice — see B22.
    vision.agc_settle_frames = 0;

    // A burst too short to yield `min_pairs` cannot produce any verdict but `TooFewPairs`,
    // and a daemon that starts anyway would answer `Unavailable` forever with nothing in
    // the log saying why. Cheaper to refuse at startup than to debug at the lock screen.
    let need = vision.min_pairs * 2;
    if cfg.burst_frames < need {
        return Err(format!(
            "burst_frames = {} cannot yield min_pairs = {} (needs at least {need})",
            cfg.burst_frames, vision.min_pairs
        ));
    }

    let pipeline = Pipeline::load(&cfg.models, vision)?;
    eprintln!(
        "faciolockd: models verified, embedder {}, tier {:?}",
        pipeline.embedder_id(),
        vision.tier
    );

    let limiter = Mutex::new(Limiter::load(&cfg.ratelimit));

    Ok(Daemon {
        session: Arc::new(Mutex::new(Session::new(cfg.clone()))),
        limiter,
        pending: Mutex::new(None),
        pipeline,
        cfg,
    })
}

pub struct Daemon {
    /// Shared with the startup warm thread.
    session: Arc<Mutex<Session>>,
    /// The failure budget (wiki/03 R5). Behind a mutex for the same reason `session` is:
    /// the daemon is serial today, but the warm thread already shares the process and
    /// Phase 6 may add more.
    limiter: Mutex<Limiter>,
    /// A guided enrolment in progress, if any. One at a time — there is one camera.
    pending: Mutex<Option<Pending>>,
    pipeline: Pipeline,
    cfg: Config,
}

/// How long a half-finished enrolment survives **without a new pose arriving**.
///
/// Long enough to find your glasses between poses, short enough that a session somebody
/// walked away from does not sit waiting to have the next person's face appended to it.
///
/// It is a gap, not a total, and that distinction is load-bearing. As a total it silently
/// breaks a slow enrolment: two poses accepted, a minute of fumbling, and the third arrives
/// to a `Pending` that has expired — so the daemon starts over and answers `Ack`, the CLI
/// counts its third `Ack` and reports success, and no template was ever written. The user
/// finds out at a lock screen. Measured from the last pose instead, "abandoned" means what it
/// sounds like: nobody has sent anything for a minute.
const ENROL_EXPIRY: Duration = Duration::from_secs(60);

/// Poses collected so far, and by whom.
///
/// **The daemon accumulates, not the CLI.** The camera, the models and the store are all on
/// this side; a CLI that gathered embeddings and handed them back would make the store write
/// a function of the peer's bytes, which is precisely what `Op::Enroll` used to be refused
/// for. What crosses the socket stays "capture a pose" and "here is what I saw".
struct Pending {
    uid: u32,
    /// Every embedding from every pose. wiki/05 matches best-of over the enrolled set and
    /// the poses exist to put variation into it, so nothing is thinned here — the near
    /// duplicates within one pose cost a cosine each and the across-pose spread is the
    /// entire point of asking for three.
    embeddings: Vec<Embedding>,
    poses: usize,
    /// When the most recent pose landed, not when the enrolment began. See [`ENROL_EXPIRY`].
    touched: Instant,
}

impl Pending {
    fn fresh(uid: u32) -> Pending {
        Pending { uid, embeddings: Vec::new(), poses: 0, touched: Instant::now() }
    }

    /// Whether this belongs to the request now being answered. A different uid or an
    /// abandoned session starts over rather than inheriting somebody else's poses.
    fn belongs_to(&self, uid: u32) -> bool {
        self.uid == uid && self.touched.elapsed() < ENROL_EXPIRY
    }

    fn record(&mut self, embeddings: Vec<Embedding>) {
        self.embeddings.extend(embeddings);
        self.poses += 1;
        self.touched = Instant::now();
    }
}

/// A poisoned lock means the warm thread panicked mid-burst.
///
/// Recovering rather than propagating is deliberate: `Session` reopens its stream on the
/// next fatal capture error anyway, so there is no invariant a panic could have broken that
/// the next `burst` does not repair. Refusing every authentication for the life of the
/// process because a warm-up panicked would be a worse failure than trying again.
fn session_of(daemon: &Daemon) -> std::sync::MutexGuard<'_, Session> {
    daemon.session.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A poisoned limiter is recovered rather than propagated, for a different reason than the
/// session's: the in-memory state is a mirror of the file, and the file is rewritten whole
/// on every change. Whatever a panic interrupted, the next `record_failure` re-derives.
///
/// Refusing to answer would be the worse failure in the other direction too — a limiter that
/// panicked would disable face unlock for everybody until the daemon restarted, which is a
/// denial of service reachable by whatever caused the panic.
fn limiter_of(daemon: &Daemon) -> std::sync::MutexGuard<'_, Limiter> {
    daemon.limiter.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Recovered rather than propagated, like the two above. The worst a poisoned lock can hold
/// here is a partial enrolment, and a partial enrolment is already something the next pose is
/// allowed to discard.
fn pending_of(daemon: &Daemon) -> std::sync::MutexGuard<'_, Option<Pending>> {
    daemon.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Daemon {
    fn serve(self) -> ExitCode {
        let listener = match serve::listen(&self.cfg.socket) {
            Ok(l) => l,
            Err(e) => return fail(&e),
        };
        eprintln!("faciolockd: listening on {}", self.cfg.socket.display());

        // **Off the accept path, not on it.** The camera is still warmed eagerly, so the
        // AGC settle frames land before the first authentication rather than inside it
        // (wiki/06) — but `listen` has already bound by this point, so a client can connect
        // at any moment, and doing the warm here on the main thread left it sitting in the
        // kernel's backlog for a measured 1779 ms with nothing to observe. Worse, it made
        // requests that need no camera at all wait for one: `NoEnrolment` answered in
        // 1753 ms cold against 0.4 ms warm, for one absent file.
        //
        // On this thread the socket answers immediately and only work that genuinely needs
        // the camera waits for it. That is also what makes the Phase 6 pre-warm agent work:
        // its poke is answered while the warm is still running, which is the entire point
        // of poking early.
        let warming = Arc::clone(&self.session);
        std::thread::spawn(move || {
            let mut session = warming.lock().unwrap_or_else(|p| p.into_inner());
            match session.burst() {
                Ok(b) => eprintln!(
                    "faciolockd: camera warm on {}, {} frames in {} ms",
                    session.device().map_or("(discovered)".into(), |d| d.display().to_string()),
                    b.meta.len(),
                    b.sensor_span().as_millis()
                ),
                Err(e) => eprintln!("faciolockd: camera unavailable at startup ({e}); will retry"),
            }
        });

        serve::run(&listener, &self);
        let _ = std::fs::remove_file(&self.cfg.socket);
        ExitCode::SUCCESS
    }

    /// Answer one decoded request. The only entry point [`serve`] has into the pipeline.
    ///
    /// `None` means the caller hung up before the work finished, so there is no answer to
    /// send and nothing to count.
    pub fn answer(&self, req: &Request, peer: &Peer<'_>) -> Option<Verdict> {
        let user = req.username();

        // **The one translation.** wiki/04 keys the store and the failure budget by uid;
        // the wire carries a name because that is what `PAM_USER` gives the module. Below
        // this line nothing sees a username.
        //
        // A name that does not resolve reads as `NoEnrolment` — the same answer an
        // unenrolled real user gets, deliberately, so the socket cannot be used to
        // enumerate accounts. It is also true: a user with no uid has no template.
        let Some(uid) = users::uid_of(user) else {
            return Some(Verdict::NoEnrolment);
        };

        match req.op {
            Op::Auth => self.authenticate(uid, peer),
            Op::Status => {
                let enrolled = store::load(&self.cfg.templates, uid)
                    .inspect_err(|e| eprintln!("<3>faciolockd: template store: {e}"))
                    .ok()
                    .flatten()
                    .is_some();
                Some(serve::status(enrolled))
            }
            Op::Enroll => self.enrol(uid, peer),
            // wiki/03 R5's lockout release (P4-b). Sent by `pam_sm_acct_mgmt` after the
            // whole PAM stack has succeeded, so it means "somebody proved who they were" —
            // by password, by face, by anything the stack accepted.
            //
            // It grants nothing and it is not an authenticator. The peer is already root by
            // the time this runs (`serve::handle` checks `SO_PEERCRED` before the body is
            // read), and a root peer that wanted to lie about this could delete the
            // database instead.
            Op::Clear => {
                match limiter_of(self).clear(uid) {
                    Ok(true) => audit::cleared(uid, &peer.describe()),
                    Ok(false) => {}
                    Err(e) => eprintln!("<3>faciolockd: rate limiter: {e}"),
                }
                Some(Verdict::Ack)
            }
        }
    }

    /// The two `hung_up` checks bracket the two expensive stages — ~400 ms of capture and
    /// ~300 ms of inference. They are checks, not interrupts: a burst already in flight
    /// finishes (decision P3-b).
    /// The two `hung_up` checks bracket the two expensive stages — ~400 ms of capture and
    /// ~300 ms of inference. They are checks, not interrupts: a burst already in flight
    /// finishes (decision P3-b).
    ///
    /// Every path that returns `Some` goes through [`audit::attempt`] exactly once. A
    /// cancelled request returns `None` and is logged as cancelled: nobody is waiting for an
    /// answer and nothing was measured, so it is not an attempt and must not be counted
    /// against the user's budget.
    fn authenticate(&self, uid: u32, peer: &Peer<'_>) -> Option<Verdict> {
        let started = Instant::now();
        let who = peer.describe();

        if peer.hung_up() {
            eprintln!("<6>faciolockd: uid {uid}: cancelled by {who} before any work");
            return None;
        }

        // **Before the template load and before the camera.** A locked-out user costs one
        // hash lookup, lights no emitter, and learns nothing about whether they are
        // enrolled. `RateLimited` is not itself an attempt and does not extend the backoff.
        let decision = limiter_of(self).decide(uid);
        if decision.refused() {
            audit::attempt(uid, &who, started.elapsed().as_millis(), None, Some(decision));
            return Some(Verdict::RateLimited);
        }

        let template = match store::load(&self.cfg.templates, uid) {
            Ok(Some(t)) => t,
            // Not enrolled is its own answer, and it is not a failed attempt. It is still
            // an attempt, so R9 still wants a line — at info, because on a machine where
            // nobody enrolled this is every lock screen and it must not train an operator
            // to ignore the level.
            Ok(None) => {
                audit::not_enrolled(uid, &who, started.elapsed().as_millis());
                return Some(Verdict::NoEnrolment);
            }
            Err(e) => {
                // A template that exists and does not decrypt is the interesting case here:
                // damaged store, or somebody trying to implant one. It is an error, never a
                // silent fall-through to the password prompt.
                eprintln!("<3>faciolockd: template store: {e}");
                audit::attempt(uid, &who, started.elapsed().as_millis(), None, None);
                return Some(Verdict::Unavailable);
            }
        };

        // The lock is held for the capture and released before the ~300 ms of inference,
        // so a warm-up still running elsewhere is the only thing an authentication can
        // queue behind, and only for as long as it holds the camera.
        let burst = match session_of(self).burst() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("<4>faciolockd: capture for uid {uid}: {e}");
                audit::attempt(uid, &who, started.elapsed().as_millis(), None, None);
                return Some(Verdict::Unavailable);
            }
        };

        // Between the two costs. The camera is released by returning here, which is the
        // part that matters — a caller that gave up must not keep the device from the next
        // one, or from a video call.
        if peer.hung_up() {
            eprintln!("<6>faciolockd: uid {uid}: cancelled after capture, skipping inference");
            return None;
        }

        let vision = *self.pipeline.config();
        let outcome = match Burst::from_planar(&burst.planar, burst.width, burst.height, &vision) {
            Ok(frames) => self.pipeline.verify(&frames, &template),
            // An unusable burst still produces a verdict, and the ambient gate lives in
            // here — so it still gets an audit line and still goes through the counting
            // rule below, which exempts it.
            Err(e) => faciolock_vision::Outcome {
                verdict: faciolock_vision::Verdict::from(e),
                liveness: Default::default(),
                face: None,
                probes: 0,
            },
        };

        self.record(uid, &outcome.verdict);
        audit::attempt(uid, &who, started.elapsed().as_millis(), Some(&outcome), None);
        Some(serve::to_wire(&outcome.verdict))
    }

    /// Move the user's failure budget, and persist it **before the daemon replies**.
    ///
    /// Ordering is the point: a `kill -9` between the verdict and the answer must not roll
    /// the counter back, or the budget is advisory. That is Phase 4's second exit criterion.
    ///
    /// What counts is [`counts_as_an_attempt`](faciolock_vision::Verdict::counts_as_an_attempt)
    /// and nothing else — a bright room, a busy camera or a malformed burst never looked at
    /// a face (P2-g). A write failure is logged and the answer still goes out: refusing to
    /// answer because the budget could not be written would turn a full disk into a lockout.
    fn record(&self, uid: u32, verdict: &faciolock_vision::Verdict) {
        if !verdict.counts_as_an_attempt() {
            return;
        }
        let mut limiter = limiter_of(self);
        let outcome = match verdict.is_allow() {
            true => limiter.clear(uid).map(|_| ()),
            false => limiter.record_failure(uid).map(|d| {
                if d == Decision::LockedOut {
                    eprintln!("<3>faciolockd: uid {uid}: face unlock disabled until a successful password authentication");
                }
            }),
        };
        if let Err(e) = outcome {
            eprintln!("<3>faciolockd: rate limiter: {e}");
        }
    }

    /// One pose of a guided enrolment (Phase 5). `faciolockctl` sends [`ENROL_POSES`] of
    /// these and the last one writes the template.
    ///
    /// **The liveness gate is verification's gate, unchanged.** `Pipeline::enroll` runs the
    /// same checks `verify` runs, deliberately: a template enrolled from a photograph would
    /// poison the store permanently, and it would do so silently. The honest limit on that
    /// today is [B24] — a flat printed face clears every check the tier enforces — which is
    /// a liveness problem rather than an enrolment one, and it is not hidden here.
    ///
    /// **No path in here touches the failure budget.** Enrolment is not an authentication
    /// attempt, and the peer is root by the time this runs.
    ///
    /// [B24]: ../../../wiki/11-bug-log.md
    fn enrol(&self, uid: u32, peer: &Peer<'_>) -> Option<Verdict> {
        let started = Instant::now();
        let who = peer.describe();

        if peer.hung_up() {
            eprintln!("<6>faciolockd: uid {uid}: enrolment cancelled by {who} before any work");
            return None;
        }

        let burst = match session_of(self).burst() {
            Ok(b) => b,
            Err(e) => {
                let refused = faciolock_vision::Verdict::Failed(e.to_string());
                let ms = started.elapsed().as_millis();
                audit::enrol_pose(uid, &who, ms, self.poses_so_far(uid), Some(&refused));
                return Some(serve::enrol_failure(&refused));
            }
        };

        // Same bracket as `authenticate`: the camera is released by returning here, and a
        // caller that gave up must not hold it away from the next one or from a video call.
        if peer.hung_up() {
            eprintln!("<6>faciolockd: uid {uid}: enrolment cancelled after capture");
            return None;
        }

        let vision = *self.pipeline.config();
        let captured = match Burst::from_planar(&burst.planar, burst.width, burst.height, &vision) {
            Ok(frames) => self.pipeline.enroll(&frames).map_err(|o| o.verdict),
            Err(e) => Err(faciolock_vision::Verdict::from(e)),
        };

        // Taken out of the slot rather than borrowed in it, so the finalise path below can
        // consume the embeddings without a second lookup. Every early return puts it back.
        let mut slot = pending_of(self);
        let mut pending = match slot.take() {
            Some(p) if p.belongs_to(uid) => p,
            // A different user, or a session nobody came back to. Starting over is the only
            // safe reading: appending this pose to someone else's poses would build a
            // template out of two faces.
            _ => Pending::fresh(uid),
        };

        let embeddings = match captured {
            Ok(e) => e,
            Err(verdict) => {
                let ms = started.elapsed().as_millis();
                audit::enrol_pose(uid, &who, ms, pending.poses, Some(&verdict));
                *slot = Some(pending);
                return Some(serve::enrol_failure(&verdict));
            }
        };

        pending.record(embeddings);
        audit::enrol_pose(uid, &who, started.elapsed().as_millis(), pending.poses, None);

        if pending.poses < ENROL_POSES {
            *slot = Some(pending);
            return Some(Verdict::Ack);
        }

        // Last pose. `pending` is dropped from here on whatever happens — a failed write must
        // not leave two poses behind for the next request to append a third to.
        //
        // The slot is released before the store write so no path in this daemon ever holds
        // the pending lock and the limiter lock at once. Serial today, and a lock ordering
        // that does not exist cannot be got wrong later.
        drop(slot);

        let Some(template) = Template::new(self.pipeline.embedder_id(), pending.embeddings) else {
            eprintln!("<3>faciolockd: uid {uid}: enrolment collected no embeddings");
            return Some(Verdict::Unavailable);
        };
        let embeddings = template.len();

        match store::save(&self.cfg.templates, uid, &template) {
            Ok(_) => {
                // A fresh enrolment clears whatever the old one accumulated. Otherwise
                // re-enrolling to fix a bad template would land the user straight back in a
                // lockout earned by the template they just replaced.
                if let Err(e) = limiter_of(self).clear(uid) {
                    eprintln!("<4>faciolockd: rate limiter: {e}");
                }
                audit::enrolled(uid, &who, embeddings);
                Some(Verdict::Ack)
            }
            Err(e) => {
                eprintln!("<3>faciolockd: template store: {e}");
                Some(Verdict::Unavailable)
            }
        }
    }

    /// Poses recorded for this user, or zero if the last one was somebody else's or has
    /// expired. For the audit line on a pose that never reached the pending state.
    fn poses_so_far(&self, uid: u32) -> usize {
        pending_of(self).as_ref().filter(|p| p.belongs_to(uid)).map_or(0, |p| p.poses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Daemon::enrol` needs a camera, two models and a store, so what is testable without
    /// hardware is the rule it turns on: whether a pose in hand belongs to the enrolment
    /// already in progress. Both ways of getting that wrong build a template out of two
    /// people, which is the failure this exists to prevent.
    #[test]
    fn a_pose_only_joins_an_enrolment_it_belongs_to() {
        let pending = Pending::fresh(1000);
        assert!(pending.belongs_to(1000));
        assert!(!pending.belongs_to(1001), "another user's pose must not be appended");
        assert!(!pending.belongs_to(0));
    }

    /// A session somebody walked away from must not sit waiting to have the next person's
    /// face added to it. `checked_sub` rather than a sleep, so the test costs nothing.
    #[test]
    fn an_abandoned_enrolment_expires_rather_than_waiting_forever() {
        let mut stale = Pending::fresh(1000);
        let past = Instant::now().checked_sub(ENROL_EXPIRY + Duration::from_secs(1));
        stale.touched = match past {
            Some(t) => t,
            // A monotonic clock younger than the expiry window — a machine up for under a
            // minute. Nothing to assert then, and skipping beats asserting something false.
            None => return,
        };
        assert!(!stale.belongs_to(1000), "an expired enrolment must not accept a fourth pose");

        let fresh = Pending::fresh(1000);
        assert!(fresh.belongs_to(1000));
    }

    /// **The expiry is a gap between poses, not a budget for the whole enrolment.**
    ///
    /// As a total it fails silently and in the worst direction: the third pose arrives to an
    /// expired `Pending`, the daemon starts over and answers `Ack`, the CLI counts its third
    /// `Ack` and says "Enrolled", and no template exists. The user finds out at a lock
    /// screen. A pose landing has to push the deadline out.
    #[test]
    fn a_slow_enrolment_survives_as_long_as_poses_keep_arriving() {
        let mut pending = Pending::fresh(1000);
        let Some(past) = Instant::now().checked_sub(ENROL_EXPIRY - Duration::from_secs(1)) else {
            return; // clock younger than the window; nothing to assert
        };

        // Two poses in, and the enrolment began just inside the window.
        pending.touched = past;
        assert!(pending.belongs_to(1000), "still alive, barely");

        // A pose lands. That must reset the clock, not merely be recorded.
        pending.record(Vec::new());
        assert_eq!(pending.poses, 1);
        assert!(
            pending.touched.elapsed() < Duration::from_secs(1),
            "a pose must push the deadline out, or a slow enrolment silently restarts"
        );
        assert!(pending.belongs_to(1000));
    }

    /// The count the daemon finalises on is the count the CLI prompts for. They are the same
    /// constant precisely so this cannot drift, and the shared crate is where it lives.
    #[test]
    fn the_finalise_point_is_the_shared_constant() {
        assert!(ENROL_POSES >= 2, "one pose is what the flag this replaced already did");
        assert_eq!(ENROL_POSES, faciolock_ipc::ENROL_POSES);
    }
}

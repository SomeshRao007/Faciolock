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
use std::time::Instant;

use faciolock_ipc::{Op, Request, Verdict};
use faciolock_vision::{Burst, Config as VisionConfig, Pipeline, pipeline::enroll_template};

use config::Config;
use ratelimit::{Decision, Limiter};
use serve::Peer;
use session::Session;

const DEFAULT_CONFIG: &str = "/etc/faciolock/faciolockd.toml";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut config_path = PathBuf::from(DEFAULT_CONFIG);
    let mut enroll_as: Option<String> = None;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => match it.next() {
                Some(v) => config_path = PathBuf::from(v),
                None => return fail("--config needs a path"),
            },
            "--enroll" => match it.next() {
                Some(v) => enroll_as = Some(v.clone()),
                None => return fail("--enroll needs a username"),
            },
            "--help" | "-h" => {
                println!("faciolockd [--config PATH] [--enroll USERNAME]");
                return ExitCode::SUCCESS;
            }
            other => return fail(&format!("unknown argument {other}")),
        }
    }

    match start(&config_path) {
        Ok(daemon) => match enroll_as {
            Some(user) => daemon.enroll_locally(&user),
            None => daemon.serve(),
        },
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
    pipeline: Pipeline,
    cfg: Config,
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
            Op::Enroll => {
                eprintln!("<4>faciolockd: refused enrolment over the socket for uid {uid}");
                Some(serve::enroll_over_the_wire())
            }
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

    /// Enrol from the command line.
    ///
    /// ponytail: the smallest thing that makes `Op::Auth` testable end to end. Phase 5's
    /// `faciolockctl enroll` replaces it with guided multi-capture and quality feedback;
    /// this takes one burst and accepts whatever the liveness gate lets through — which is
    /// the same gate verification uses, deliberately, because enrolling from a photograph
    /// would poison the store permanently.
    fn enroll_locally(self, user: &str) -> ExitCode {
        // Reuse the wire's charset rule rather than re-implementing it: a username that
        // could never arrive over the socket must not be enrollable from argv either.
        if Request::new(Op::Enroll, user, 0).is_err() {
            return fail(&format!("{user:?} is not a usable username"));
        }
        let Some(uid) = users::uid_of(user) else {
            return fail(&format!("no such user: {user}"));
        };

        let burst = match session_of(&self).burst() {
            Ok(b) => b,
            Err(e) => return fail(&format!("capture: {e}")),
        };
        let vision = *self.pipeline.config();
        let frames = match Burst::from_planar(&burst.planar, burst.width, burst.height, &vision) {
            Ok(b) => b,
            Err(e) => return fail(&format!("{:?}", faciolock_vision::Verdict::from(e))),
        };

        match enroll_template(&self.pipeline, &frames) {
            Ok(template) => match store::save(&self.cfg.templates, uid, &template) {
                Ok(path) => {
                    // A fresh enrolment clears whatever the old one accumulated. Otherwise
                    // re-enrolling to fix a bad template would land the user straight back
                    // in a lockout earned by the template they just replaced.
                    if let Err(e) = limiter_of(&self).clear(uid) {
                        eprintln!("<4>faciolockd: rate limiter: {e}");
                    }
                    println!(
                        "enrolled {user} (uid {uid}): {} embeddings -> {}",
                        template.len(),
                        path.display()
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => fail(&e),
            },
            Err(outcome) => fail(&format!("{:?}", outcome.verdict)),
        }
    }
}

//! `faciolockd` — the unprivileged daemon that owns the camera and the ML.
//!
//! wiki/04's privilege split lives here by *absence*: this process holds the V4L2 device,
//! two ONNX models and the template store, and the PAM module holds a socket. Nothing in
//! this binary runs as root.
//!
//! ## Phase 2 exit criterion 4
//!
//! "`faciolockd` answers a hand-rolled socket request with a real verdict from the real
//! camera." That is what this is. Deliberately **not** here, because Phase 3 and 4 own them
//! and building them early would freeze decisions the measurements have not made yet
//! (wiki/09):
//!
//! - `SO_PEERCRED`, remote-session rejection, cancellation — Phase 3, and they belong in
//!   [`serve`], on the daemon side of the socket.
//! - Rate limiting, lockout, audit logging, TPM sealing — Phase 4. In particular the
//!   failure-budget policy is **not** on the wire ([B20] reopened the `Environment`
//!   exemption and it has to stay re-decidable without a protocol break — wiki/13).
//! - Socket activation and idle-exit — Phase 6.
//!
//! [B20]: ../../../wiki/11-bug-log.md

mod config;
mod models;
mod serve;
mod session;
mod store;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use faciolock_ipc::{Op, Request, Verdict};
use faciolock_vision::{Burst, Config as VisionConfig, Pipeline, pipeline::enroll_template};

use config::Config;
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

    Ok(Daemon { session: Arc::new(Mutex::new(Session::new(cfg.clone()))), pipeline, cfg })
}

pub struct Daemon {
    /// Shared with the startup warm thread, and the only mutable state the daemon has.
    session: Arc<Mutex<Session>>,
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
        match req.op {
            Op::Auth => self.authenticate(user, peer),
            Op::Status => {
                let enrolled = store::load(&self.cfg.templates, user)
                    .inspect_err(|e| eprintln!("faciolockd: template store: {e}"))
                    .ok()
                    .flatten()
                    .is_some();
                Some(serve::status(enrolled))
            }
            Op::Enroll => {
                eprintln!("faciolockd: refused enrolment over the socket for {user}");
                Some(serve::enroll_over_the_wire())
            }
        }
    }

    /// The two `hung_up` checks bracket the two expensive stages — ~400 ms of capture and
    /// ~300 ms of inference. They are checks, not interrupts: a burst already in flight
    /// finishes (decision P3-b).
    fn authenticate(&self, user: &str, peer: &Peer<'_>) -> Option<Verdict> {
        if peer.hung_up() {
            eprintln!("faciolockd: {user}: cancelled by {} before any work", peer.describe());
            return None;
        }

        let template = match store::load(&self.cfg.templates, user) {
            Ok(Some(t)) => t,
            // Not enrolled is its own answer, and it is not a failed attempt.
            Ok(None) => return Some(Verdict::NoEnrolment),
            Err(e) => {
                eprintln!("faciolockd: template store: {e}");
                return Some(Verdict::Unavailable);
            }
        };

        // The lock is held for the capture and released before the ~300 ms of inference,
        // so a warm-up still running elsewhere is the only thing an authentication can
        // queue behind, and only for as long as it holds the camera.
        let burst = match session_of(self).burst() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("faciolockd: capture for {user}: {e}");
                return Some(Verdict::Unavailable);
            }
        };

        // Between the two costs. The camera is released by returning here, which is the
        // part that matters — a caller that gave up must not keep the device from the next
        // one, or from a video call.
        if peer.hung_up() {
            eprintln!("faciolockd: {user}: cancelled after capture, skipping inference");
            return None;
        }

        let vision = *self.pipeline.config();
        let frames = match Burst::from_planar(&burst.planar, burst.width, burst.height, &vision) {
            Ok(b) => b,
            Err(e) => {
                let verdict = faciolock_vision::Verdict::from(e);
                eprintln!("faciolockd: {user}: {verdict:?}");
                return Some(serve::to_wire(&verdict));
            }
        };

        let outcome = self.pipeline.verify(&frames, &template);
        // Phase 4 turns this into a structured journal entry with the liveness report and
        // the peer credentials. For now it is one line, and it is the *detailed* verdict —
        // the narrow one goes on the wire, because telling a caller which check rejected
        // them is telling an attacker what to fix (wiki/04).
        eprintln!(
            "faciolockd: {user}: {:?} probes={} dropped={} discarded={} span={}ms",
            outcome.verdict,
            outcome.probes,
            burst.dropped,
            burst.discarded,
            burst.sensor_span().as_millis()
        );
        Some(serve::to_wire(&outcome.verdict))
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
            Ok(template) => match store::save(&self.cfg.templates, user, &template) {
                Ok(path) => {
                    println!(
                        "enrolled {user}: {} embeddings -> {}",
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

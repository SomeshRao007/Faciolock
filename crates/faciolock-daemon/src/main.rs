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
use std::sync::Mutex;
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

fn start(config_path: &Path) -> Result<Daemon, String> {
    let cfg = Config::load(config_path)?;
    models::verify(&cfg.models)?;

    let mut vision = VisionConfig::default();

    vision.tier = cfg.tier()?;

    vision.agc_settle_frames = 0;

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
        session: Mutex::new(Session::new(cfg.clone())),
        limiter,
        pending: Mutex::new(None),
        pipeline,
        cfg,
    })
}

pub struct Daemon {
    session: Mutex<Session>,
    limiter: Mutex<Limiter>,
    pending: Mutex<Option<Pending>>,
    pipeline: Pipeline,
    cfg: Config,
}

const ENROL_EXPIRY: Duration = Duration::from_secs(60);

struct Pending {
    uid: u32,
    embeddings: Vec<Embedding>,
    poses: usize,
    touched: Instant,
}

impl Pending {
    fn fresh(uid: u32) -> Pending {
        Pending { uid, embeddings: Vec::new(), poses: 0, touched: Instant::now() }
    }

    fn belongs_to(&self, uid: u32) -> bool {
        self.uid == uid && self.touched.elapsed() < ENROL_EXPIRY
    }

    fn record(&mut self, embeddings: Vec<Embedding>) {
        self.embeddings.extend(embeddings);
        self.poses += 1;
        self.touched = Instant::now();
    }
}

fn may_exit(pending: Option<&Pending>) -> bool {
    pending.is_none_or(|p| p.touched.elapsed() >= ENROL_EXPIRY)
}

fn session_of(daemon: &Daemon) -> std::sync::MutexGuard<'_, Session> {
    daemon.session.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn limiter_of(daemon: &Daemon) -> std::sync::MutexGuard<'_, Limiter> {
    daemon.limiter.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn pending_of(daemon: &Daemon) -> std::sync::MutexGuard<'_, Option<Pending>> {
    daemon.pending.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Daemon {
    pub fn may_idle_exit(&self) -> bool {
        may_exit(pending_of(self).as_ref())
    }

    fn serve(self) -> ExitCode {
        let (listener, ours) = match serve::inherited() {
            Ok(Some(l)) => (l, false),
            Ok(None) => match serve::listen(&self.cfg.socket) {
                Ok(l) => (l, true),
                Err(e) => return fail(&e),
            },
            Err(e) => return fail(&e),
        };
        let idle = if ours { None } else { self.cfg.idle_timeout() };
        eprintln!(
            "faciolockd: listening on {} ({}), idle-exit {}",
            self.cfg.socket.display(),
            if ours { "self-bound" } else { "socket-activated" },
            idle.map_or("off".into(), |d| format!("{} s", d.as_secs()))
        );

        serve::run(&listener, &self, idle);

        if ours {
            let _ = std::fs::remove_file(&self.cfg.socket);
        }
        ExitCode::SUCCESS
    }

    pub fn answer(&self, req: &Request, peer: &Peer<'_>) -> Option<Verdict> {
        let user = req.username();

        let subject = users::uid_of(user);

        if !serve::may_ask(peer.uid(), subject, req.op) {
            eprintln!("<4>faciolockd: refused {} — {:?} for {user:?}", peer.describe(), req.op);
            return None;
        }

        let Some(uid) = subject else {
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

    fn authenticate(&self, uid: u32, peer: &Peer<'_>) -> Option<Verdict> {
        let started = Instant::now();
        let who = peer.describe();

        if peer.hung_up() {
            eprintln!("<6>faciolockd: uid {uid}: cancelled by {who} before any work");
            return None;
        }

        let decision = limiter_of(self).decide(uid);
        if decision.refused() {
            audit::attempt(uid, &who, started.elapsed().as_millis(), None, Some(decision));
            return Some(Verdict::RateLimited);
        }

        let template = match store::load(&self.cfg.templates, uid) {
            Ok(Some(t)) => t,
            Ok(None) => {
                audit::not_enrolled(uid, &who, started.elapsed().as_millis());
                return Some(Verdict::NoEnrolment);
            }
            Err(e) => {
                eprintln!("<3>faciolockd: template store: {e}");
                audit::attempt(uid, &who, started.elapsed().as_millis(), None, None);
                return Some(Verdict::Unavailable);
            }
        };

        let burst = match session_of(self).burst() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("<4>faciolockd: capture for uid {uid}: {e}");
                audit::attempt(uid, &who, started.elapsed().as_millis(), None, None);
                return Some(Verdict::Unavailable);
            }
        };

        if peer.hung_up() {
            eprintln!("<6>faciolockd: uid {uid}: cancelled after capture, skipping inference");
            return None;
        }

        let vision = *self.pipeline.config();
        let outcome = match Burst::from_planar(&burst.planar, burst.width, burst.height, &vision) {
            Ok(frames) => self.pipeline.verify(&frames, &template),
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

        if peer.hung_up() {
            eprintln!("<6>faciolockd: uid {uid}: enrolment cancelled after capture");
            return None;
        }

        let vision = *self.pipeline.config();
        let captured = match Burst::from_planar(&burst.planar, burst.width, burst.height, &vision) {
            Ok(frames) => self.pipeline.enroll(&frames).map_err(|o| o.verdict),
            Err(e) => Err(faciolock_vision::Verdict::from(e)),
        };

        let mut slot = pending_of(self);
        let mut pending = match slot.take() {
            Some(p) if p.belongs_to(uid) => p,
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

        drop(slot);

        let Some(template) = Template::new(self.pipeline.embedder_id(), pending.embeddings) else {
            eprintln!("<3>faciolockd: uid {uid}: enrolment collected no embeddings");
            return Some(Verdict::Unavailable);
        };
        let embeddings = template.len();

        match store::save(&self.cfg.templates, uid, &template) {
            Ok(_) => {
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

    fn poses_so_far(&self, uid: u32) -> usize {
        pending_of(self).as_ref().filter(|p| p.belongs_to(uid)).map_or(0, |p| p.poses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pose_only_joins_an_enrolment_it_belongs_to() {
        let pending = Pending::fresh(1000);
        assert!(pending.belongs_to(1000));
        assert!(!pending.belongs_to(1001), "another user's pose must not be appended");
        assert!(!pending.belongs_to(0));
    }

    #[test]
    fn an_abandoned_enrolment_expires_rather_than_waiting_forever() {
        let mut stale = Pending::fresh(1000);
        let past = Instant::now().checked_sub(ENROL_EXPIRY + Duration::from_secs(1));
        stale.touched = match past {
            Some(t) => t,
            None => return,
        };
        assert!(!stale.belongs_to(1000), "an expired enrolment must not accept a fourth pose");

        let fresh = Pending::fresh(1000);
        assert!(fresh.belongs_to(1000));
    }

    #[test]
    fn a_slow_enrolment_survives_as_long_as_poses_keep_arriving() {
        let mut pending = Pending::fresh(1000);
        let Some(past) = Instant::now().checked_sub(ENROL_EXPIRY - Duration::from_secs(1)) else {
            return;
        };

        pending.touched = past;
        assert!(pending.belongs_to(1000), "still alive, barely");

        pending.record(Vec::new());
        assert_eq!(pending.poses, 1);
        assert!(
            pending.touched.elapsed() < Duration::from_secs(1),
            "a pose must push the deadline out, or a slow enrolment silently restarts"
        );
        assert!(pending.belongs_to(1000));
    }

    #[test]
    fn an_enrolment_in_flight_keeps_the_daemon_alive() {
        assert!(may_exit(None), "an idle daemon with nothing pending exits");

        let mid = Pending::fresh(1000);
        assert!(!may_exit(Some(&mid)), "two poses in is not idle");

        let mut stale = Pending::fresh(1000);
        let Some(past) = Instant::now().checked_sub(ENROL_EXPIRY + Duration::from_secs(1)) else {
            return;
        };
        stale.touched = past;
        assert!(may_exit(Some(&stale)), "an abandoned enrolment is not worth staying up for");
    }

    #[test]
    fn the_finalise_point_is_the_shared_constant() {
        assert!(ENROL_POSES >= 2, "one pose is what the flag this replaced already did");
        assert_eq!(ENROL_POSES, faciolock_ipc::ENROL_POSES);
    }
}

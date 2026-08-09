//! The camera session: one warm stream, reopened when the driver takes it away.
//!
//! **The camera is not a startup precondition.** A daemon that refuses to start because a
//! video call holds the device takes the socket down with it, and PAM then gets a failed
//! `connect()` rather than a fast `Unavailable` — the same outcome for the user, with the
//! socket unit flapping underneath. So the stream is opened lazily and every failure to
//! open is answered, not fatal. That is wiki/09's "reply fast, don't fight for the device"
//! read as a lifecycle rule rather than only as an error mapping.
//!
//! Keeping the stream between requests is what buys wiki/06's warm path: the AGC settle
//! frames (wiki/10 Probe 3) are paid once, here, instead of inside an authentication.

use std::path::PathBuf;

use faciolock_camera::{Capture, CaptureError, CapturedBurst, discover, open};

use crate::config::Config;

pub struct Session {
    capture: Option<Capture>,
    device: Option<PathBuf>,
    cfg: Config,
}

impl Session {
    pub fn new(cfg: Config) -> Session {
        Session { capture: None, device: cfg.device.clone(), cfg }
    }

    /// The node this session is streaming from, once it has one. For the startup log.
    pub fn device(&self) -> Option<&PathBuf> {
        self.device.as_ref()
    }

    /// Capture a burst, opening or reopening the stream if that is what it takes.
    ///
    /// One retry, and exactly one: [`CaptureError::is_fatal`] means the streaming fd is
    /// unusable — a suspend/resume invalidates it with `ENODEV` — and reopening is the
    /// documented recovery. Retrying a second time would just be a slower way to fail while
    /// holding an authentication open.
    pub fn burst(&mut self) -> Result<CapturedBurst, CaptureError> {
        if self.capture.is_none() {
            self.capture = Some(self.start()?);
        }
        let deadline = self.cfg.capture_deadline();
        let want = self.cfg.burst_frames;

        let first = self.capture.as_mut().expect("just opened").burst(want, deadline);
        match first {
            Ok(b) => Ok(b),
            Err(e) if e.is_fatal() => {
                self.capture = None;
                let mut fresh = self.start()?;
                let out = fresh.burst(want, deadline);
                if out.is_ok() {
                    self.capture = Some(fresh);
                }
                out
            }
            Err(e) => Err(e),
        }
    }

    fn start(&mut self) -> Result<Capture, CaptureError> {
        let node = match &self.device {
            // The configured value is a `by-path` symlink; resolving it every time is what
            // makes the config survive the renumbering that made `by-id` wrong (wiki/10
            // Probe 4).
            Some(path) => path.clone(),
            None => {
                let selected = discover().map_err(|e| {
                    CaptureError::Device(std::io::Error::other(e.to_string()))
                })?;
                // Persist the stable name if the machine has one, so discovery runs once
                // per process rather than once per burst.
                self.device = Some(selected.by_path.clone().unwrap_or(selected.node.clone()));
                selected.node
            }
        };
        let dev = open(&node, self.cfg.geometry())?;
        Capture::start(dev, self.cfg.geometry(), self.cfg.warmup_frames)
    }
}

use std::path::PathBuf;
use std::time::{Duration, Instant};

use faciolock_camera::{Capture, CaptureError, CapturedBurst, discover, open};

use crate::config::Config;

pub struct Session {
    device: Option<PathBuf>,
    cfg: Config,
}

impl Session {
    pub fn new(cfg: Config) -> Session {
        Session { device: cfg.device.clone(), cfg }
    }

    pub fn burst(&mut self) -> Result<CapturedBurst, CaptureError> {
        let until = Instant::now() + Duration::from_secs(6);
        let mut capture = self.start(until)?;
        let want = self.cfg.burst_frames;

        let first = capture.burst(want, self.remaining(until));
        match first {
            Ok(b) => Ok(b),
            Err(e) if e.is_fatal() => {
                drop(capture);
                let mut fresh = self.start(until)?;
                fresh.burst(want, self.remaining(until))
            }
            Err(e) => Err(e),
        }
    }

    fn remaining(&self, until: Instant) -> Duration {
        self.cfg.capture_deadline().min(until.saturating_duration_since(Instant::now()))
    }

    fn start(&mut self, until: Instant) -> Result<Capture, CaptureError> {
        let node = match &self.device {
            Some(path) => path.clone(),
            None => {
                let selected = discover().map_err(|e| {
                    CaptureError::Device(std::io::Error::other(e.to_string()))
                })?;
                self.device = Some(selected.by_path.clone().unwrap_or(selected.node.clone()));
                selected.node
            }
        };
        let dev = open(&node, self.cfg.geometry())?;
        Capture::start(dev, self.cfg.geometry(), self.cfg.warmup_frames, until)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_burst_uses_only_the_time_left_in_the_attempt() -> Result<(), String> {
        let session = Session::new(Config::default());
        let near = Instant::now() + Duration::from_millis(10);
        if session.remaining(near) > Duration::from_millis(10) {
            return Err("a burst could run past the total attempt limit".into());
        }
        let expired = Instant::now() - Duration::from_millis(1);
        if !session.remaining(expired).is_zero() {
            return Err("an expired attempt must not start another burst".into());
        }
        let far = Instant::now() + Duration::from_secs(6);
        if session.remaining(far) > session.cfg.capture_deadline() {
            return Err("a burst could exceed its configured capture deadline".into());
        }
        Ok(())
    }
}

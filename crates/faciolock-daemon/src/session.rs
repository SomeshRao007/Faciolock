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

    pub fn device(&self) -> Option<&PathBuf> {
        self.device.as_ref()
    }

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
        Capture::start(dev, self.cfg.geometry(), self.cfg.warmup_frames)
    }
}

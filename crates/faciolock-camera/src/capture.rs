use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use v4l::buffer::{Flags as BufFlags, Type};
use v4l::device::Handle;
use v4l::io::mmap::Stream as MmapStream;
use v4l::io::traits::CaptureStream;
use v4l::Device;

use crate::enumerate::GREY;

const BUFFERS: u32 = 4;

const STREAM_TIMEOUT: Duration = Duration::from_millis(2_000);

const EINTR_RETRIES: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
}

impl Geometry {
    pub const fn new(width: u32, height: u32) -> Geometry {
        Geometry { width, height }
    }
    pub const fn stride(&self) -> usize {
        self.width as usize * self.height as usize
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FrameMeta {
    pub sequence: u32,
    pub timestamp: Duration,
}

pub struct CapturedBurst {
    pub planar: Vec<u8>,
    pub meta: Vec<FrameMeta>,
    pub width: usize,
    pub height: usize,
    pub discarded: usize,
    pub dropped: u32,
}

impl CapturedBurst {
    pub fn sensor_span(&self) -> Duration {
        match (self.meta.first(), self.meta.last()) {
            (Some(a), Some(b)) => b.timestamp.saturating_sub(a.timestamp),
            _ => Duration::ZERO,
        }
    }
}

#[derive(Debug)]
pub enum CaptureError {
    Busy,
    Stalled { got: usize, want: usize },
    NotCapture,
    WrongFormat { got: String },
    WrongGeometry { want: Geometry, got: Geometry },
    Device(io::Error),
}

impl CaptureError {
    pub(crate) fn from_io(e: io::Error) -> CaptureError {
        match e.raw_os_error() {
            Some(libc_ebusy) if libc_ebusy == 16 => CaptureError::Busy,
            _ => CaptureError::Device(e),
        }
    }

    pub fn is_fatal(&self) -> bool {
        match self {
            CaptureError::Stalled { .. } | CaptureError::Device(_) => true,
            CaptureError::Busy
            | CaptureError::NotCapture
            | CaptureError::WrongFormat { .. }
            | CaptureError::WrongGeometry { .. } => false,
        }
    }
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureError::Busy => write!(f, "camera is in use by another process"),
            CaptureError::Stalled { got, want } => {
                write!(f, "camera stalled: {got} of {want} frames before the deadline")
            }
            CaptureError::NotCapture => write!(f, "not a video-capture device"),
            CaptureError::WrongFormat { got } => write!(f, "device offers {got}, not GREY"),
            CaptureError::WrongGeometry { want, got } => write!(
                f,
                "device gave {}x{}, expected {}x{}",
                got.width, got.height, want.width, want.height
            ),
            CaptureError::Device(e) => write!(f, "device error: {e}"),
        }
    }
}

pub struct Capture {
    stream: MmapStream<'static>,
    handle: Arc<Handle>,
    _dev: Device,
    geom: Geometry,
}

impl Capture {
    pub fn start(dev: Device, geom: Geometry, warmup: usize) -> Result<Capture, CaptureError> {
        let handle = dev.handle();
        let mut stream = MmapStream::with_buffers(&dev, Type::VideoCapture, BUFFERS)
            .map_err(CaptureError::from_io)?;
        stream.set_timeout(STREAM_TIMEOUT);

        let mut cap = Capture { stream, handle, _dev: dev, geom };
        for _ in 0..warmup.max(1) {
            cap.take(Instant::now() + STREAM_TIMEOUT)?;
        }
        Ok(cap)
    }

    pub fn geometry(&self) -> Geometry {
        self.geom
    }

    pub fn burst(&mut self, want: usize, deadline: Duration) -> Result<CapturedBurst, CaptureError> {
        let until = Instant::now() + deadline;
        self.flush()?;

        let stride = self.geom.stride();
        let mut planar = Vec::with_capacity(want * stride);
        let mut meta: Vec<FrameMeta> = Vec::with_capacity(want);
        let mut discarded = 0usize;
        let mut dropped = 0u32;

        while meta.len() < want {
            let (bytes, m) = match self.take(until) {
                Ok(v) => v,
                Err(CaptureError::Stalled { .. }) => {
                    return Err(CaptureError::Stalled { got: meta.len(), want });
                }
                Err(e) => return Err(e),
            };

            if m.flagged || m.bytes < stride {
                discarded += 1;
                continue;
            }

            if let Some(prev) = meta.last() {
                dropped += m.sequence.saturating_sub(prev.sequence).saturating_sub(1);
            }
            planar.extend_from_slice(&bytes[..stride]);
            meta.push(FrameMeta { sequence: m.sequence, timestamp: m.timestamp });
        }

        Ok(CapturedBurst {
            planar,
            meta,
            width: self.geom.width as usize,
            height: self.geom.height as usize,
            discarded,
            dropped,
        })
    }

    fn flush(&mut self) -> Result<(), CaptureError> {
        while self.ready(0)? {
            self.next_raw()?;
        }
        Ok(())
    }

    fn take(&mut self, until: Instant) -> Result<(Vec<u8>, RawMeta), CaptureError> {
        self.wait_ready(until)?;
        self.next_raw()
    }

    fn wait_ready(&mut self, until: Instant) -> Result<(), CaptureError> {
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(CaptureError::Stalled { got: 0, want: 1 });
            }
            let ms = i32::try_from(left.as_millis()).unwrap_or(i32::MAX).max(1);
            if self.ready(ms)? {
                return Ok(());
            }
        }
    }

    fn ready(&self, timeout_ms: i32) -> Result<bool, CaptureError> {
        for _ in 0..EINTR_RETRIES {
            match self.handle.poll(libc_pollin(), timeout_ms) {
                Ok(n) => return Ok(n > 0),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(CaptureError::from_io(e)),
            }
        }
        Err(CaptureError::Device(io::Error::from(io::ErrorKind::Interrupted)))
    }

    fn next_raw(&mut self) -> Result<(Vec<u8>, RawMeta), CaptureError> {
        for _ in 0..EINTR_RETRIES {
            match CaptureStream::next(&mut self.stream) {
                Ok((bytes, m)) => {
                    return Ok((
                        bytes.to_vec(),
                        RawMeta {
                            sequence: m.sequence,
                            timestamp: Duration::new(
                                m.timestamp.sec.max(0) as u64,
                                (m.timestamp.usec.clamp(0, 999_999) as u32) * 1_000,
                            ),
                            bytes: m.bytesused as usize,
                            flagged: m.flags.contains(BufFlags::ERROR),
                        },
                    ));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(CaptureError::from_io(e)),
            }
        }
        Err(CaptureError::Device(io::Error::from(io::ErrorKind::Interrupted)))
    }
}

struct RawMeta {
    sequence: u32,
    timestamp: Duration,
    bytes: usize,
    flagged: bool,
}

const fn libc_pollin() -> i16 {
    0x001
}

pub const REQUIRED_FORMAT: v4l::FourCC = GREY;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stride_is_the_frame_size_the_corpus_uses() {
        assert_eq!(Geometry::new(640, 360).stride(), 230_400);
    }

    #[test]
    fn only_stream_failures_are_session_fatal() {
        assert!(CaptureError::Stalled { got: 0, want: 6 }.is_fatal());
        assert!(CaptureError::Device(io::Error::from(io::ErrorKind::BrokenPipe)).is_fatal());
        assert!(!CaptureError::Busy.is_fatal());
        assert!(!CaptureError::NotCapture.is_fatal());
        assert!(!CaptureError::WrongFormat { got: "YUYV".into() }.is_fatal());
    }

    #[test]
    fn ebusy_is_recognised_and_not_confused_with_a_device_error() {
        let busy = CaptureError::from_io(io::Error::from_raw_os_error(16));
        assert!(matches!(busy, CaptureError::Busy));
        let enodev = CaptureError::from_io(io::Error::from_raw_os_error(19));
        assert!(matches!(enodev, CaptureError::Device(_)));
    }

    #[test]
    fn a_burst_reports_the_span_the_sensor_saw() {
        let burst = CapturedBurst {
            planar: Vec::new(),
            meta: vec![
                FrameMeta { sequence: 10, timestamp: Duration::from_millis(1_000) },
                FrameMeta { sequence: 15, timestamp: Duration::from_millis(1_167) },
            ],
            width: 640,
            height: 360,
            discarded: 0,
            dropped: 4,
        };
        assert_eq!(burst.sensor_span(), Duration::from_millis(167));
    }
}

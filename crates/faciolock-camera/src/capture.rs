//! Streaming frames, and proving they are the frames the sensor produced.
//!
//! wiki/10 Probe 3c cost a corpus: `ffmpeg` silently duplicated every frame when writing
//! to disk, the file was exactly the expected size, every frame was a valid image, and the
//! ON/OFF pairing that every liveness check depends on was wrong. The general lesson it
//! left is a requirement on this module — **never assume the frames you were handed are
//! the frames the sensor produced.** So every frame carries its kernel sequence number and
//! timestamp up to the caller, gaps are counted rather than smoothed over, and frames the
//! driver flagged as errored are discarded rather than measured.

use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use v4l::buffer::{Flags as BufFlags, Type};
use v4l::device::Handle;
use v4l::io::mmap::Stream as MmapStream;
use v4l::io::traits::CaptureStream;
use v4l::Device;

use crate::enumerate::GREY;

/// How many driver buffers to request.
///
/// Enough that the sensor always has somewhere to write while we memcpy the previous
/// frame; few enough that the stale backlog flushed at request time is small. The driver
/// may grant fewer than asked, which is why nothing here is indexed by a buffer count.
const BUFFERS: u32 = 4;

/// Backstop only. Every wait is bounded by [`Capture::wait_ready`] polling with the real
/// deadline, so this exists purely so that a poll/DQBUF race cannot become an infinite
/// block — `v4l` 0.14 defaults to waiting forever, which in an authentication path would
/// hang `sudo`.
const STREAM_TIMEOUT: Duration = Duration::from_millis(2_000);

/// `EINTR` is not an error, it is a signal arriving at an inconvenient moment. `v4l` 0.14
/// retries it nowhere, so it is retried here. Bounded, because an unbounded retry loop
/// against a genuinely broken device is a hang wearing a different hat.
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

/// What the kernel said about one frame.
#[derive(Clone, Copy, Debug)]
pub struct FrameMeta {
    /// The driver's frame counter. Gaps mean frames the sensor produced and we never saw.
    pub sequence: u32,
    /// When the driver stamped the frame, on `CLOCK_MONOTONIC`. This — not our wall clock
    /// on either side of the capture — is what says whether a frame was exposed before or
    /// after an authentication request arrived.
    pub timestamp: Duration,
}

/// A burst of frames, back to back, with everything needed to audit it.
pub struct CapturedBurst {
    /// Frames concatenated, `width * height` bytes each — the shape
    /// `faciolock_vision::Burst::from_planar` consumes.
    pub planar: Vec<u8>,
    pub meta: Vec<FrameMeta>,
    pub width: usize,
    pub height: usize,
    /// Frames the driver handed us and we refused: `V4L2_BUF_FLAG_ERROR`, or short.
    pub discarded: usize,
    /// Frames the sensor produced that never reached us, counted from sequence gaps.
    /// A burst with gaps spans more wall-clock time than its frame count implies, which
    /// would otherwise inflate the latency measurement silently.
    pub dropped: u32,
}

impl CapturedBurst {
    /// Wall-clock span from the first frame's exposure to the last, per the kernel.
    pub fn sensor_span(&self) -> Duration {
        match (self.meta.first(), self.meta.last()) {
            (Some(a), Some(b)) => b.timestamp.saturating_sub(a.timestamp),
            _ => Duration::ZERO,
        }
    }
}

#[derive(Debug)]
pub enum CaptureError {
    /// The device is in use — a video call, usually. Arrives at `REQBUFS` or `STREAMON`,
    /// not necessarily at `open`, because UVC permits multiple opens. Answer fast and do
    /// not fight for the device (wiki/04).
    Busy,
    /// The deadline passed with too few usable frames.
    ///
    /// **Session-fatal, not retryable.** See [`Capture::wait_ready`].
    Stalled { got: usize, want: usize },
    /// The node is not a video-capture device.
    NotCapture,
    WrongFormat { got: String },
    WrongGeometry { want: Geometry, got: Geometry },
    /// Anything else the driver said. The session is torn down and reopened rather than
    /// retried in place: a suspend/resume cycle invalidates a streaming fd with `ENODEV`
    /// and no amount of retrying at this level fixes that.
    Device(io::Error),
}

impl CaptureError {
    pub(crate) fn from_io(e: io::Error) -> CaptureError {
        match e.raw_os_error() {
            Some(libc_ebusy) if libc_ebusy == 16 => CaptureError::Busy,
            _ => CaptureError::Device(e),
        }
    }

    /// Whether the streaming session must be torn down and reopened.
    pub fn is_fatal(&self) -> bool {
        match self {
            // A stall means the deadline was hit inside `v4l`'s own wait, which leaves a
            // buffer queued that the next call would queue again -- see `wait_ready`.
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

/// A streaming session. Holds the device open and the stream running between requests, so
/// that authentication does not pay wiki/06's 200–600 ms cold path.
pub struct Capture {
    // Declaration order is drop order: stop the stream and unmap before releasing the
    // device.
    stream: MmapStream<'static>,
    handle: Arc<Handle>,
    _dev: Device,
    geom: Geometry,
}

impl Capture {
    /// Open the stream and leave it running.
    ///
    /// `warmup` frames are captured and thrown away. The reference machine reads
    /// 22.7, 23.4, 14.3, 31.4 before the AGC settles into a clean alternation (wiki/10
    /// Probe 3), and discarding those here rather than at authentication time is the whole
    /// point of keeping a session warm.
    pub fn start(dev: Device, geom: Geometry, warmup: usize) -> Result<Capture, CaptureError> {
        let handle = dev.handle();
        let mut stream = MmapStream::with_buffers(&dev, Type::VideoCapture, BUFFERS)
            .map_err(CaptureError::from_io)?;
        stream.set_timeout(STREAM_TIMEOUT);

        let mut cap = Capture { stream, handle, _dev: dev, geom };
        // The first `next()` is what queues the buffers and issues STREAMON -- `v4l` starts
        // the stream lazily, so a session that never reads a frame never actually streams.
        for _ in 0..warmup.max(1) {
            cap.take(Instant::now() + STREAM_TIMEOUT)?;
        }
        Ok(cap)
    }

    pub fn geometry(&self) -> Geometry {
        self.geom
    }

    /// Capture `want` usable frames, none of which existed before this call.
    ///
    /// **Fresh frames only (P2-c).** Everything the driver has ready is dropped first, so
    /// every frame returned finished exposing after the flush. Keeping the camera warm and
    /// reusing its pictures are different things: this keeps the first and refuses the
    /// second, so the audit log in Phase 4 can name a moment rather than a window.
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

            // A frame the driver flagged, or one that came up short, is not evidence of
            // anything. Counted so a burst assembled from half-broken frames cannot look
            // like a clean one.
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

    /// Drop every frame the driver already has ready.
    ///
    /// This is what makes the burst fresh. It is also why the flush polls with a zero
    /// timeout rather than reading until a stream timeout: see [`Capture::wait_ready`].
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

    /// Wait until a frame is ready, or the deadline passes.
    ///
    /// **The reason every read is preceded by a poll**, rather than leaning on `v4l`'s own
    /// timeout: `Stream::next` queues the current buffer *before* it polls, and returns
    /// early if the poll times out. The buffer stays queued, and the next `next()` queues
    /// the same index again — which videobuf2 rejects, because a buffer that is not in the
    /// `DEQUEUED` state cannot be queued. One timeout therefore breaks the stream for
    /// good. Polling first means `v4l`'s timeout path is never taken; if the backstop ever
    /// does fire, [`CaptureError::is_fatal`] tears the session down rather than retrying
    /// into that dead end.
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

    /// One frame, copied out of the driver's buffer so it can be requeued immediately
    /// (P2-d). Only called when [`Capture::ready`] has already said a frame is waiting.
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

/// `POLLIN`. Spelled out rather than depending on `libc` directly — this crate's whole
/// dependency list is `v4l`, and one constant is not worth changing that.
const fn libc_pollin() -> i16 {
    0x001
}

/// The format this pipeline requires, for callers that want to name it.
pub const REQUIRED_FORMAT: v4l::FourCC = GREY;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stride_is_the_frame_size_the_corpus_uses() {
        // wiki/10 Probe 2: 640x360 Y800 on the reference machine.
        assert_eq!(Geometry::new(640, 360).stride(), 230_400);
    }

    /// A stall or a driver error means the session is dead and must be rebuilt; being busy
    /// or misconfigured means it never started and there is nothing to tear down.
    #[test]
    fn only_stream_failures_are_session_fatal() {
        assert!(CaptureError::Stalled { got: 0, want: 6 }.is_fatal());
        assert!(CaptureError::Device(io::Error::from(io::ErrorKind::BrokenPipe)).is_fatal());
        assert!(!CaptureError::Busy.is_fatal());
        assert!(!CaptureError::NotCapture.is_fatal());
        assert!(!CaptureError::WrongFormat { got: "YUYV".into() }.is_fatal());
    }

    /// EBUSY has to survive the trip through `io::Error` as `Busy`, or a video call in
    /// progress reads as a broken camera and tears the session down in a loop.
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

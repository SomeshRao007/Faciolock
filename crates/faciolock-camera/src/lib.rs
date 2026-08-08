//! The camera: bytes off a V4L2 device, and nothing else.
//!
//! This crate knows about `/dev/videoN`, `VIDIOC_*` and `mmap`. It knows nothing about
//! liveness, faces, sockets or PAM. It hands up owned pixel buffers with the metadata
//! needed to prove they are the frames the sensor actually produced —
//! [`FrameMeta::sequence`] and [`FrameMeta::timestamp`] — and stops there.
//!
//! ## Copy-out, not zero-copy (P2-d)
//!
//! Frames are memcpy'd out of the mmap'd driver buffer and the buffer is requeued
//! immediately. wiki/05 originally called for zero-copy on memory-budget grounds; that
//! argument is wrong by three orders of magnitude. Six GREY 640×360 frames are 1.4 MB and
//! ~150 µs against a 400 ms latency budget and a 120 MB peak allowance. Holding six driver
//! buffers un-requeued through ~350 ms of inference would starve the queue for a saving
//! that does not register.
//!
//! ## Three defects in `v4l` 0.14, worked around here
//!
//! P2-a chose the crate for correct kernel struct layouts on architectures that cannot be
//! tested here. These are its costs, verified by reading its source rather than its docs.
//! Every one of them is handled at the call site in [`capture`]:
//!
//! 1. **Its poll timeout defaults to infinite.** A stalled camera would hang an
//!    authentication forever. [`Capture`] sets a deadline on every stream and never
//!    leaves it unset.
//! 2. **It retries `EINTR` nowhere.** A signal arriving mid-`ioctl` surfaces as a failed
//!    capture. [`capture`] retries around it.
//! 3. **`Drop for Stream` calls `panic!()`** on any `STREAMOFF` failure other than
//!    `ENODEV`, and a panic in drop during unwind aborts the process. Nothing here panics
//!    on a capture path, so the only route to that abort is a genuine driver failure
//!    during teardown.
//! 4. **One timeout permanently breaks a stream.** `Stream::next` queues the current
//!    buffer before it polls and returns early if the poll expires, so the following call
//!    queues the same buffer index twice and videobuf2 refuses it. Every read here is
//!    preceded by its own `poll`, so `v4l`'s timeout path is never reached —
//!    [`Capture::wait_ready`] has the detail.
//!
//! ## Nothing here writes to the device
//!
//! No UVC extension-unit control is ever set. The reference hardware strobes its emitter
//! on its own (wiki/10 Probe 3), which is what let Q3 delete emitter control from the
//! project entirely along with the USB-bus-hang hazard that comes with brute-forcing UVC
//! controls. The capability probe is therefore a *read* — open, take 30 frames, look at
//! them — and it is composed from [`Capture::burst`] plus
//! `faciolock_vision::frame::classify` at the call site rather than reimplemented here.
//! That keeps the strobe/constant/dark decision in the one crate that has a corpus to
//! justify its thresholds, and keeps this crate's dependency list at exactly `v4l`.

pub mod capture;
pub mod enumerate;

pub use capture::{Capture, CaptureError, CapturedBurst, FrameMeta, Geometry};
pub use enumerate::{Candidate, DiscoveryError, GREY, Node, Selected, discover, open, survey};

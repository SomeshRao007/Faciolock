# 14 — Tuning Constants

Every hardcoded number in the source, its value, and why it is that value.

This page exists because the source no longer carries prose. The reasoning below used to
live in doc comments beside each constant; it was moved here rather than deleted, so
changing one of these numbers still starts by reading what it was chosen against.

The **liveness thresholds** are deliberately not repeated here — they are a live argument,
not settled trivia, and they belong with the measurements that constrain them:
[12](12-falloff-calibration.md) and [13](13-daylight.md). The current defaults are
`dark_ratio_max` 1e-3 enforced, `falloff_cv_min` 0.05 enforced, `response_min` 5.0
advisory, `motion_max` 1.0 advisory.

## Camera

| Constant | Value | Why |
|---|---|---|
| `BUFFERS` | 4 | Enough that the sensor always has somewhere to write while we memcpy the previous frame; few enough that the stale backlog flushed at request time is small. The driver may grant fewer than asked, which is why nothing is indexed by a buffer count. |
| `STREAM_TIMEOUT` | 2000 ms | Backstop only. Every wait is bounded by `Capture::wait_ready` polling with the real deadline. This exists purely so a poll/DQBUF race cannot become an infinite block — `v4l` 0.14 defaults to waiting forever, which in an authentication path would hang `sudo`. |
| `EINTR_RETRIES` | 8 | `EINTR` is not an error, it is a signal arriving at an inconvenient moment. `v4l` 0.14 retries it nowhere. Bounded, because an unbounded retry loop against a genuinely broken device is a hang wearing a different hat. |
| `GREY` / `REQUIRED_FORMAT` | `GREY` (Y800) | 8-bit greyscale, which `ffplay` reports as `Y800`. The only format the reference machine's IR camera offers, and the only one this pipeline can consume. |
| `REFERENCE` (hardware test) | 640×360 | [10](10-hardware-probe.md) Probe 2: `Y800`, 640×360, 30 fps. |

## IPC protocol

| Constant | Value | Why |
|---|---|---|
| `VERSION` | 1 | A peer speaking anything else is refused rather than guessed at. |
| `MAX_USERNAME` | 32 | [04](04-architecture.md)'s cap. `useradd` allows 32 characters, and this is a fixed array, so the cap is a compile-time fact rather than a runtime check that could be forgotten. |
| `HEADER` | 5 | `version \| op \| timeout_ms \| username_len` before the name itself. |
| `REQUEST_MAX` | `HEADER + MAX_USERNAME` | Largest legal request, and also the receive buffer size — a longer datagram is refused without being read into anything. |
| `RESPONSE_LEN` | 4 | Responses are fixed-width. Nothing about them varies with the request. |
| `ENROL_POSES` | 3 | Lives in the shared crate because **both ends count**: `faciolockctl` prompts this many times and the daemon writes the template on the last one. Two constants that must agree are one constant, or the CLI stops prompting early and the daemon waits forever. The response carries no "how many left" field deliberately — `Response` is parsed by the PAM module in a root process, and Phase 5 changes nothing about it. |

## Daemon

| Constant | Value | Why |
|---|---|---|
| `ENROL_EXPIRY` | 60 s | How long a half-finished enrolment survives **without a new pose arriving**. It is a gap, not a total, and the distinction is load-bearing: as a total it silently breaks a slow enrolment — two poses accepted, a minute of fumbling, the third arrives to an expired `Pending`, so the daemon starts over and answers `Ack`, the CLI counts its third `Ack` and reports success, and no template was ever written. The user finds out at a lock screen. See [11](11-bug-log.md). |
| `PEER_IO_TIMEOUT` | 2 s | [04](04-architecture.md): "daemon-side hard timeout regardless of what the client asked for". The request is one datagram already in flight by the time the connection is accepted, so this is not a budget for the peer to spend — it is the bound that stops a peer from *not* spending it. The daemon is serial, so a client that connects and never sends would otherwise park it in `read` forever with the camera held. Only root can reach the socket, so this is robustness rather than access control. |
| `LOCKOUT_AFTER` | 3 | Failures before face authentication is disabled for the user ([03](03-pam-and-security.md) R5). |
| `BACKOFF` | 1 s, 2 s | Applied after failures 1 and 2, indexed by `failures - 1`. At `LOCKOUT_AFTER` the entry is locked and the delay stops being consulted. |
| `LISTEN_FDS_START` | 3 | The first fd systemd hands an activated service. Fixed by the protocol. |
| `MANIFEST` | `MANIFEST.sha256` | Lives beside the models, in `sha256sum` format: `<64 hex>  <filename>`. |
| `REQUIRED` | the 2 files `Pipeline::load` opens | Listed rather than "every `.onnx` in the directory", so the manifest cannot pass by covering files nobody loads while missing one we do. |
| `ERR` | 3 | Syslog priority, as systemd parses it off a stderr line. |

## PAM module

| Constant | Value | Why |
|---|---|---|
| `DEFAULT_SOCKET` | `/run/faciolock.sock` | [04](04-architecture.md)'s data layout. Overridable only from the `pam.d` line. |
| `DEADLINE` | 3000 ms | The worst stall a wedged daemon can add to a lock screen before the password prompt appears (P3-c). The daemon's own hard cap is a 1500 ms capture deadline plus ~350 ms of inference, so 3 s is that worst case with margin. Nothing about it is a target: the warm path measures ~673 ms ([06](06-performance-budget.md)). Shorter cuts off honest slow answers; longer is a user staring at a frozen screen. |
| `REQUEST_TIMEOUT_MS` | 2000 | The timeout the daemon is *asked* for. It applies its own regardless ([04](04-architecture.md)), so this is a hint, set below `DEADLINE` so the daemon gives up first and we hear about it. |
| `CLEAR_DEADLINE` | 300 ms | `Op::Clear` touches no camera and no models — a hash-map removal and a small file rewrite — so it gets a fraction of `DEADLINE`. What matters is the cost when the daemon is *absent*, the common case on a machine where face unlock was never set up: `connect` to a missing socket fails immediately, so a normal password login pays nothing. The pathological case is a daemon that accepted and then wedged, and there the cost is **900 ms, not 300** — `client::ask` bounds connect, write and read *each* by the deadline rather than the exchange as a whole (P3-c). It is added to a login that has already succeeded, so the user is watching a session that should already be open. Under a second is a stall; three seconds, which reusing `DEADLINE` would have cost, is a bug report. |

## Vision — models and geometry

These are not tuning knobs. They are the shapes the pretrained models were built against,
and changing one silently moves every enrolled template into a different space from every
verification.

| Constant | Value | Why |
|---|---|---|
| `CHIP_SIZE` | 112 | Side of the aligned chip. SFace's input, and ArcFace's convention. |
| `REFERENCE_POINTS` | 5 points, 112×112 coords | The canonical ArcFace five-point reference. Right eye, left eye, nose tip, right mouth corner, left mouth corner — the same order YuNet emits, which is the whole reason one model covers detection and alignment both. These exact constants are the de-facto standard the embedder was trained against. |
| `LANDMARKS` | 5 | What YuNet emits, and exactly what the similarity transform in `align` needs — one model, not two. |
| `SFACE_DIMS` | 128 | SFace's embedding width. |
| `STRIDES` | 8, 16, 32 | Feature-map strides YuNet predicts at. |
| `OUTPUT_NAMES` | 12 names | Graph output order, which the decode indexes by position. Verified by name at load time rather than trusted: silently decoding `obj_8` as though it were `cls_8` would still produce boxes — worse ones, for no visible reason. |
| `PAD_VALUE` | 0.0 | Value written into the letterbox margin. Pinned, and covered by the alignment goldens. Padding is not a free choice: it changes what the detector sees at the frame edge, and if it differed between enrolment and verification the two would be detecting on different images. |

## Vision — strobe detection

| Constant | Value | Why |
|---|---|---|
| `STROBE_SNR_MIN` | 10.0 | How far the emitter's contribution must sit above the same-phase noise floor before a capture is called a strobe. **Not a security boundary** — it answers "is this hardware doing what it was enrolled as", not "is this an attack". A floodlight attack is *also* mounted against strobing hardware and reads as `Strobing` here, correctly; rejecting it is liveness' job. See the measurements below. |
| `STROBE_REGULARITY_MIN` | 0.9 | Fraction of steps that must reverse direction. Every capture the project has reads exactly 1.000 — including the daylight ones, which is the measurement that showed the emitter never stopped strobing. Carried at 0.9 to tolerate a dropped frame, matching the pre-[B18](11-bug-log.md) `flips >= 90%` rule it replaces. |
| `GRID` | 6 | Cells per side of the falloff grid. 6×6 gives 36 samples of the face box: enough for the shape of the falloff to show up, coarse enough that each cell still averages over hundreds of pixels rather than sensor noise. On a typical 134×181 detected face that is ~660 px per cell. |

Every strobe SNR the project has measured:

```text
live, nocturnal (5 captures)      931 – 15,461
live, daylight  (1 capture)             549
floodlight attack, nocturnal             53   <- still strobing hardware, correctly
```

10 is ~5× below the weakest thing measured. A continuous source bright enough to drive the
emitter under it fails this and is rejected as `TierMismatch`, which is fail-closed and is
P1-e working as intended.

## Probes and test harnesses

| Constant | Value | Why |
|---|---|---|
| `PROBE_FRAMES` | 30 | Enough to see the AGC settle and still leave ~25 frames of pattern. [10](10-hardware-probe.md) Probe 3 took 60 and the first four were convergence; 30 at the measured 12–15 fps is ~2 s of hardware exposure, the right cost for a command someone runs once. |
| `PROBE_DEADLINE` | 5 s | Generous: not on any latency path, and a stall here would report "no camera" for a camera that was merely slow. |
| `PROBE_WARMUP` / `AGC_SETTLE` / `AGC_SETTLE_FRAMES` | 5 | Frames discarded before the pattern is measured. The reference machine reads 22.7, 23.4, 14.3, 31.4 before the AGC settles into a clean alternation, and measuring those as signal would report a broken strobe ([10](10-hardware-probe.md) Probe 3). |
| `DEADLINE` (CLI client) | 10 s | Generous on purpose. A cold daemon warms the camera on its first request, which measured 339 ms for the warm alone plus the burst behind it. |
| `GIVE_UP` | 3 | Consecutive enrolment failures before the tone changes — advice for a pose the pipeline refused, and the end of the road for one it could not judge. |
| `SEARCH_FRAMES` | 20 | How far into a capture to look for a frame worth detecting on. |
| `ON_OBJECT` | 8.0 | A cell this weak is background, not object, so the box straddles an edge. |
| `W` (synthetic frames) | 64 | Small on purpose: these run in unit tests. |
| `ITERS` (runtime spike) | 50 | Inferences per measurement after warm-up. Enough for a stable p95 without making the spike take minutes on the slower runtime. |
| `ROI_FRACTION` (probe spike) | 0.5 | Centre crop instead of a real face box. At laptop distance the face fills most of the frame, which is enough to compare live-vs-print falloff. |

## Choices the code cannot show

Three decisions that read as arbitrary in the source and are not recorded anywhere else.

**`Limiter::load` clamps a future timestamp to now.** A `last_failure` in the future makes
every backoff comparison succeed forever once the clock is moved back, so it is clamped on
the way in. Setting the clock needs root, which is not the attacker [03](03-pam-and-security.md)
R5 is aimed at — but the clamp is one line and the alternative is reasoning about who can
call `settimeofday`. `Limiter::decide` uses saturating arithmetic for the same reason in the
other direction: a clock that jumped forward reads as "the window has passed" rather than
underflowing into a very long one.

**`Limiter::load` skips a corrupt line rather than failing.** A lost line puts the user at
zero failures, which is where they would be on a machine where the file had never existed.
Refusing to start would take face unlock down for everybody because one line got mangled.
What must not happen is a corrupt line reading as "locked" for someone who is not, and it
cannot: the field either parses or the line is dropped.

**`Peer::of` uses `libc::getsockopt` rather than `UnixStream::peer_cred`.** `std`'s method
is still unstable ([rust-lang#42839](https://github.com/rust-lang/rust/issues/42839)) and
this crate builds on stable. `getsockopt` is what it does anyway, and `serve.rs` already
reaches for `libc` to get `SOCK_SEQPACKET`.

**`Limiter::clear` returns whether anything changed.** `pam_sm_acct_mgmt` sends `Op::Clear`
on every successful login ([03](03-pam-and-security.md) P4-b), and most have nothing to
clear. A journal line per login saying "nothing to do" is noise that hides the ones that
matter.

## Deliberate shortcuts

The `ponytail:` markers left in the source are one line each. Their ceilings and upgrade
paths are here.

| Where | Shortcut | Ceiling / upgrade path |
|---|---|---|
| `daemon/serve.rs` | One connection at a time. | There is one camera; two concurrent authentications would contend for it anyway. Revisit only if a second capture device ever exists. |
| `daemon/serve.rs` | Cancellation checked at stage boundaries. | An already-running burst still finishes before the check lands. Finer granularity needs the capture loop to poll, which is more code for a bounded wait. |
| `daemon/ratelimit.rs` | The whole file is rewritten on each update. | One line per enrolled user on a personal machine, so a few hundred bytes. A keyed database would be more code and one more thing to corrupt. Revisit if a multi-user host appears — not a machine this project targets. |
| `daemon/audit.rs` | Plain text rather than structured `_FIELDS`. | Three characters versus the native journal protocol. Upgrade if anything ever needs to query the audit log by field. |
| `pam/pam.rs` | `libc::syslog` rather than `pam_syslog`. | Same journal entry, without linking a second symbol. |
| `spikes/probe` | Centre crop instead of a face box. | Superseded in Phase 1 by the YuNet detection box: the ROI tracks the face instead of assuming it is centred, and off-centre captures stop being noise. |

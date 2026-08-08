# 04 — Architecture

## Shape

Three components. Copied in spirit from `fprintd` / `pam_fprintd`, which is the
design Debian already ships and trusts.

```
┌──────────────────────────────────────────────────────────────┐
│  sudo / login / sddm / kscreenlocker            [ROOT]       │
│                                                              │
│    dlopen("pam_faciolock.so")                                │
│      ├─ reject if remote session (SSH_*, PAM_RHOST)          │
│      ├─ connect(/run/faciolock.sock)   ← mode 0600, root     │
│      ├─ send  AuthRequest{ username, timeout_ms }            │
│      ├─ poll: socket readable  ‖  PAM conversation cancelled │
│      └─ recv  Verdict{ Ok | Deny | Unavailable }             │
│                                                              │
│    ~400 LOC Rust. No camera. No ML. No config parsing.       │
└───────────────────────────┬──────────────────────────────────┘
                            │ SOCK_SEQPACKET, SO_PEERCRED-checked
┌───────────────────────────┴──────────────────────────────────┐
│  faciolockd                            [user faciolock]      │
│  socket-activated · idle-exits · systemd-hardened            │
│                                                              │
│   ┌────────┐  ┌──────────┐  ┌─────────┐  ┌───────┐  ┌──────┐ │
│   │ V4L2   │→ │ liveness │→ │ detect  │→ │ embed │→ │match │ │
│   │ capture│  │ IR strobe│  │ YuNet   │  │ SFace │  │cosine│ │
│   └────────┘  └──────────┘  └─────────┘  └───────┘  └──────┘ │
│                                                    ↑         │
│   rate limiter · audit log · TPM unseal ───────────┘         │
└───────────────────────────┬──────────────────────────────────┘
                            │
┌───────────────────────────┴──────────────────────────────────┐
│  faciolockctl (CLI)  — enroll, list, remove, test, diagnose  │
│  polkit-gated; talks to the same daemon over the same socket │
└──────────────────────────────────────────────────────────────┘
```

## Why a resident daemon and not a spawned process

Howdy spawns per-authentication. That costs 1–3 s every time. A daemon pays it once.
But the deeper reason is the one `fprintd` gives: **a camera is a singleton device
that multiple things want.** With a daemon you get arbitration, a single place for
rate limiting, a single place for the audit log, and a place to hold a pre-warmed
camera. With a spawn you get none of those and you get them re-implemented badly.

Cost of the daemon: a resident process. Which we avoid — see below.

## Idle cost: how "zero RSS when idle" works

1. `faciolock.socket` is a systemd socket unit. Nothing runs at boot.
2. First `connect()` activates `faciolockd`. It loads models (mmap'd, so pages are
   shared and demand-loaded), opens the camera, answers.
3. After `idle_timeout` (default ~30 s) with no requests, the daemon exits. The
   socket unit stays; the next connect re-activates.
4. **Pre-warm**: a tiny user-session agent subscribes to the compositor's
   idle/lock signals and pokes the socket ~200 ms *before* the lock screen appears,
   so the camera is already streaming when PAM asks. This is where the "instant"
   feel comes from — it hides V4L2 warm-up, which is the real latency, not inference.

The pre-warm agent is a convenience, not a dependency: without it everything still
works, just with the camera warm-up on the critical path.

## IPC: what and why

**Transport: `AF_UNIX` `SOCK_SEQPACKET` at `/run/faciolock.sock`, mode 0600 root:root.**

`SOCK_SEQPACKET` gives message boundaries for free — no length-prefix framing bug to
write, no partial-read handling. Filesystem socket, not abstract namespace, so
permissions actually apply.

Alternative considered: **D-Bus**, which is what `fprintd` uses. Trade-off:

| | Unix socket | D-Bus |
|---|---|---|
| Deps in the root PAM process | libc only | libdbus / zbus + XML introspection |
| Available during early boot / initrd | yes | system bus may not be up |
| Ecosystem integration (KDE settings UI) | need our own | free, standard |
| Activation | systemd socket unit | D-Bus activation |
| Attack surface in root process | minimal | a full IPC library |

The security argument favours the raw socket; the integration argument favours D-Bus.
A defensible hybrid: **PAM module ↔ daemon over the raw socket** (hot path, root,
must be minimal), **CLI/settings-UI ↔ daemon over D-Bus** (cold path, unprivileged,
wants ecosystem integration). Decision deferred to [08](08-open-questions.md).

### Protocol

Fixed-layout binary, hard-capped sizes, no dynamic allocation driven by the peer.

```
Request  (from PAM module, root only)
  u8   version
  u8   op          1=Auth 2=Enroll 3=Status
  u16  timeout_ms
  u8   username_len            ≤ 32
  u8[username_len] username    validated: POSIX portable charset only
```

```
Response
  u8   version
  u8   verdict     0=Deny 1=Allow 2=Unavailable 3=NoEnrolment 4=RateLimited
  u16  elapsed_ms
```

Rules:
- Peer credentials via `SO_PEERCRED` checked **before** reading the body. `uid != 0`
  → close, log, no camera activity.
- One in-flight request per connection. Close = cancel.
- Daemon-side hard timeout regardless of what the client asked for.
- The response carries no token, no nonce, no session — nothing replayable.

## Crate layout

```
faciolock/
├── Cargo.toml                  workspace
├── crates/
│   ├── faciolock-pam/          cdylib  → pam_faciolock.so
│   │   └── deps: libc only. no serde, no tokio, no image, no ort.
│   ├── faciolock-ipc/          shared wire types, no_std-friendly
│   ├── faciolock-daemon/       bin → faciolockd
│   │   └── camera/ liveness/ detect/ embed/ match/ store/ ratelimit/ audit/
│   ├── faciolock-vision/       the pipeline, testable headless from image files
│   ├── faciolock-store/        template store + TPM sealing
│   └── faciolock-cli/          bin → faciolockctl
├── models/                     vendored ONNX + licenses + SHA256SUMS
├── data/
│   ├── faciolock.service  faciolock.socket
│   ├── pam-configs/faciolock
│   └── polkit/…
└── wiki/
```

The hard rule that this layout exists to enforce: **`faciolock-pam` must have a
dependency tree of exactly `libc` and `faciolock-ipc`.** That is auditable in a
`cargo tree` diff on every PR, and it is the thing a distro security reviewer will
check. If a dependency ever appears there, the review is over.

## The `faciolock-vision` boundary

The vision pipeline takes frames in and produces a verdict out, with no knowledge of
V4L2, sockets, or PAM. That means the entire security-relevant logic can be tested
headlessly against a corpus of image files — real faces, printed photos, phone
screens, videos — in CI, with no camera. Without this, the liveness claims in
[05](05-vision-pipeline.md) are unfalsifiable, and unfalsifiable security claims are
worth nothing.

## Data layout

```
/usr/lib/security/pam_faciolock.so       root:root 0644
/usr/libexec/faciolockd                  root:root 0755
/usr/share/faciolock/models/*.onnx       root:root 0644  (+ SHA256SUMS)
/etc/faciolock/config.toml               root:root 0644
/var/lib/faciolock/users/<uid>.tpl       root:root 0600  (TPM-sealed)
/var/lib/faciolock/ratelimit.db          root:root 0600
/run/faciolock.sock                      root:root 0600
```

Keyed by **uid**, not username — usernames get reused, uids are what PAM and
`SO_PEERCRED` actually deal in.

The camera device is recorded in `config.toml` as a **`/dev/v4l/by-path/…` symlink**,
resolved once by `faciolockctl diagnose`. Not `videoN` (renumbers across boots) and
**not `by-id`** — on the reference machine `by-id` collides between the RGB and IR
interfaces of the same USB device and resolves to the wrong camera
([10/Probe 4](10-hardware-probe.md#probe-4--device-node-disambiguation)). The daemon
re-verifies the format and geometry on open regardless of what the config says.

## Failure behaviour

| Failure | Result |
|---|---|
| Daemon not installed / socket missing | `connect()` fails → `PAM_AUTHINFO_UNAVAIL` → password prompt, instantly |
| Daemon hangs | client-side deadline → `PAM_AUTHINFO_UNAVAIL` |
| Daemon crashes mid-request | socket EOF → `PAM_AUTHINFO_UNAVAIL` |
| Camera busy (video call) | daemon replies `Unavailable` fast, doesn't fight for the device |
| **Room too bright in NIR** | gate fires before any biometric work → `Environment` → `Unavailable`. **Not counted as a failed attempt** ([B18](11-bug-log.md#b18)) |
| No enrolment | `NoEnrolment` → `PAM_USER_UNKNOWN`, no camera touched |
| PAM module panics | `catch_unwind` → `PAM_AUTH_ERR` |

Every row ends in the user typing their password. Nothing here can lock anyone out.

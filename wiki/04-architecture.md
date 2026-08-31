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
│  polkit-gated, runs as root. enroll/test/bench go over the   │
│  socket; list/remove read the template directory (P5-b)      │
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
4. ~~**Pre-warm**: a tiny user-session agent subscribes to the compositor's
   idle/lock signals and pokes the socket ~200 ms *before* the lock screen appears.~~
   **Deferred, and the design above does not work.** See below.

Steps 1–3 are built (Phase 6). [`data/systemd/faciolock.socket`](../data/systemd/faciolock.socket)
is the socket unit; `serve::inherited` is `sd_listen_fds` hand-rolled against the three
environment variables and a `getsockopt`, for the same reason [`audit`](03-pam-and-security.md#r9--audit-every-attempt)
writes its own priority prefixes — no `libsystemd` in the process that parses V4L2 buffers.
`idle_timeout_secs` (default 30) is honoured **only when the socket was inherited**: a
self-bound daemon that exits on a timer has nothing to reactivate it, which is not an
idle-exit but a crash on a schedule.

Two properties that had to be built rather than configured:

- **The daemon must not unlink the socket path when the fd came from systemd.** systemd keeps
  its own fd bound to that inode across every activation. Unlinking on exit would mean the
  *first* idle-exit permanently broke reactivation — a daemon that works until it works.
- **An enrolment in flight blocks the exit.** `ENROL_EXPIRY` is 60 s against a default idle
  of 30, so somebody fetching their glasses between poses is the ordinary case. A `Pending`
  lives in the process's memory and nowhere else.

### The pre-warm agent does not work as specified, and it is deferred (P6-a)

Two problems, and the second is fatal to the design rather than to the implementation.

**It cannot reach the socket.** The socket is 0600 root:root by
[R2](03-pam-and-security.md#r2--the-daemon-decides-who-you-are-the-pam-module-states-who-you-claim-to-be).
A user-session agent's `connect()` returns `EACCES` before the kernel queues anything, so
nothing activates and nothing warms. That much is fixable — a second, laxer socket that the
daemon accepts and closes — at the price of letting anyone in group `video` start the daemon
and light the IR emitter at will.

**It pokes at the wrong end of the absence.** The plan warmed on the *lock* signal. You lock
the screen when you leave; the daemon idle-exits 30 s later; you return five minutes after
that. The warm expired before it was needed. The moment worth warming is "a person just came
back" — and on KDE the greeter takes the input that signals it and then calls PAM itself,
with nothing observable in between. There is no signal a session agent can subscribe to that
arrives early enough to help.

So what the agent was meant to hide stays visible, and it is worth naming:

| Cold unlock, socket-activated | Cost |
|---|---|
| exec → socket answering (model load + SHA256 of 39 MB) | 377 ms |
| camera open + AGC settle frames | ~400 ms |
| the 6-frame burst | ~340–410 ms |
| detect + embed + match | ~350 ms |
| | **~1.5 s**, against the PAM module's 3 s deadline |

Warm, only the last two rows are paid: **~750 ms**. `idle_timeout_secs` is therefore the real
lever and the only honest one — it trades that ~800 ms against a resident process holding a
camera. It is config, and the number is an operator's to choose.

**The daemon-side half of the pre-warm went the same way, and it took a bug to notice
(P6-e).** `serve()` used to warm the camera in a thread as soon as it had a listener — the
agent's job done from the inside, on "somebody connected" instead of "a person came back".
Once the PAM profile was enabled those two events diverged completely: cron's account stack
connects several times an hour, so the emitter came on for a full idle window with nobody in
front of it ([B29](11-bug-log.md#b29)). It could not have paid for itself in the meantime —
PAM runs the auth stack before the account stack, so a `Clear`-triggered warm never precedes
an `Auth`, and inside one login the arriving request just queues behind the warm's burst on
the session mutex. The camera now opens for `Op::Auth` and `Op::Enroll` and for nothing else,
and the table above is what the first one of those pays.

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
wants ecosystem integration). ~~Decision deferred to [08](08-open-questions.md).~~

**Decided 2026-08-23 against the hybrid** ([08 Q4](08-open-questions.md#q4--ipc-raw-unix-socket-d-bus-or-both--decided-2026-08-23-raw-socket-only),
P5-a). One surface: `faciolockctl` speaks this socket, as root, gated by polkit at the
`pkexec` boundary. The hybrid's payoff was richer enrolment feedback, and the existing
verdicts already separate the two things a person can act on — `Deny` means it looked and
refused, `Unavailable` means it could not judge. That is "move closer" versus "close the
blinds" with no new protocol, and it is not worth `zbus` in the process that parses V4L2
buffers.

### Protocol

Fixed-layout binary, hard-capped sizes, no dynamic allocation driven by the peer.

```
Request  (from PAM module, root only)
  u8   version
  u8   op          1=Auth 2=Enroll 3=Status 4=Clear
  u16  timeout_ms
  u8   username_len            ≤ 32
  u8[username_len] username    validated: POSIX portable charset only
```

```
Response
  u8   version
  u8   verdict     0=Deny 1=Allow 2=Unavailable 3=NoEnrolment 4=RateLimited 5=Ack
  u16  elapsed_ms
```

`Clear`/`Ack` were added in Phase 4 (P4-b) and are the lockout release: `pam_sm_acct_mgmt`
sends the op after the PAM stack has succeeded, the daemon forgets that user's failures, and
`Ack` is the acknowledgement. It authorises nothing — `Ack` is enumerated in the same
exhaustive `is_allow` match as everything else precisely so it cannot become an allow by
being forgotten.

**`Enroll` became a real operation in Phase 5, with no change to these bytes.** It was
refused outright until then because an unauthenticated enrolment endpoint is a store write
available to anyone who can reach the socket; the two preconditions it was waiting on —
`SO_PEERCRED` and polkit — now both exist. One request captures **one pose**; the daemon
accumulates and writes the template on the last one. `faciolock_ipc::ENROL_POSES` is how many,
and it lives in the shared crate because both ends count: the CLI prompts that many times and
the daemon finalises on that number, so two constants that must agree are one constant.

The reply reuses the verdicts already defined: `Ack` for a captured pose, `Deny` for one the
pipeline looked at and refused, `Unavailable` for one it could not judge. **There is no
"poses remaining" field and there must not be** — `Response` is parsed by a module loaded
into `sudo` and `login`, and enrolment is not a good enough reason to touch that parser.

**The failure-budget policy is deliberately not on the wire.** What crosses is a *category*;
what to do about it is decided in the daemon. [B20](11-bug-log.md#b20) reopened the
`Environment` exemption once already, and it has to stay re-decidable without a protocol
break ([13](13-daylight.md)).

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
│   │   └── config/ models/ serve/ session/ store/ ratelimit/ audit/ users/
│   ├── faciolock-vision/       the pipeline, testable headless from image files
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

**`faciolock-store` was planned and is not being built** (decision P4-d). The store is
`faciolock-daemon/src/store.rs`, because it has exactly one consumer. A crate with one
consumer is a boundary that has to be maintained and buys nothing. The rule above is
unaffected — it is about what `faciolock-pam` depends on, and the store is not in that tree
either way.

**Phase 5 corrected the second half of that sentence.** P4-d used to add "`faciolockctl`
reaches the store through the socket like everything else, not by linking it", and the first
clause did not survive contact with `list`. A list of enrolled uids is a variable-length
answer and `Response` is four fixed bytes parsed inside a root PAM process; carrying it would
mean growing that parser for the least security-sensitive feature in the project.

So the CLI **reads the template directory and never its contents** (P5-b). A filename is a
uid, a mtime is a date, `remove` is an unlink — and reaching any of it requires being root,
which is a strictly larger capability than the unlink. Nothing links `store.rs`, decrypts
anything or reads the key, so the property P4-d actually protects is intact: the store has
one consumer and the format has one implementation.

`chacha20poly1305` is the only dependency Phase 4 added, daemon-only, and it is packaged in
the archive at the version pinned here.

## The `faciolock-vision` boundary

The vision pipeline takes frames in and produces a verdict out, with no knowledge of
V4L2, sockets, or PAM. That means the entire security-relevant logic can be tested
headlessly against a corpus of image files — real faces, printed photos, phone
screens, videos — in CI, with no camera. Without this, the liveness claims in
[05](05-vision-pipeline.md) are unfalsifiable, and unfalsifiable security claims are
worth nothing.

## Data layout

```
/usr/lib/security/pam_faciolock.so           root:root            0644
/usr/libexec/faciolockd                      root:root            0755
/usr/bin/faciolockctl                        root:root            0755  (P5-a; polkit-gated)
/usr/share/polkit-1/actions/
                 org.faciolock.policy        root:root            0644
/usr/share/faciolock/models/*.onnx           root:root            0644  (+ SHA256SUMS)
/etc/faciolock/faciolockd.toml               root:root            0644
/var/lib/faciolock/                          faciolock:faciolock  0700
/var/lib/faciolock/templates/<uid>.tpl       faciolock:faciolock  0600  (encrypted, R6)
/var/lib/faciolock/templates/key             faciolock:faciolock  0600  (P4-a)
/var/lib/faciolock/ratelimit.db              faciolock:faciolock  0600
/run/faciolock.sock                          root:root            0600
```

Keyed by **uid**, not username — usernames get reused, uids are what PAM and
`SO_PEERCRED` actually deal in. The translation happens once, in the daemon's `users.rs`;
nothing below that line sees a name.

**The ownership above is a Phase 4 correction.** This table used to say `root:root 0600` for
the templates while [03 R7](03-pam-and-security.md#r7--sandbox-the-daemon-aggressively) runs
the daemon as `User=faciolock`. Both cannot be true: the daemon must read templates to do its
job, and it is not root. The files belong to the daemon's own user, and the honest
consequence is that **anything running as `faciolock` can read the key**. That is one process
today, and Phase 6's sandbox is what keeps it one.

The key sits in the same directory as the templates deliberately. Anything that can read one
can read the other, so separating them would look like a boundary without being one.

The socket stays `root:root` because it is created by systemd (Phase 6) or by a daemon
started as root that then drops privileges — not by the daemon's own user.

The camera device is recorded in `config.toml` as a **`/dev/v4l/by-path/…` symlink**,
resolved once by `faciolockctl diagnose`. Not `videoN` (renumbers across boots) and
**not `by-id`** — on the reference machine `by-id` collides between the RGB and IR
interfaces of the same USB device and resolves to the wrong camera
([10/Probe 4](10-hardware-probe.md#probe-4--device-node-disambiguation)). The daemon
re-verifies the format and geometry on open regardless of what the config says.

**The capability tier is written next to it, by the same command** (Phase 5). It is the one
security-relevant value in that file, and it is in a file rather than inferred because P1-e
says it must be: inferring the tier from the burst being authenticated would let an attacker
with a bright enough continuous IR source wash out the ON/OFF separation and *demote* the
machine to a tier whose checks they can pass.

`diagnose` therefore only ever asserts a tier **upward**. It reports `ir-strobe` when it
observes a clean strobe and writes nothing at all otherwise — because the measurement that
says "no strobe here" and the measurement that says "somebody is pointing a floodlight at
this" are the same measurement, and [B18](11-bug-log.md#b18) showed ordinary daylight
produces it. A machine with a genuinely constant emitter needs a human to write
`tier = "ir-constant"` by hand. `--write` also refuses to overwrite an existing config, so a
diagnostic run in a bright room cannot rewrite a working machine.

## Failure behaviour

| Failure | Result |
|---|---|
| Daemon not installed / socket missing | `connect()` fails → `PAM_AUTHINFO_UNAVAIL` → password prompt, instantly |
| Daemon hangs | client-side deadline → `PAM_AUTHINFO_UNAVAIL` |
| Daemon crashes mid-request | socket EOF → `PAM_AUTHINFO_UNAVAIL` |
| Camera busy (video call) | daemon replies `Unavailable` fast, doesn't fight for the device |
| **Room too bright in NIR** | gate fires before any biometric work → `Environment` → `Unavailable`. **Not counted as a failed attempt** ([B18](11-bug-log.md#b18)) |
| No enrolment | `NoEnrolment` → `PAM_USER_UNKNOWN`, no camera touched |
| **Rate-limited or locked out** | `RateLimited` → `PAM_AUTH_ERR`, answered before the store lookup and before the camera. Same code as `Deny`, deliberately: telling a caller they are locked out tells an attacker their budget |
| **Template exists and does not decrypt** | `Unavailable` → password prompt, **and an error in the journal**. Never "not enrolled" — a damaged or implanted store must not be silent |
| PAM module panics | `catch_unwind` → `PAM_AUTH_ERR` |
| **Anything at all, in `pam_sm_acct_mgmt`** | `PAM_IGNORE`. The account stack is the one place a failure would refuse the login rather than fall through, so that function has no path that can return anything else |

Every row ends in the user typing their password. Nothing here can lock anyone out.

# 03 — PAM & Security Model

## Part 1: The PAM contract

### What a module is

A PAM module is a shared object (`pam_faciolock.so`) that the application `dlopen`s.
It exports C-ABI symbols; for authentication only two matter:

```c
int pam_sm_authenticate(pam_handle_t *pamh, int flags, int argc, const char **argv);
int pam_sm_setcred(pam_handle_t *pamh, int flags, int argc, const char **argv);
```

`pam_sm_setcred` must exist and can just `return PAM_SUCCESS;` — PAM will error if
it's missing. In Rust this is a `crate-type = ["cdylib"]` with `#[no_mangle] pub
extern "C"` functions.

Reference: Linux-PAM Module Writers' Guide.

### The execution context you are in

This is the part that drives the whole design:

- **You are usually root.** `sudo`, `su`, `login`, `sddm`, and `kscreenlocker`'s
  helper all run privileged when the auth stack executes. Every byte of code you
  pull into that process is root-privileged code.
- **You are inside someone else's process.** You do not control its signal handlers,
  its threads, its `LD_PRELOAD`, its file descriptors, or its event loop.
- **You may be called concurrently.** KDE deliberately runs two PAM stacks at once
  (password + biometric). Your module must be reentrant and cancellable.
- **You must not block forever.** A hung PAM module means an unusable lock screen.
- **`PAM_AUTHTOK` is cleared before returning to the application.** You cannot leak a
  password out through it, and you should not try to put one in.

### Return codes — the fail-closed rule

| Situation | Return |
|---|---|
| Face matched, liveness passed | `PAM_SUCCESS` |
| Face seen but not a match | `PAM_AUTH_ERR` |
| Timeout / no face found | `PAM_AUTH_ERR` |
| No camera, daemon down, socket refused | `PAM_AUTHINFO_UNAVAIL` |
| User has no enrolment | `PAM_USER_UNKNOWN` |
| Anything unexpected, any internal error | `PAM_AUTH_ERR` |

**There is exactly one code path that returns `PAM_SUCCESS`**, and it is reached only
after a positive match *and* a positive liveness verdict. Everything else — including
every `Err` branch, every parse failure, every panic — collapses to a failure code.

In Rust this means:
- the entry point is a thin wrapper: `catch_unwind(|| real_auth(...)).unwrap_or(PAM_AUTH_ERR)`
- `panic = "abort"` is **wrong** here; unwinding across the FFI boundary is UB, so we
  catch at the boundary and never let a panic reach the caller.
- no `?` propagating to a default-success value; the success value is constructed in
  one place only.

**Built in Phase 3** as `crates/faciolock-pam`. The single construction site is `code_for`,
and two tests hold it: one asserts that no verdict but `Allow` produces `PAM_SUCCESS`, the
other that `code_for` and `faciolock_ipc::Verdict::is_allow` — the daemon's own notion of
success — cannot drift apart. `RateLimited` deliberately returns the same code as `Deny`:
distinguishing them tells an attacker whether to keep going.

The `panic = "abort"` prohibition has a second consequence now that the module is built and
weighed. It is also the only lever that would bring the binary under
[06](06-performance-budget.md)'s size line, and it stays off the table: the resident
footprint is inside budget anyway, and trading the fail-closed guarantee for disk space
would be trading the property this section is about.

### Stack placement

We ship a `/usr/share/pam-configs/faciolock` profile so `pam-auth-update` installs it
correctly. Placement is `auth sufficient` **before** `pam_unix.so`, with a `Priority:`
that puts it above unix. `sufficient` means: success ends the stack happily, failure
falls through to the password. It must never be `required` or `requisite` — that
would let a broken camera lock the user out of their machine.

## Part 2: Threat model

### Assets

| Asset | Why it matters |
|---|---|
| The authentication verdict | Grants root via `sudo`, unlocks the session |
| Enrolled face templates | Biometrics are not revocable. You get one face. |
| The camera stream | A privacy asset independent of auth |
| The daemon's address space | It parses attacker-influenced pixels |

### Adversaries

**A1 — Physical presenter.** Has a photo of you (social media), a phone/laptop screen,
a printed high-res IR-ish image, or a video. Has physical access to the locked
machine. *This is the realistic attacker and the one liveness must stop.*

**A2 — Local unprivileged user / malware in your session.** Wants to escalate to
root. Will try: writing to the template store, replacing a model file, feeding the
daemon a hostile frame via a fake V4L2 device, racing the socket, connecting to the
daemon and claiming to be another user, or replaying a previous success.

**A3 — Offline disk attacker.** Has the laptop, pulls the SSD. Wants templates
(biometric theft) or wants to plant a template that matches their own face.

**A4 — Remote.** `sudo` over SSH. Must be refused unconditionally — there is no
camera at the other end and a face check makes no sense.

**A5 — The lookalike / twin.** Not solvable. Documented, not engineered around.

### Attack surface inventory

| Surface | Exposed to | Mitigation |
|---|---|---|
| PAM module code | A2 (loaded in root proc) | Rust, ~400 LOC, no parsing, no ML, no camera |
| Unix socket | A2 | `SO_PEERCRED`, root-only mode 0600, abstract-namespace forbidden |
| IPC message parsing | A2 | fixed-size binary framing, hard length caps, no serde on untrusted input beyond a size-bounded frame |
| V4L2 frame data | A1, A2 | parsed only in the unprivileged sandboxed daemon |
| ONNX model files | A2, A3 | root-owned, 0644, hash-verified at load |
| Template store | A2, A3 | daemon-owned dir 0700, AEAD-encrypted contents bound to the uid ([R6](#r6--templates-are-encrypted-the-key-is-a-file-not-the-tpm-v1)) |
| Config file | A2 | root-owned, read by daemon only, never by the PAM module |

## Part 3: Design rules that fall out of this

### R1 — Privilege separation is the whole security story

The PAM module runs as root but does almost nothing. The daemon does everything but
runs as a dedicated unprivileged user with camera group access. Neither half alone is
both powerful and complex. Details in [04](04-architecture.md).

### R2 — The daemon decides *who* you are, the PAM module states *who you claim to be*

The PAM module sends: "the user being authenticated is `csr`, and here is my
`SO_PEERCRED` proving I am uid 0". The daemon never trusts a username from a
non-root peer. An unprivileged process connecting to the socket gets refused before
the camera is even touched — this is what stops A2 from asking the daemon to
authenticate root and reading back a yes.

Concretely: the socket is mode `0600` owned by root, so only root can connect at all;
`SO_PEERCRED` is a second, in-band check so a future permission mistake is not
instantly fatal.

**Built in Phase 3, and one detail is worth stating because it is not the obvious
choice: a refused peer gets no reply at all.** `serve::handle` reads the peer credentials
before the request body, and a `uid != 0` connection is closed having caused no decode, no
template lookup and no camera activity. Answering — even `Deny` — would make the socket an
oracle: an unprivileged process could ask "is `root` enrolled?" and read the difference
between `Deny` and `NoEnrolment`. Closing says nothing, and wiki/04 already defines socket
EOF on the caller's side as `PAM_AUTHINFO_UNAVAIL`, so the honest client's behaviour is
unchanged.

The mode and the credential check are genuinely independent layers, which
[B23](11-bug-log.md#b23) is a small demonstration of: the mechanism that had been buying the
0600 turned out to have a scope nobody chose, and the credential check is what meant that
was a tidiness problem rather than a security one.

### R3 — Verdicts are not transferable

The reply is a single byte on the same connection, with no token, no session, no
cache. There is nothing to steal and replay. A success is bound to the connection
that asked for it and dies with it.

### R4 — Reject remote sessions in the PAM module, before any IPC

Check `PAM_RHOST`, `SSH_CONNECTION`, `SSH_CLIENT`, `SSH_TTY`. If any indicates a
remote session, return `PAM_AUTHINFO_UNAVAIL` immediately. Cheap and closes A4.

### R5 — Rate limit in the daemon, per user, persistently

Exponential backoff on consecutive failures, persisted across daemon restarts so
A1 can't reset it by killing the service. After N failures, face auth is disabled
for that user until a successful password authentication. This is the piece Howdy
has none of, and it is what turns "a photo might work" from a 200-attempt certainty
into a 3-attempt gamble.

**Built in Phase 4** as `crates/faciolock-daemon/src/ratelimit.rs`. N is **3** — the number
this paragraph already implied, taken from here rather than invented. One failure costs 1 s,
two costs 2 s, the third disables face unlock for that user until something clears it.

**This stopped being a backstop.** The paragraph above is written as though liveness were
the primary control and this the safety net. [B24](11-bug-log.md#b24) reversed that: a flat
printed face clears every enforced liveness check and is detected by YuNet while doing it.
Until a check exists that rejects a print, **the failure budget is the only thing between a
photograph and unlimited retries.** Nothing about the design changed; what changed is that it
is now load-bearing, and it is worth knowing that when reading it.

Three properties that took deciding, and are asserted rather than described:

- **The budget is checked before the template load and before the camera.** A locked-out user
  costs one hash lookup, lights no emitter, and learns nothing about whether they are
  enrolled. `RateLimited` is not itself an attempt and does not extend the backoff — else a
  caller retrying in a loop could hold themselves locked out without ever showing a face.
- **Only verdicts that looked at a face count.** `Environment`, `Unusable` and `Failed` are
  exempt (P2-g): the weather must not be able to disable face unlock. A cancelled request
  never counts either — a user who walks away from a lock screen has not failed anything.
- **The failure is written before the reply is sent.** A `kill -9` between the verdict and
  the answer must not roll the counter back, or the budget is advisory.

The numbers are hardcoded and there is no configuration key for them (P4-c). A threshold
reachable from an editable file is a threshold an attacker with one write sets to zero, and
the file would be read by the process that grants access.

#### How the lockout is released

`pam_sm_acct_mgmt` sends `Op::Clear` (decision P4-b). PAM runs the account stack after
authentication has succeeded, so reaching that function *is* the signal — by password, by
face, by whatever the stack accepted. `pam_sm_setcred` was the obvious candidate and lost on
coverage: `sudo`, `su` and `login` call `pam_setcred`, but screen lockers are inconsistent
about it, and the lock screen is precisely where a locked-out user is standing.

**That module returns `PAM_IGNORE` on every path, including panics.** A failure returned from
the *auth* stack falls through to a password prompt. A failure returned from the **account**
stack refuses the login outright. Faciolock has no opinion about whether an account is valid
and must never be able to express one, so there is no code path in it that can say anything
else.

`Op::Clear` grants nothing and is not an authenticator. The peer is already root by the time
it runs — `SO_PEERCRED` is checked before the request body is read — and a root peer that
wanted to lie about it could delete the database instead.

### R6 — Templates are encrypted; the key is a file, not the TPM (v1)

**What was designed:** embeddings encrypted with a key sealed to TPM 2.0 against a PCR
policy, so the blob is worthless on any other machine and worthless on this one if the boot
chain changed.

**What v1 ships:** the encryption, with the key in a root-only file. [Q5 decided this on
2026-08-23](08-open-questions.md#q5--tpm-sealing-v1-or-later--decided-2026-08-23-later)
and P4-a records it. The reduced guarantee, stated plainly rather than implied:

| Attack | v1 (key file) | TPM, no PCR | TPM + PCR 7 |
|---|---|---|---|
| Copies `<uid>.tpl` off the disk | **stopped** | stopped | stopped |
| Writes a template to implant their own face | **stopped** | stopped | stopped |
| Moves a template between users | **stopped** | stopped | stopped |
| Images the whole disk | **not stopped** | stopped | stopped |
| Boots a live USB on this laptop | **not stopped** | not stopped | stopped |
| Has root on the running machine | not stopped | not stopped | not stopped |

The last row is true of every column and is worth stating: root can read the decrypted
template out of the daemon's address space regardless. Everything here is about the **disk**,
not about a live compromise.

**Built in Phase 4** as `store.rs`. XChaCha20-Poly1305 over the Phase 2 encoding, with the
uid and the key-provider discriminant as associated data — so a template that is renamed to
another uid, or edited by one bit, is a decryption failure rather than a different face. A
file that exists and does not decrypt is an **error**, never "not enrolled": the latter would
downgrade a damaged or tampered store to a silent password prompt.

The key lives in the same directory as the templates on purpose. Anything that can read one
can read the other, and separating them would be theatre.

### R7 — Sandbox the daemon aggressively

The daemon is the process that touches attacker-controlled pixels. systemd unit
hardening, at minimum:

```ini
User=faciolock
NoNewPrivileges=yes
PrivateNetwork=yes
PrivateTmp=yes
ProtectSystem=strict
ProtectHome=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictNamespaces=yes
RestrictRealtime=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
SystemCallFilter=@system-service
SystemCallArchitectures=native
DeviceAllow=/dev/video* rw
ReadWritePaths=/var/lib/faciolock
```

Target `systemd-analyze security faciolock.service` ≤ 2.0. Note
`MemoryDenyWriteExecute=yes` may conflict with a JIT-ing inference runtime — one more
input to the runtime choice in [08](08-open-questions.md).

### As shipped — measured 2026-08-29

[`data/systemd/faciolock.service`](../data/systemd/faciolock.service).
**`systemd-analyze security` reads 0.6, SAFE**, against a target of 2.0. Measured offline
(`--offline=true`) against the unit file, so the number is reproducible without installing
anything. `MemoryDenyWriteExecute=yes` turned out to cost nothing: `tract` is a pure-Rust
interpreter with no JIT and the models are mmap'd read-only, so [Q2](08-open-questions.md#q2--inference-runtime--decided-tract)
settled this line as a side effect of settling the runtime.

Everything the sketch above listed is in the unit, plus `ProtectKernelLogs`, `ProtectClock`,
`ProtectHostname`, `ProtectProc=invisible`, `ProcSubset=pid`, `RestrictSUIDSGID`, `RemoveIPC`,
an empty `CapabilityBoundingSet`, `RestrictAddressFamilies=AF_UNIX`, `IPAddressDeny=any` and
`SystemCallFilter=~@resources @privileged` on top of `@system-service`.

Four corrections to the sketch, each of which would have scored better than it deserved:

| Sketch | Shipped | Why |
|---|---|---|
| `DeviceAllow=/dev/video* rw` | `DeviceAllow=char-video4linux rw` | The glob is a *path*, and the numbering renumbers across boots ([10 Probe 4](10-hardware-probe.md#probe-4--device-node-disambiguation)). The device class does not. |
| `ReadWritePaths=/var/lib/faciolock` | `StateDirectory=faciolock` | Creates the directory 0700 owned by the daemon's user on first start, rather than requiring something else to have made it first. |
| (absent) | `SupplementaryGroups=video` | `/dev/video2` is `root:video 0660`. This is the daemon's one privilege over a plain unprivileged process, and without it `User=faciolock` cannot open the camera at all. |
| (implied) | **no `PrivateDevices`, no `PrivateUsers`** | `PrivateDevices=yes` gives a private `/dev` with no video nodes in it. `PrivateUsers=yes` maps every group but the service's own to `nobody`, which takes `video` with it. Both score well and neither runs. |

The three findings `systemd-analyze` still reports are the three that cannot be fixed without
breaking the daemon — `SupplementaryGroups`, `PrivateUsers`, `DeviceAllow` — plus
`RootDirectory=`, which would be a chroot for a service that already cannot write outside its
state directory.

This is also what makes wiki/04's honest note about the template key bearable. Anything
running as `faciolock` can read the key; the sandbox is what keeps that "one process" rather
than "any process that manages to be that user".

### R8 — No side effects outside the auth decision

No `/dev/uinput`. No writing to the user's home. No network. No notifications from
the PAM module. The module's only observable effect is its return code.

### R9 — Audit everything

Every attempt logged to the journal via the daemon: timestamp, username, verdict,
reason, liveness score, match distance, and the PID/UID of the requesting peer.
Successes and failures both. This is table stakes for a security component and it is
what a distro security team will look for first.

**Built in Phase 4** as `audit.rs`, one `key=value` line per attempt from a single
construction site — the same discipline as the single `PAM_SUCCESS` site and the single
`Allow` site, applied to the log, so an attempt cannot return without a record.

```text
<4>faciolockd: auth uid=1000 verdict=Deny reason=no-match best=0.312 live=~Falloff:0.0400
    probes=3 pid 27608 uid 0 elapsed=673 counted=1
```

**Username became uid.** wiki/04 keys everything persistent by uid, and the log follows;
`users.rs` is the one place a name is translated, and nothing below it sees one.

The leading `<N>` is the syslog priority, which systemd parses off a service's stderr — so
`journalctl -p` works for the cost of three characters rather than a `libsystemd` dependency
in the process that parses attacker-controlled pixels. `<5>` allow, `<4>` deny, `<3>` lockout
or pipeline fault, `<6>` a question the daemon declined to answer. A bright room logged at
warning would train an operator to ignore the level.

Two things the line carries that the paragraph above did not ask for, and should have:

- **`counted=`** — whether this attempt moved the failure budget. The exemptions in R5 are a
  policy that can be argued with, and an audit log that hid which way it went would make that
  argument unresolvable.
- **Advisory liveness failures**, marked `~`. These are the checks waiting on a spoof
  recording to calibrate against; the journal is where the evidence for calibrating them
  accumulates. An advisory failure rejected nothing, and the mark says so.

What is logged is the **detailed** verdict — which check, at what value. What crosses the
socket stays narrow, because telling a caller which check rejected them is telling an
attacker what to fix.

## Part 4: Explicit limitations (to be in the README verbatim)

- A sufficiently determined attacker with a 3D-printed mask and knowledge of the
  system can defeat this. So can an identical twin.
- On hardware without an IR camera, RGB-only mode is materially weaker; liveness
  degrades to heuristics.
- This is a convenience factor. Do not use it as the sole authentication method, and
  do not use it to protect anything you would not protect with a 4-digit PIN.

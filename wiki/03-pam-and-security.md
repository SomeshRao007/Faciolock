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
| Template store | A2, A3 | root-owned dir, TPM-sealed contents |
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

### R6 — Templates sealed to the TPM

Embeddings are encrypted with a key sealed to TPM 2.0 against a PCR policy. The
sealed blob on disk is worthless on any other machine, and worthless on this machine
if the boot chain changed. This directly addresses A3 both ways: no biometric theft,
and no offline template implantation (an attacker can write a file, but cannot
produce one the daemon will unseal).

Open question: PCR selection is a real usability trade-off — bind too tightly and a
kernel update invalidates enrolment. See [08](08-open-questions.md). Fallback when no
TPM is present: a key in a root-only file, with the reduced guarantee documented.

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

### R8 — No side effects outside the auth decision

No `/dev/uinput`. No writing to the user's home. No network. No notifications from
the PAM module. The module's only observable effect is its return code.

### R9 — Audit everything

Every attempt logged to the journal via the daemon: timestamp, username, verdict,
reason, liveness score, match distance, and the PID/UID of the requesting peer.
Successes and failures both. This is table stakes for a security component and it is
what a distro security team will look for first.

## Part 4: Explicit limitations (to be in the README verbatim)

- A sufficiently determined attacker with a 3D-printed mask and knowledge of the
  system can defeat this. So can an identical twin.
- On hardware without an IR camera, RGB-only mode is materially weaker; liveness
  degrades to heuristics.
- This is a convenience factor. Do not use it as the sole authentication method, and
  do not use it to protect anything you would not protect with a 4-digit PIN.

# 01 — Goals & Scope

## Problem

Kubuntu 26 ships no IR face authentication. `fprintd` covers fingerprints and is a
first-class citizen (D-Bus daemon, PAM module, KDE settings integration). The
equivalent for the IR camera that ships on most business laptops does not exist in
the archive. The only real option is Howdy, which is a third-party PPA/AUR install,
Python-based, and carries an explicit "do not use as sole authentication" warning
from its own author.

## Goals

| # | Goal | Measurable as |
|---|---|---|
| G1 | Works everywhere a password works | login, SDDM greeter, kscreenlocker, `sudo`, `su`, polkit |
| G2 | Fast | p95 auth ≤ 400 ms wall-clock from PAM entry to verdict |
| G3 | Light | 0 bytes RSS when idle; < 120 MB peak during a match |
| G4 | Accurate | FRR < 3% at FAR < 1e-4 on enrolled user, IR mode |
| G5 | Secure | Photo/screen replay defeated by design, not by a heuristic |
| G6 | Feels built-in | systemd unit, KDE settings module, `pam-auth-update` profile |
| G7 | Shippable by Debian/Ubuntu | 100% DFSG-compatible licensing, models included |

G7 constrains G4 hard — see [05](05-vision-pipeline.md). The best face models
(InsightFace `buffalo_l`, ArcFace R100) are **research/non-commercial only** and
cannot go in the archive.

## Non-goals

- **Not a password replacement.** It is a convenience factor. Password must always
  remain reachable, and we will say so in the README as loudly as Howdy does.
- **Not multi-user identification.** We verify a claimed identity (1:1), we do not
  search a gallery (1:N). 1:1 is faster and much safer.
- **Not RGB-first.** RGB-only is a degraded mode we support, not the design target.
  Liveness on RGB alone is a research problem; on IR it is mostly physics.
- **No cloud, no network.** The daemon runs with `PrivateNetwork=yes`.
- **No GPU requirement.** CPU-only must hit the budget. GPU/NPU is optional later.

## Why Rust

Not for benchmark reasons — the ML runtime dominates either way. For these:

- The PAM module is loaded into `sudo`, `login`, and the screen locker, all of which
  run as root. A memory-safety bug there is a root compromise. Rust gives a
  memory-safe `cdylib` with a C ABI, which is exactly the shape PAM wants.
- Untrusted input (V4L2 frames, ONNX tensors, enrolled template files) gets parsed in
  safe code.
- Single static binary, no interpreter, no import path to hijack. Howdy's PAM module
  `posix_spawn`s a Python interpreter; that is a large, mutable trust surface.

Honest counterpoint: Rust does not make the *design* secure. The privilege split in
[04](04-architecture.md) is what does that. Rust just removes one class of bug.

## What "unique" means here

Five things nothing on Linux currently does together:

1. **Active IR liveness** — strobe the emitter, difference the frames. Cheap, and a
   screen or photo physically cannot pass it. ([05](05-vision-pipeline.md))
2. **Zero idle cost** — socket-activated, idle-exits, pre-warms on a compositor
   signal so the camera is already streaming before you're asked to authenticate.
   ([06](06-performance-budget.md))
3. **No ML in the root process** — the PAM module contains no camera and no
   inference code at all. ([04](04-architecture.md))
4. **TPM-sealed templates** — embeddings at rest are useless without the machine's
   TPM and matching PCR state. ([03](03-pam-and-security.md))
5. **Correct PAM semantics** — hard fail-closed, real rate limiting, instant
   cancellation so it composes with kscreenlocker's dual PAM stacks.
   ([07](07-desktop-and-packaging.md))

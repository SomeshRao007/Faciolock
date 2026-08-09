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
- **Not a daylight system yet, on the reference hardware.** Added 2026-08-08 from
  measurement; **the reason was corrected later the same day, and the scope is provisional
  rather than permanent** ([B18](11-bug-log.md#b18)).

  What holds: above an ambient floor of roughly 1, no **magnitude** statistic separates an
  honest capture from one with IR injected — ratios, SNRs and responses all carry the
  ambient pedestal and converge as it rises. `dark_ratio_max` is a magnitude and cannot be
  rescued. So today Faciolock authenticates in **dim and artificially-lit rooms** —
  evening, night, curtains drawn, ceiling light — and **refuses** in a daytime room with a
  real window, reporting `Environment` so PAM falls through to the password. A large room
  whose only sunlight is indirect and ~10 m away reads 13.91.

  What was **wrong**: this was first written up as a property of the sensor, on the theory
  that purpose-built face-auth optics use a 940 nm emitter behind a bandpass filter and
  would see a lower floor in the same room. That reasoning is refuted by its own premise —
  Windows Hello succeeds in that room *on this same laptop*, with this same sensor,
  emitter and glass. Optics cannot differ from themselves. The camera's Microsoft
  face-authentication capture mode was then found unset and tried: it works, and it does
  not move the ambient floor either
  ([10/Probe 7](10-hardware-probe.md#probe-7--msxu-face-authentication-mode)).

  What actually blocks it: **the falloff check is ambient-invariant and has never been
  calibrated.** It runs on the ON−OFF difference, where the pedestal cancels exactly,
  leaving the emitter's inverse-square pattern over the face box — subject geometry, which
  ambient light cannot change. Measured across the corpus, the daylit live band
  (CV 0.307–0.385, including the problem room and a capture at pedestal 51.7) sits inside
  the nocturnal band (0.266–0.348). The gate sits in front of the whole burst, so it takes
  this check down along with the one it was derived to protect.

  It stays a non-goal **pending negative-class calibration** — n=2–3 daylit captures and no
  flat-presentation recording at all. Opening it now would trade a measured refusal for an
  unmeasured acceptance. The unlock is ten minutes of recording in the bright room, not
  different hardware: a flat printed face, the same print curved, and the print *with* an
  IR floodlight (`tools/record-negatives.sh`). No printer is needed and the face need not
  be the enrolled user's — falloff asks whether a surface is flat, not who it is. A screen
  cannot substitute: `spoof_phone` reads `no face`, because the IR camera sees the emitter
  reflected off the glass rather than the image, so screens are refused by detection and
  never reach the check.

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

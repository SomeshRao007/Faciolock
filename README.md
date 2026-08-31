# Faciolock

IR face authentication for Linux, via PAM. Written in Rust, built to behave like a
platform component rather than an add-on.

> [!WARNING]
> **Alpha. It unlocks a screen, and it has been run on exactly one laptop.**
>
> The full stack works end to end — enrolment, `sudo`, SDDM and the KDE lock screen all
> authenticate by face on the reference machine. Two measured limitations ship with it:
> face authentication **refuses in bright daylight**, and a **printed photograph is not
> reliably rejected**. Both are below, neither is a surprise, and neither is fixed.
> See [Status](#status) and [Hardware](wiki/15-hardware-compatibility.md).

---

## Security ceiling — read this first

**Face authentication is a convenience factor, not a password replacement.** Faciolock is
designed to sit alongside your password, never to replace it, and a password must always
remain reachable. This is stated here as plainly as [Howdy](https://github.com/boltgolt/howdy)
states it, for the same reasons.

What that means concretely:

- A sufficiently determined attacker with a purpose-built artefact — an actively
  NIR-illuminated 3D mask — is **not reliably rejected**. This is also the bar Windows
  Hello sits at.
- **A flat printed photograph is not reliably rejected either, and that is worse than the
  line above.** Measured, not assumed: a magazine print read *above* the worst live face
  on the falloff statistic the liveness check uses, so there is currently **no enforced
  check that rejects a print**. A print costs a printer, not a workshop.
  [B24](wiki/11-bug-log.md#b24).
- **In bright daylight, face authentication refuses.** Ambient infrared saturates the
  ratio the liveness checks are built on, so honest captures are rejected and you type
  your password. It fails closed rather than open, which is the right direction, and it
  is still the largest open defect in the project. [B18](wiki/11-bug-log.md#b18).
- Every claim below distinguishes what has been **measured** from what has been
  **assumed**. Some checks that sound obvious have already been refuted by measurement
  (see [B17](wiki/11-bug-log.md)); assume nothing on this page is true because it is
  plausible.
- Faciolock will ship disabled by default and require explicit opt-in.

If you need a second factor that resists a motivated attacker, use a hardware token.

---

## Why this exists

Kubuntu and Debian ship no IR face authentication. `fprintd` covers fingerprints and is a
first-class citizen — D-Bus daemon, PAM module, desktop settings integration. The
equivalent for the IR camera that ships on most business laptops does not exist in the
archive. The only real option is Howdy: a third-party PPA/AUR install, Python-based, whose
own author warns against using it as a sole authentication factor.

Faciolock aims at the thing that is missing: an IR face authentication stack that a distro
could actually ship.

| Goal | Measured as |
|---|---|
| Works everywhere a password works | login, greeter, screen locker, `sudo`, `su`, polkit |
| Fast | p95 auth ≤ 400 ms, PAM entry to verdict |
| Light | 0 bytes resident when idle |
| Accurate | FRR < 3% at FAR < 1e-4, IR mode |
| Secure | replay defeated by design, not by a heuristic |
| Shippable | 100% DFSG-compatible licensing, models included |

That last row is a hard constraint, not an aspiration, and it has already cost two model
choices — InsightFace and EdgeFace both ship permissive *code* with non-commercial
*weights*. See [B10](wiki/11-bug-log.md).

---

## How it works

```
V4L2 ─→ cheap liveness ─→ detect ─→ ROI liveness ─→ align ─→ embed ─→ match ─→ verdict
frames  dark-frame,       YuNet     falloff,        112×112   SFace     cosine
        response                    motion          chip                k-of-n
        (no box needed)             (needs the box)
```

**Nothing is embedded or matched until liveness has passed.**

The differentiating idea is that liveness is **arithmetic, not a neural network**. The IR
camera on the reference machine already alternates emitter-lit and emitter-dark frames in
hardware — 15 ON/OFF pairs per second, with no input from us. Differencing those frames
gives a depth and reflectance signal for the cost of a few `f32` reductions: no PAD model,
no extra weights, no licence question, and **no writes to the camera device**.

Two aggregation rules run in opposite directions over the same burst, and they must never
be unified:

| | Rule | Why |
|---|---|---|
| **Liveness** | **every** pair must pass | one contaminated frame is evidence of an attack, and evidence does not average out |
| **Matching** | k of n must agree | one bad frame is a blink or a blur, and flukes *should* average out |

Getting that wrong is not theoretical. A pulsed IR source passed a per-pair dark-frame
check on 16.7% of pairs, which at the capture budget is a **1-in-36 authentication
bypass**. Conjunctive aggregation rejects it with four orders of magnitude to spare —
[B9](wiki/11-bug-log.md).

Architecture is copied in spirit from `fprintd`, which is the design Debian already ships
and trusts: an unprivileged, socket-activated daemon owns the camera and the ML, and a
small PAM module owns nothing but a Unix socket. The PAM module is loaded into `sudo` and
the screen locker, both running as root, so it is required to have a dependency tree of
exactly `libc` and the IPC crate — auditable in a `cargo tree` diff on every change.

Full design: [`wiki/`](wiki/).

---

## Status

| Phase | Scope | State |
|---|---|---|
| 0 | Hardware probe, corpus, runtime spikes | done, one item deferred |
| 1 | `faciolock-vision` — frames in, verdict out | done |
| 2 | `v4l` capture, `faciolockd` skeleton | done |
| 3 | PAM module + IPC | done |
| 4 | Template store, encryption, rate limiting | done — TPM sealing deferred to post-v1 |
| 5 | `faciolockctl` enrolment CLI | done |
| **6** | **systemd integration, sandboxing, all PAM services** | **done — the KDE lock screen unlocks by face** |
| 7 | Packaging, PPA release | in progress |

### What is verified

Measured on the reference machine against recorded captures, not asserted:

| Claim | Evidence |
|---|---|
| Hardware self-strobes, 15 ON/OFF pairs/sec | 3 independent runs |
| Dark-frame integrity rejects an IR floodlight | 4 orders of magnitude margin, 0 false rejects in 210 live pairs |
| One contaminated pair sinks a burst | replayed in CI, synthetic and recorded |
| YuNet detects faces in **near-infrared** | 5/5 live captures; 0 on an empty room, a floodlight, and a phone screen |
| SFace discriminates on **near-infrared** | genuine cross-capture cosine 0.60–0.90 against a 0.363 threshold |
| Detection and alignment match OpenCV | box IoU 0.998–0.999; aligned chip mean diff 0.000, max 1 |
| A phone screen showing a photo is rejected | verdict `NoFace` — though see below |
| **The KDE lock screen unlocks by face** | 2026-08-31, twice, on a trackpad touch with no keystroke |
| **`sudo` authenticates by face** | 2026-08-31, `sudo -K` then `sudo true`, no password prompt |
| **A login through a display manager** | SDDM, 2026-08-31 — though the greeter decides *when* the face check runs, not us |
| **Zero bytes resident when idle** | socket-activated, idle-exits, reactivates on the same inode under real systemd |
| **The sandbox scores 0.6 and still does the work** | `systemd-analyze security`, and the same run opened the camera and loaded both models under it |
| **A refused or unjudgeable face still lets you in with a password** | observed through `pam-auth-update`, and nothing that never saw a face moved the failure budget |
| **The PAM module's dependency tree is `libc` + the IPC crate** | `cargo tree`, asserted by a test on every change |

### What is not

Stated as plainly as the verified list, because a security claim without evidence is worth
nothing:

- **Printed photos are tested, and they win.** The deferral was closed on 2026-08-19 and the
  answer was the bad one: a magazine print measured CV 0.2733 against a worst live face of
  0.2661, so the falloff check reads the print as *more* alive than a person. No liveness
  check currently rejects a print. [B24](wiki/11-bug-log.md#b24).
- **Daylight rejects honest faces.** Every strobe statistic is a ratio and ambient infrared
  saturates a ratio. The gate fails closed, so the cost is a password prompt rather than a
  bypass, but face authentication is effectively unavailable in a sunlit room.
  [B18](wiki/11-bug-log.md#b18).
- **The response test does not do what it was designed to do.** It was meant to reject
  screens on the grounds that an LCD emits nothing at 850 nm. A phone measured `D = 36.2`
  against a live minimum of 48.6, not the predicted ~0, because the statistic is
  whole-frame and the frame is mostly room. The phone was rejected — by *detection*, not by
  this check. [B17](wiki/11-bug-log.md).
- **FAR is unmeasurable.** The corpus has one subject. The cosine threshold is OpenCV's
  RGB-calibrated 0.363 — paper-derived and provisional, and it does not meet this project's
  own "measured on our own corpus" standard.
- **Latency has two measurements and they disagree.** An end-to-end bench against the
  release daemon and the wiki's per-stage table differ by 133 ms in a direction that is
  arithmetically impossible, so one of them is measuring something other than its label.
  Until the frame burst is timed inside the daemon, **neither number is quotable** and none
  is quoted here.
- **One machine.** Every hardware claim comes from a single ASUS Vivobook.

Three of the four liveness checks report as `advisory` rather than `enforced` in the
verdict, so the audit log cannot overstate what was actually checked.

---

## Installing

You need an **IR camera**. Check first — [`wiki/15`](wiki/15-hardware-compatibility.md)
has the one-liner, and an RGB webcam will not do.

Three packages: `faciolock` (daemon, CLI, units), `libpam-faciolock` (the module and its
`pam-auth-update` profile), `faciolock-models` (the weights). Build them from a checkout:

```bash
./tools/mk-source-package.sh --binary
```

**Installing does not enable anything.** The profile registers at `Default: no`, so no PAM
stack names `pam_faciolock.so` until you run `pam-auth-update` and tick it. Then:

```bash
sudo faciolockctl diagnose --write
```
```bash
sudo faciolockctl enroll $USER
```
```bash
sudo faciolockctl test $USER
```

`test` exercises the whole path without touching a single PAM file. For anything that
authenticates inside your own desktop session — the lock screen above all — you also have
to join the group the socket is gated on, and log out and back in:

```bash
sudo gpasswd -a $USER faciolock-auth
```

Enable it on `sudo` alone first, hold a root shell in a second terminal while you do, and
read the recovery path before you start: [`wiki/07`](wiki/07-desktop-and-packaging.md#rolling-it-out-and-getting-back-out).
Every failure mode — daemon down, not enrolled, rate-limited, refused — falls through to
your password by construction, and the worst a broken PAM file can do is make one service
refuse to authenticate. It cannot make a machine unbootable.

---

## Building

Requires a Rust toolchain (1.93+) and nothing else — the inference runtime is
[`tract`](https://github.com/sonos/tract), pure Rust, so the built binary links only
`libc`, `libm` and `libgcc_s`. There is no `onnxruntime`, no OpenCV and no Python in the
runtime dependency chain.

```bash
cargo test -p faciolock-vision
```

Tests that need neither the ONNX models nor the recorded captures always run; those that do
need them print a visible `SKIPPED — DEGRADED RUN` banner rather than passing quietly. Set
`FACIOLOCK_CORPUS_REQUIRED=1` to make absence a hard failure.

```bash
cargo test --release -p faciolock-vision -- --nocapture
```

Reports the measured numbers: per-stage latency, landmark convergence across detector input
sizes, cross-capture similarity, and class separation for each liveness metric.

### Models

Not in version control — they are large upstream binaries. Fetch from OpenCV Zoo via
`media.githubusercontent.com` (the plain `raw.` URL returns a 131-byte LFS pointer):

| Model | Licence | Purpose |
|---|---|---|
| `face_detection_yunet_2026may.onnx` | MIT | detection + 5 landmarks, ~230 KB |
| `face_recognition_sface_2021dec.onnx` | Apache-2.0 | 128-d embedding, ~37 MB |

Provenance and licence status per file: [`models/README.md`](models/README.md).

### Regenerating golden references

Detection and alignment are pinned against OpenCV, so that a bug present from the first
commit cannot be frozen in as correct. This found a real defect —
[B16](wiki/11-bug-log.md), a half-pixel sampling offset invisible to four self-consistent
tests. Needs a throwaway venv, never the runtime:

```bash
python3 -m venv .venv && .venv/bin/pip install opencv-python-headless numpy
```
```bash
.venv/bin/python tools/gen-goldens.py
```

---

## Layout

```
crates/faciolock-vision/   the pipeline: liveness, detection, alignment, embedding, matching
crates/faciolock-camera/   V4L2 capture
crates/faciolock-daemon/   faciolockd: the socket, the store, the policy, the audit log
crates/faciolock-pam/      pam_faciolock.so -- libc + the IPC crate, and nothing else
crates/faciolock-ipc/      the wire protocol both ends share
crates/faciolock-cli/      faciolockctl: enrol, diagnose, test, bench
data/                      systemd units, sysusers, polkit, the pam-configs profile
debian/                    the three binary packages
corpus/                    recorded IR captures (.gray gitignored, sidecars committed)
models/                    vendored ONNX + per-file licence status
spikes/                    Phase 0 throwaway measurement tools
fuzz/                      fuzz targets, excluded from the workspace on purpose
tools/                     installer, harnesses, corpus recorder, golden generator
wiki/                      design, threat model, budgets, and the bug log
```

The [bug log](wiki/11-bug-log.md) is the most useful page in the wiki. Most entries are
*design* bugs found by measurement rather than typos — a threshold that was defeated by a
pulsed light source, a model whose weights were not licensed the way its repository was, a
benchmark that measured the CPU's boost clock. They are written down so the same class gets
caught again.

---

## Licence

**GPL-3.0-or-later** — [`LICENSE`](LICENSE).

Vendored model files carry their own upstream licences (MIT and Apache-2.0) and are
tracked separately in [`models/README.md`](models/README.md); the project licence does not
apply to them. Every crate in the dependency tree is permissively licensed and compatible;
the tally is in [`debian/copyright`](debian/copyright).

## Security

**Do not report suspected vulnerabilities via public issues.** Email
`cokasomeshrao@outlook.com` with `[faciolock security]` in the subject.

Full policy, timelines, and the list of things that are known limitations rather than
vulnerabilities: [`SECURITY.md`](SECURITY.md). Read the out-of-scope section before
writing — a printed photograph defeating liveness is documented, not news.

## Acknowledgements

- [Howdy](https://github.com/boltgolt/howdy) — the prior art, and honest about its own
  limits in a way that set the standard for this project's documentation.
- [OpenCV Zoo](https://github.com/opencv/opencv_zoo) — YuNet and SFace.
- [`fprintd`](https://gitlab.freedesktop.org/libfprint/fprintd) — the privilege-separation
  design this copies.
- [`tract`](https://github.com/sonos/tract) — pure-Rust ONNX inference.

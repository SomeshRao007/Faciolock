# Faciolock

Infrared face authentication for Linux, via PAM. Written in Rust and built to behave like
a platform component: a socket-activated system daemon, a minimal PAM module, and native
KDE Plasma integration.

Faciolock works everywhere a password works — login, the SDDM greeter, the KDE lock
screen, `sudo`, `su` and polkit — and a password always remains available as a fallback.

## Features

- **IR liveness without a neural network.** Liveness is computed from the camera's own
  emitter-lit and emitter-dark frames. No presentation-attack model, no extra weights,
  and no writes to the camera device.
- **Pure-Rust inference.** Face detection (YuNet) and recognition (SFace) run on
  [`tract`](https://github.com/sonos/tract). No OpenCV, no `onnxruntime`, no Python at
  runtime; the binaries link only `libc`, `libm` and `libgcc_s`.
- **Privilege separation.** An unprivileged, sandboxed daemon owns the camera and the
  models. The PAM module owns nothing but a Unix socket, and its dependency tree is
  exactly `libc` and the IPC crate — enforced by a test.
- **Zero memory when idle.** The daemon is socket-activated and exits when idle.
- **Encrypted template store**, a persistent per-user failure budget, and an audit log.
- **Desktop integration.** A KDE System Settings page, and SDDM and lock-screen themes
  that start a face scan as soon as the greeter appears.
- **Safe by construction.** Every failure mode — daemon down, not enrolled,
  rate-limited, refused — falls through to the password prompt.

## Security notice

**Face authentication is a convenience factor, not a password replacement.** Faciolock
is designed to sit alongside your password, never to replace it.

Known limitations:

- A flat **printed photograph is not reliably rejected** by the liveness checks.
- A purpose-built 3D mask under active near-infrared illumination is not reliably
  rejected — the same bar Windows Hello sits at.
- The false-accept rate has not been measured on a multi-subject dataset. The cosine
  threshold is OpenCV's published 0.363.
- In bright daylight, authentication requires a usable emitter signal; weak, irregular
  or saturated captures are refused and fall through to the password.

If you need a second factor that resists a motivated attacker, use a hardware token.

## How it works

### Pipeline

```
V4L2 ─→ cheap liveness ─→ detect ─→ ROI liveness ─→ align ─→ embed ─→ match ─→ verdict
frames  dark-frame,       YuNet     falloff,        112×112   SFace     cosine
        response                    motion          chip                k-of-n
        (no box needed)             (needs the box)
```

**Nothing is embedded or matched until liveness has passed.**

IR cameras built for Windows Hello alternate emitter-lit and emitter-dark frames in
hardware, roughly 15 ON/OFF pairs per second. Differencing those pairs gives a depth and
reflectance signal for the cost of a few `f32` reductions. Every pair must show real
emitter modulation (OFF/ON ≤ 0.9 and ON−OFF ≥ 5 intensity levels) in a clear alternating
pattern.

Two aggregation rules run over the same burst, deliberately in opposite directions:

| | Rule | Why |
|---|---|---|
| **Liveness** | **every** pair must pass | one contaminated frame is evidence of an attack, and evidence does not average out |
| **Matching** | k of n must agree | one bad frame is a blink or a blur, and flukes *should* average out |

### Architecture

```
  sudo / su / login / sddm / kscreenlocker / polkit
                       │  PAM
                       ▼
             pam_faciolock.so            libc + faciolock-ipc only
                       │  SOCK_SEQPACKET, /run/faciolock.sock
                       ▼                 (0660 root:faciolock-auth)
                 faciolockd              socket-activated, sandboxed, unprivileged
           ┌───────────┼───────────┐
           ▼           ▼           ▼
       IR camera    YuNet +    encrypted templates,
        (V4L2)       SFace     failure budget, audit log
```

The design follows `fprintd`: the daemon owns the camera, the models and the templates;
the PAM module, which is loaded into processes running as root, asks one question over a
socket and returns.

## Requirements

- **An IR camera** of the kind shipped for Windows Hello. An RGB webcam will not work.
- A Debian- or Ubuntu-based distribution with systemd and PAM.
- KDE Plasma 6 for the optional settings page and greeter integration.

To check for an IR camera, install `v4l-utils` and run:

```bash
for d in /dev/video*; do echo "=== $d"; v4l2-ctl -d "$d" --info --list-formats 2>&1 | grep -E "Card type|\[[0-9]\]"; done
```

Look for a device whose card type mentions **IR** and whose only pixel format is
**`GREY`**.

## Installation

Faciolock is installed as Debian packages built from this repository.

| Package | Contents |
|---|---|
| `faciolock` | the daemon `faciolockd`, the `faciolockctl` CLI, systemd units, polkit policy |
| `libpam-faciolock` | the PAM module and its `pam-auth-update` profile |
| `faciolock-models` | the YuNet and SFace ONNX models |
| `plasma-faciolock-kcm` | *optional* — the KDE System Settings page |
| `plasma-faciolock-greeters` | *optional* — automatic face scanning in SDDM and the lock screen |

### 1. Install the build dependencies

```bash
sudo apt install git curl rsync build-essential debhelper devscripts lintian cargo rustc libpam0g-dev cmake extra-cmake-modules libkf6kcmutils-dev libkf6coreaddons-dev qt6-declarative-dev
```

Rust 1.85 or newer is required.

### 2. Fetch the models

The models are not stored in git. Download them from the OpenCV Zoo and verify them:

```bash
curl -L -o models/face_detection_yunet_2026may.onnx https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/face_detection_yunet/face_detection_yunet_2026may.onnx
```

```bash
curl -L -o models/face_recognition_sface_2021dec.onnx https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/face_recognition_sface/face_recognition_sface_2021dec.onnx
```

```bash
(cd models && sha256sum -c MANIFEST.sha256)
```

### 3. Build the packages

```bash
./tools/mk-source-package.sh --binary
```

The packages are written to `../faciolock-build/`.

### 4. Install

```bash
cd ../faciolock-build && sudo apt install ./faciolock_*.deb ./libpam-faciolock_*.deb ./faciolock-models_*.deb
```

On KDE Plasma, also install the settings page and greeter integration:

```bash
cd ../faciolock-build && sudo apt install ./plasma-faciolock-kcm_*.deb ./plasma-faciolock-greeters_*.deb
```

## Setup

Detect the camera and write the daemon configuration:

```bash
sudo faciolockctl diagnose --write
```

Enrol your face (three poses):

```bash
sudo faciolockctl enroll
```

Run one authentication exactly as PAM would, without touching any PAM configuration:

```bash
sudo faciolockctl test
```

To unlock the KDE lock screen by face, add your account to the `faciolock-auth` group,
then log out and back in:

```bash
sudo gpasswd -a $USER faciolock-auth
```

The PAM profile is enabled on install. Until a face is enrolled, authentication falls
through to the password prompt.

## Usage

| Command | Description |
|---|---|
| `faciolockctl enroll [USER]` | guided enrolment, three poses |
| `faciolockctl list` | list enrolled users |
| `faciolockctl remove USER [--yes]` | delete an enrolment |
| `faciolockctl test [USER]` | one authentication, exactly as PAM would request it |
| `faciolockctl status [USER]` | whether a user is enrolled; needs no root and no camera |
| `faciolockctl enable` / `disable` | turn face authentication on or off system-wide |
| `faciolockctl bench [-n N] [USER]` | N authentications, reporting p50/p95/p99 latency |
| `faciolockctl diagnose [--write]` | detect the camera and generate the daemon configuration |

`USER` defaults to `$SUDO_USER`. `status` and `diagnose` run as your own user; the other
commands require root. The daemon configuration lives in `/etc/faciolock/faciolockd.toml`.

On KDE Plasma, the same actions are available under **System Settings → Face Unlock**.

## Uninstall

```bash
sudo faciolockctl remove $USER
```

```bash
sudo apt purge plasma-faciolock-greeters plasma-faciolock-kcm libpam-faciolock faciolock faciolock-models
```

If a PAM change ever locks you out of a service, a root shell (or the recovery entry in
the boot menu) can run `faciolockctl disable`. Full recovery notes ship in
`/usr/share/doc/libpam-faciolock/README.Debian`.

## Development

Build and test the workspace:

```bash
cargo build --workspace --release
```

```bash
cargo test --workspace
```

Tests that need the ONNX models or recorded IR captures print a visible
`SKIPPED — DEGRADED RUN` banner when those files are absent, rather than passing
silently. Set `FACIOLOCK_CORPUS_REQUIRED=1` to make their absence a hard failure.

Detection and alignment are pinned against OpenCV reference outputs. To regenerate the
golden files, use a throwaway virtual environment — never the runtime:

```bash
python3 -m venv .venv && .venv/bin/pip install opencv-python-headless numpy
```

```bash
.venv/bin/python tools/gen-goldens.py
```

### Project layout

```
crates/faciolock-vision/   the pipeline: liveness, detection, alignment, embedding, matching
crates/faciolock-camera/   V4L2 capture
crates/faciolock-daemon/   faciolockd: the socket, the template store, the policy, the audit log
crates/faciolock-pam/      pam_faciolock.so — libc + the IPC crate, nothing else
crates/faciolock-ipc/      the wire protocol shared by both ends
crates/faciolock-cli/      faciolockctl: enrol, diagnose, test, bench
data/                      systemd units, sysusers, polkit policy, pam-configs profile, greeters
debian/                    Debian packaging
kcm/                       the KDE settings page (C++/QML, CMake)
models/                    model manifest and per-file licence status
fuzz/                      fuzz targets, kept out of the workspace on purpose
spikes/                    standalone hardware and runtime measurement tools
tests/                     greeter QML tests
tools/                     packaging, installer, test harnesses, golden generator
```

## Security

Please do not report vulnerabilities through public issues. Use GitHub's
[private vulnerability reporting](https://github.com/SomeshRao007/Faciolock/security/advisories/new)
instead.

## License

**GPL-3.0-or-later** — see [`LICENSE`](LICENSE).

The model files carry their own upstream licences (MIT for YuNet, Apache-2.0 for SFace)
and are tracked in [`models/README.md`](models/README.md). Every crate in the dependency
tree is permissively licensed; the full tally is in [`debian/copyright`](debian/copyright).

## Acknowledgements

- [Howdy](https://github.com/boltgolt/howdy) — the prior art for face authentication on
  Linux.
- [OpenCV Zoo](https://github.com/opencv/opencv_zoo) — the YuNet and SFace models.
- [`fprintd`](https://gitlab.freedesktop.org/libfprint/fprintd) — the privilege-separation
  design this follows.
- [`tract`](https://github.com/sonos/tract) — pure-Rust ONNX inference.

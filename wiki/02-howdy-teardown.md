# 02 — Howdy Teardown

Source: <https://github.com/boltgolt/howdy>. Read to understand what to keep and
what to change. Howdy is a good project and the prior art here — this page is a
design critique, not a dunk.

## Repository layout

```
howdy/src/
├── pam/            C++  — the PAM module (main.cc, ~15 KB)
│   ├── main.cc         pam_sm_authenticate, process spawn, race logic
│   ├── enter_device.cc /dev/uinput synthetic Enter keypress
│   └── optional_task.hh
├── cli/            Python — add/remove/list/test/config subcommands
├── recorders/      Python — camera capture abstraction
├── rubberstamps/   Python — post-match "extra checks" (nod, blink…)
├── compare.py      Python — the actual recognition, spawned per auth
├── config.ini
└── dlib-data/      model download scripts
```

Build: Meson + Ninja. Runtime deps: Python 3.6+, dlib, OpenCV, INIReader, libevdev.

## How authentication actually flows

```
sudo/login/kscreenlocker  (root)
        │
        └─ dlopen pam_howdy.so                       ← C++, in the root process
             │
             ├─ INIReader parses /lib/security/howdy/config.ini
             ├─ checks: disabled? SSH session? lid closed? model file exists?
             │
             ├─ posix_spawnp(python3, compare.py, username)
             │      └─ Python + OpenCV + dlib start cold, open camera,
             │         capture, detect, embed, compare, exit(code)
             │
             ├─ concurrently: pam_get_authtok()  ← password task
             │
             ├─ whichever finishes first wins; loser gets SIGTERM
             │
             └─ on success, writes a synthetic Enter to /dev/uinput
                to unblock the UI's password prompt
```

## What Howdy gets right — keep these

- **The PAM module does not do recognition itself.** It spawns a separate process.
  That instinct is correct; we take it further (a persistent daemon, not a spawn).
- **Racing face against password.** Never make the user wait for the camera to fail
  before they can type. This is mandatory, not optional.
- **Config-driven disable, SSH detection, lid-state check.** All sensible guards.
- **Per-user model files keyed by username.** Simple and right.

## Where it's weak

### W1 — Cold start on every single authentication

Every `sudo` pays the full cost of: fork/exec, Python interpreter boot, `import cv2`,
`import dlib`, model load from disk, V4L2 device open, camera warm-up, then inference.
On a typical laptop this is 1–3 seconds, and most of it is not the recognition. This
is the single biggest reason Howdy "feels" heavy. A resident daemon deletes almost
all of it.

### W2 — Enormous, mutable trust surface reachable from a root process

`pam_howdy.so` is loaded into `sudo` and `login`. It then executes a Python
interpreter, which resolves `import` statements against a filesystem path, loads
OpenCV and dlib shared objects, and parses a config file. Everything on that path is
part of the root attack surface. Compare to `pam_fprintd`, which speaks D-Bus to an
already-running daemon and links nothing heavy.

The upstream mitigation is "keep it read-only in `/lib/security`". That is a
deployment convention, not an architectural guarantee.

### W3 — The `/dev/uinput` Enter-key injection

To unblock a UI that is blocking on a password prompt, Howdy synthesizes an Enter
keypress into the global input stream. This is a workaround for applications that
don't handle a PAM module succeeding without a token. It is:

- a privileged, global side effect from inside an auth module,
- racy (the keystroke can land in the wrong window),
- and unnecessary on modern KDE, which runs multiple PAM stacks concurrently and
  cancels the loser ([07](07-desktop-and-packaging.md)).

We will not ship anything like this.

### W4 — No real liveness

Howdy's own README: a well-printed photo may be enough. `rubberstamps/` adds optional
challenge-response (nod, blink) but that is a usability tax, is defeated by a video
replay, and is off by default. Using an IR camera is treated as a *quality*
improvement, not as the *security* mechanism it can be. See [05](05-vision-pipeline.md).

### W4b — Howdy does not control the IR emitter at all

Worth stating explicitly because it defines the gap we're filling. Howdy opens the
device and hopes the LEDs come on. When they don't, the answer is "go install
[linux-enable-ir-emitter](https://github.com/EmixamPP/linux-enable-ir-emitter)" — an
external tool, run once, that brute-forces UVC controls until the emitter lights and
then leaves it configured. Integration has been an open request for years
([#567](https://github.com/boltgolt/howdy/issues/567)); the emitter is simply not
Howdy's problem.

Two consequences:

1. The emitter is a **binary on/off setup step**, never a per-frame signal. Nothing
   in Howdy could implement strobe liveness even if it wanted to.
2. Emitter trouble is the single most common Howdy support issue
   ([#543](https://github.com/boltgolt/howdy/issues/543),
   [#699](https://github.com/boltgolt/howdy/issues/699),
   [#876](https://github.com/boltgolt/howdy/issues/876)), including the classic
   footgun where `/dev/video0` and `/dev/video2` both detect faces but only one
   drives the LEDs.

**Hazard we inherit if we do this natively**: some hardware is *motion-reactive* —
e.g. ASUS laptops with the Sonix 3277:0018, where the emitter is driven by a
proximity sensor and fires on its own. On those,
[running the UVC brute-force can hang the USB bus and freeze the machine](https://github.com/boltgolt/howdy/issues/1109).
Any emitter probing we write needs a hardware denylist and must never run
unattended at auth time — probing is a one-time `faciolockctl diagnose` operation,
never something the daemon does on the hot path.

### W5 — Templates stored in the clear

Face descriptors sit in a `.dat` file under the models dir. Anyone who can read them
gets a portable biometric template. There is no binding to the machine.

### W6 — No rate limiting or lockout

Nothing throttles repeated attempts. A brute-force presentation attack (many printed
variants, many angles) costs the attacker only time.

### W7 — Config parsing in the root process

`INIReader` runs on a file inside `pam_sm_authenticate`. Small, but it is untrusted-
adjacent parsing where none is needed. Config belongs to the daemon.

### W8 — Python

Not a moral failing — a packaging and startup-cost one. Python + dlib + OpenCV is
~400 MB of dependencies to authenticate a user, it is impossible to make start fast,
and dlib's `dlib_face_recognition_resnet_model_v1` is a 2017-era ResNet-29 that is
both slower and less accurate than 2021+ mobile-class models.

## Direct comparison table

| | Howdy | Faciolock (proposed) |
|---|---|---|
| PAM module language | C++ | Rust |
| Recognition process | spawned per-auth, Python | resident daemon, Rust |
| Cold start per auth | 1–3 s | ~0 (pre-warmed) |
| Code in root address space | PAM + INIReader + spawn | PAM + a Unix socket client |
| ML/camera in root process | no (subprocess) — but subprocess is unsandboxed | no, and daemon is sandboxed + unprivileged |
| Liveness | optional challenge-response | active IR strobe difference |
| Template at rest | plaintext `.dat` | TPM-sealed |
| Rate limiting | none | daemon-enforced |
| UI unblocking | `/dev/uinput` Enter injection | none needed |
| Models license | dlib CC0 (fine) | YuNet MIT / SFace Apache-2.0 / EdgeFace BSD-3 |
| In Debian/Ubuntu archive | no | design target |

## Reference: fprintd, the model to copy

`fprintd` is a D-Bus system daemon over `libfprint`; `pam_fprintd` is a thin PAM
module that talks to it. The predecessor `pam_fprint` drove the hardware directly
from the PAM module and was abandoned. The stated reason for the daemon layer is
arbitration — multiple applications competing for one device — which applies exactly
as much to a camera. This is the strongest available argument for our
[architecture](04-architecture.md), and it is the architecture Debian already trusts.

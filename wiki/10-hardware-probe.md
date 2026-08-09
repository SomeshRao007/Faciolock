# 10 — Hardware Probe Log

Lab notebook. Exact commands and raw output from probing the reference machine.
Kept verbatim so results are reproducible and so other people can run the same
sequence against their own hardware for the compatibility list.

**All commands here are read-only.** Nothing writes to the camera. This is the
complete, safe probe sequence — do not run `linux-enable-ir-emitter configure` or any
UVC brute-force as part of it ([why](02-howdy-teardown.md#w4b--howdy-does-not-control-the-ir-emitter-at-all)).

---

## Reference machine

ASUS Vivobook · Kubuntu 26 · ffmpeg 8.0.1-3ubuntu2 · v4l-utils · 2026-08-03

---

## Probe 1 — enumerate video devices and formats

```bash
for d in /dev/video*; do echo "=== $d"; v4l2-ctl -d "$d" --info --list-formats 2>/dev/null | grep -E "Card type|\[[0-9]\]"; done
```

```
=== /dev/video0
	Card type        : ASUS FHD webcam: ASUS FHD webca
	[0]: 'MJPG' (Motion-JPEG, compressed)
	[1]: 'YUYV' (YUYV 4:2:2)
=== /dev/video1
	Card type        : ASUS FHD webcam: ASUS FHD webca
=== /dev/video2
	Card type        : ASUS FHD webcam: ASUS IR camera
	[0]: 'GREY' (8-bit Greyscale)
=== /dev/video3
	Card type        : ASUS FHD webcam: ASUS IR camera
```

**Result: `/dev/video2` is the IR camera** — `GREY` and nothing else, and the card type
says so outright. Both heuristics from
[05](05-vision-pipeline.md#finding-the-ir-camera) fire on the same device.

Note `/dev/video1` and `/dev/video3` list **no formats at all**. They are not cameras;
see Probe 4.

### First run of this probe was inconclusive — kept as a lesson

The original attempt printed only the four `echo` lines and no format details.
`v4l2-ctl` was not installed, and `2>/dev/null` swallowed the "command not found".

**`2>/dev/null` hides a missing binary exactly as well as it hides a permission
error.** A probe that silently degrades to "no evidence" is worse than one that fails
loudly, because "no evidence" reads like "nothing there". The daemon's own device
enumeration must distinguish *absent*, *inaccessible*, and *present-but-not-IR* rather
than collapsing all three into one empty result.

```bash
sudo apt install v4l-utils
```

---

## Probe 2 — identify the IR device and its format

```bash
ffplay -f v4l2 -i /dev/video2
```

Relevant line from the output:

```
Input #0, video4linux2,v4l2, from '/dev/video2':
  Stream #0:0: Video: rawvideo (Y800 / 0x30303859), gray, 640x360, 55296 kb/s, 30 fps, 30 tbr
```

**Result: `/dev/video2` is the IR camera.**

| Property | Value | Why it matters |
|---|---|---|
| Pixel format | `Y800` / `gray` | 8-bit greyscale — RGB webcams rarely offer this. Primary IR heuristic ([05](05-vision-pipeline.md#finding-the-ir-camera)). |
| Resolution | 640×360 | Low and wide — consistent with an IR sensor, not a 1080p RGB webcam. |
| Frame rate | 30 fps | Gives 15 ON/OFF pairs per second. |
| Codec | `rawvideo` | **No MJPEG decode needed** — removes a JPEG parser from the daemon's attack surface. |

### Visual emitter check

While the stream above was running, a phone's front camera pointed at the bezel
showed a **faint purple glow, visibly blinking**. 850 nm NIR registers as purple on
most phone sensors; the IR-cut filter on rear cameras usually blocks it, front
cameras usually don't.

This confirmed the emitter fires on device open with no configuration — but *not*
what the blinking meant. Two candidates:

- **H1** — genuine per-frame strobing (would give us free active liveness)
- **H2** — PWM dimming aliasing against the phone's rolling shutter (constant
  illumination in practice, no free strobe)

The eye cannot distinguish these. The IR sensor can. Hence Probe 3.

---

## Probe 3 — per-frame mean luma (the decisive test)

```bash
ffmpeg -f v4l2 -i /dev/video2 -frames:v 60 -vf "signalstats,metadata=print:key=lavfi.signalstats.YAVG" -f null - 2>&1 | grep YAVG
```

Raw output, 60 frames, `lavfi.signalstats.YAVG` prefix stripped:

```
22.6954       23.412        14.3455       31.4154      ← AGC converging
 0.00126302   59.9136        0.00366753   61.9249
 0.00404514   61.0619        0.00392795   60.3695
 0.0040842    59.8574        0.00400608   59.3313
 0.0040625    58.8541        0.00408854   58.4122
 0.00403212   58.0754        0.00400174   57.7173
 0.00414931   57.4615        0.00385851   56.9237
 0.00415799   56.8727        0.00402778   56.46
 0.00415799   56.3348        0.00403212   55.9671
 0.00396701   55.8291        0.00417535   55.4042
 0.00419705   55.3045        0.0044184    55.063
 0.00423611   54.7756        0.00448785   54.6959
 0.00424045   54.6283        0.00430556   54.5722
 0.00414497   54.0984        0.00412326   54.2633
 0.00456163   53.7433        0.00436198   53.7693
```

**Result: H1 confirmed. The hardware strobes the emitter per frame.**

### Reading the data

1. **Frames 1–4 are AGC convergence** (22.7, 23.4, 14.3, 31.4). Discard the first
   ~5 frames of any capture. Pre-warming avoids paying this at authentication time.

2. **From frame 5, perfect alternation.** 27 clean ON/OFF pairs, no missed beats, no
   drift in phase. `~0.004` alternating with `~54–62`. A ratio of roughly **14,000:1**
   — this is not a marginal signal.

3. **OFF frames read `0.004`, not `0.000`.** Nonzero means genuine sensor read noise,
   i.e. a real exposure with the emitter dark — not a dropped, skipped, or synthetic
   frame. **We get a true dark reference for free.** This is what enables the
   external-IR-floodlight check ([05](05-vision-pipeline.md#the-strobe-difference-test)),
   which was not possible in the original hand-rolled-strobe design.

4. **ON frames decay 61.9 → 53.7 over ~2 s** (−13%). AGC settling and/or LED thermal
   droop. **Therefore: never threshold on absolute brightness.** Only the ON/OFF
   ratio and the spatial falloff pattern are stable across a capture. This is the
   single most important calibration constraint the probe produced.

### What this deleted from the project

| Deleted | Because |
|---|---|
| Native `UVCIOC_CTRL_QUERY` emitter control | hardware already strobes |
| Per-laptop emitter configuration database | nothing to configure |
| Motion-reactive hardware denylist | we never write a UVC control |
| USB-bus-hang risk | ditto |
| Runtime dep on `linux-enable-ir-emitter` | downgraded to `Suggests:` |

Capability detection becomes a **read-only observation** — capture 30 frames, test
for alternation — rather than a write-probe. Strictly safer and strictly simpler.
Full write-up: [Q3 in 08](08-open-questions.md#q3--ir-emitter--resolved-no-emitter-code-at-all).

---

## Probe 3b — repeat run, 24 hours later

Same command, same machine, different ambient lighting. Confirms the strobe is stable
and — more usefully — shows how far the *absolute* values move between sessions.

```
31.9318  32.9474  14.1797  44.2447        ← AGC converging, discard
 0.000164931  61.5877   0.000399306  63.7701
 0.000516493  62.9413   0.000503472  62.2813
 …                                        ← 28 clean alternating pairs
 0.000655382  56.5051   0.000863715  63.7969
 0.0010026    63.6564   0.00108073   63.4741
```

| | First run | This run | Change |
|---|---|---|---|
| OFF floor | ~0.0040 | ~0.0005 | **10× lower** |
| ON range | 61.9 → 53.7 | 63.8 → 56.5, then back to 63.8 | AGC re-opened mid-capture |
| ON/OFF ratio | ~14,000:1 | ~100,000:1 | both enormous |
| Alternation | 27 clean pairs | 28 clean pairs | stable |

**What this changes.** The alternation itself is rock solid — that claim survives.
But the OFF floor is not a constant, it is an ambient-noise reading that moved by an
order of magnitude between two runs on *the same machine*, and the ON series does not
even decay monotonically — AGC re-opened partway through and jumped it back to 63.8.

The existing rule "never threshold on absolute brightness" was written about the ON
frames. **It applies at least as strongly to the OFF frames**, which means the
dark-frame integrity check ([05](05-vision-pipeline.md#the-strobe-difference-test))
cannot be "OFF < some constant". It has to be a ratio against the ON frame in the same
pair. A fixed floor tuned in a dark room would reject every honest attempt in daylight.

The ratio is the only quantity here that stayed the same order of magnitude across
both runs, and it is the one to threshold on.

---

## Probe 3c — recording to disk duplicates frames unless you stop it

Found while building `tools/record.sh`. **This one silently corrupts a corpus**, so it
is written up in full.

Probes 3 and 3b use `-f null -`, which encodes nothing. The moment the same capture is
written to a file with `-f rawvideo`, ffmpeg rate-converts the output to a constant
30 fps and **duplicates frames** to fill it. A 30-frame capture came back as 15 distinct
sensor frames, each written twice:

```
  0 agc   29.27560     1 agc   29.27560     2 agc   29.27560     3 agc   29.27560
  ...
 20 ON    47.62171    21 ON    47.62171    22 off    0.00039    23 off    0.00039
 24 ON    72.89330    25 ON    72.89330    26 off    0.00066    27 off    0.00066
```

Every value appears exactly twice. The sensor's `ON,off,ON,off` becomes `ON,ON,off,off`
on disk.

**Why this is dangerous rather than merely annoying.** The file is exactly the expected
size. Every individual frame is a valid, well-formed image. Nothing errors. The only
symptom is that the ON/OFF pairing — which every liveness check in
[05](05-vision-pipeline.md#the-strobe-difference-test) is built on — is wrong. A corpus
recorded this way would produce plausible-looking numbers that mean nothing, and the
thresholds derived from it would be garbage.

**Fix:** `-fps_mode passthrough` on the output.

```bash
ffmpeg -f v4l2 -input_format gray -video_size 640x360 -framerate 30 -i "$dev" \
       -frames:v 60 -fps_mode passthrough -f rawvideo -pix_fmt gray out.gray
```

With the flag, from our own analysis code rather than ffmpeg's:

```
tier: ir-strobe

  0 agc   31.67136     1 agc   32.83174     2 agc   14.15270     3 agc   56.17587
  4 agc    0.00030     5 ON    86.91084     6 off    0.00086     7 ON    85.42877
  8 off    0.00096     9 ON    88.92887    10 off    0.00101    11 ON    88.07828
  …
 56 off    0.00151    57 ON    79.53946    58 off    0.00145    59 ON    79.40342
```

**Independent confirmation of Probes 3 and 3b.** Different tool, different code path,
same conclusion: 27 clean alternating pairs, AGC garbage in the first ~4 frames,
monotonic-ish ON droop (86.9 → 79.4, −8.6%), nonzero OFF floor.

Third data point on the OFF floor: **~0.0010** here, against 0.0040 and 0.0005 in the
two earlier runs. Three sessions, three different orders of magnitude. The
ratio-not-absolute rule holds up.

**General lesson for the daemon**, beyond ffmpeg: never assume the frames you were
handed are the frames the sensor produced. `faciolockd` reads V4L2 buffers directly and
so is not exposed to this specific bug, but it *is* exposed to the same class — dropped
buffers, repeated buffers, and reordering under load. ON/OFF phase must be
re-established by measuring luma on every capture, never by assuming frame `n` is ON
because frame `n-1` was OFF. Queued as a probe below.

---

## Probe 4 — device-node disambiguation

Four `/dev/videoN` nodes, two physical cameras. Which node is which, and what is the
stable name to put in config?

```bash
ls -l /dev/v4l/by-id/
ls -l /dev/v4l/by-path/ | grep -v usbv2
```

```
########## BY-ID
usb-Sonix_Technology_Co.__Ltd._ASUS_FHD_webcam_SN0001-video-index0 -> ../../video0
usb-Sonix_Technology_Co.__Ltd._ASUS_FHD_webcam_SN0001-video-index1 -> ../../video3

########## BY-PATH
pci-0000:00:14.0-usb-0:7:1.0-video-index0 -> ../../video0
pci-0000:00:14.0-usb-0:7:1.0-video-index1 -> ../../video1
pci-0000:00:14.0-usb-0:7:1.2-video-index0 -> ../../video2
pci-0000:00:14.0-usb-0:7:1.2-video-index1 -> ../../video3
```

### Finding 1 — `by-id` cannot address the IR camera on this hardware

**Two symlinks exist for four nodes, and neither one points at `/dev/video2`.**

Both cameras are interfaces of a single USB device that reports one serial
(`SN0001`), so udev generates the same `by-id` basename for both and the last writer
wins. What survives is `-video-index0 → video0` (the **RGB** camera) and
`-video-index1 → video3` (the IR **metadata** node, Finding 2). The actual IR camera
has no `by-id` name at all.

```bash
readlink -f /dev/v4l/by-id/usb-Sonix_Technology_Co.__Ltd._ASUS_FHD_webcam_SN0001-video-index0
# /dev/video0     ← the RGB camera
```

This invalidates a design decision. [04](04-architecture.md) and
[05](05-vision-pipeline.md) both said to resolve the camera by `by-id` symlink rather
than `videoN` because the numbering is unstable across boots. The instability is real,
but `by-id` is not the fix here — it silently resolves to the **wrong camera**, which
is worse than an unstable name that fails loudly.

**`by-path` is unambiguous**: all four nodes appear, and the USB interface number
separates the cameras — `…-usb-0:7:1.0-*` is the RGB pair, `…-usb-0:7:1.2-*` is the IR
pair. It is stable across boots for as long as the device stays on the same port, which
for an internal laptop camera is always.

Cost: `by-path` encodes the PCI address, so it is machine-specific and must come from a
per-machine probe rather than a shipped default. That was already the plan — the device
path is written to config by `faciolockctl diagnose` after a one-time probe.

### Finding 2 — half the nodes are not cameras

```bash
v4l2-ctl -d /dev/video3 --all | grep -E "Device Caps|Metadata|Sample Format"
```

```
		Metadata Capture
	Device Caps      : 0x04a00000
Format Metadata Capture:
	Sample Format   : 'UVCH' (UVC Payload Header Metadata)
```

`/dev/video1` and `/dev/video3` are **UVC payload-header metadata nodes**, one per
camera. They expose no capture formats and can never yield a frame.

Enumeration must filter on `V4L2_CAP_VIDEO_CAPTURE` before applying any IR heuristic.
Without it the probe walks four nodes, two of which are guaranteed dead ends, and the
"capture one frame and check chroma variance" fallback in
[05](05-vision-pipeline.md#finding-the-ir-camera) would hang or error on them rather
than cleanly skipping.

### Resulting device-selection order

1. Enumerate `/dev/video*`, **keep only nodes with `V4L2_CAP_VIDEO_CAPTURE`**
2. Prefer a node offering `GREY` — on this machine that alone is decisive
3. Tie-break on card-type name (`IR`, `Infrared`, `NIR`), then resolution
4. Persist the choice as the **`by-path`** symlink, never `by-id`, never `videoN`

---

## Probe 5 — first corpus, and the first real security result

Six captures, 90 frames each, 42 ON/OFF pairs each. Analysed with `spikes/probe`, not
ffmpeg. Every capture classified `ir-strobe` from our own code.

```
RECORDING                          median       MAX(OFF/ON)   verdict@1e-3
attack_floodlight_remote         7.18e-02        1.30e-01         REJECT
live_dark_frontal                3.21e-06        5.36e-06          allow
live_lit_angle_left              1.88e-06        5.03e-06          allow
live_lit_frontal                 2.00e-06        3.76e-06          allow
live_lit_frontal                 1.45e-06        4.43e-06          allow
live_lit_glasses                 1.73e-06        3.88e-06          allow
null_empty_room                  1.00e-05        1.54e-05          allow
```

**Dark-frame integrity is verified on real data.** Worst honest capture `1.54e-05`
against an attack at `1.30e-01` — four orders of magnitude. A threshold at `1e-3` sits
~65× above the worst live reading and ~130× below the attack. **Zero false rejections
across 210 live pairs**, including a dark room and glasses.

The catch, and it is the important part: the separation only holds when the burst is
aggregated **conjunctively**. Per-pair, the attack read as clean 16.7% of the time,
because an IR remote pulses. Full analysis: [B9](11-bug-log.md#b9).

### What the corpus could not answer

`falloffCV` came out unusable, for a reason unrelated to the missing print sample. Mean
response per cell, % of peak:

```
live_lit_frontal                     null_empty_room
   8   16   14   12   48  100          70   76   76   72   66   53
   9   10   10   66   47   94          77   82   81   78   72   59
   9    5   21   79   52   76          84   88   84   81   78   65
   6   25   38   74   69   77          90   95   90   87   84   70
  19   44   53   87   80   91          94  100   99   96   90   75
  39   54   57   92   79   94          77   66   73   80   81   71
```

The "frontal" capture peaks in the **bottom-right corner**. The subject is at the edge
of the centre-crop ROI, so the metric is measuring a subject-versus-background gradient
rather than facial curvature — two captures of the same pose gave 0.31 and 0.64.

**The falloff test needs a real face box before it can be evaluated at all.** See
[B7](11-bug-log.md#b7). This is a second, independent blocker on the falloff claim,
alongside the missing print.

---

## Probe 6 — the delivered frame rate is not the advertised one

Every earlier probe in this file reports **30 fps**, and every one of them got that number
from the device rather than from a clock. `ffplay`'s stream header, `VIDIOC_G_PARM` and
`record.sh`'s sidecar all report the rate that was *configured*.

Measured from kernel buffer timestamps (`v4l2_buffer.timestamp`, stamped by `uvcvideo` at
capture), 30 frames, 2026-08-06:

```
frame interval   min 63.8 ms   p50 68.0 ms   max 68.1 ms
delivered rate   14.7 fps
```

Steady, not jitter — the spread across 29 intervals is 4 ms. Two `ffmpeg` captures the same
morning independently read **12.11 and 12.12 fps** over 90 frames.

**So "15 ON/OFF pairs per second" was never measured either**; the real figure is ~6–7.
Consequences are in [B19](11-bug-log.md#b19) and the budget restatement in
[06](06-performance-budget.md).

**Settled 2026-08-06: the rate is *not* ambient-driven.** A controlled pair two minutes
apart, one with the blinds open and one with them closed under a ceiling light:

```text
capture                       ambient pedestal   fps delivered
live_daylight_frontal   12:14            51.74           12.11
attack_floodlight_dayl  12:14            20.33           12.01
live_night_lamp         12:16             0.00           12.15
```

The ambient floor moves from 51.74 to 0.00 and the frame rate does not move at all. So
**~12 fps is simply what this camera delivers**, and the 30 fps in every earlier probe was
the advertised figure. The halved rate and [B18](11-bug-log.md#b18) are independent
findings that arrived together.

The same pair is also the controlled experiment that confirms B18's mechanism: same
machine, same hour, one variable changed. And note the third row is a *midday* capture with
an ambient floor of zero — **ceiling light emits essentially no near-infrared, so "night"
here is an optical condition, not a time.** That is why the corpus recorded after dark and
the corpus recorded under a lamp at noon are the same condition, and why nothing in the
project noticed the variable until sunlight reached the sensor.

---

## Reproducing this on other hardware

For the [compatibility list](07-desktop-and-packaging.md#hardware-reality-check).
Run Probes 1–4 and report:

1. Which `/dev/videoN` reports `gray` / `Y800`
2. Its resolution and frame rate
3. Whether the YAVG series alternates (`ir-strobe`), stays flat and bright
   (`ir-constant`), or stays flat and dark (emitter off — needs
   `linux-enable-ir-emitter` as a one-time setup step)
4. **Whether `by-id` addresses your IR camera, or collides as it does here** — this is
   the one that most needs a second data point. If other vendors give their two UVC
   interfaces distinct serials, `by-id` works for them and only Sonix-style shared
   serials need `by-path`.

Install `v4l-utils` and `ffmpeg` first. Substitute your own device node — the numbering
is not stable across boots, which is why the daemon persists a `by-path` symlink rather
than `videoN`. See Probe 4 for why not `by-id`.

---

## Probe 7 — the camera has a face-authentication mode, and Linux never set it {#probe-7--msxu-face-authentication-mode}

**Why:** the reference machine dual-boots, and Windows Hello authenticates in a room where
Faciolock refuses ([B18](11-bug-log.md#b18)). Since it is the same sensor, emitter and
glass, the difference has to be in how the camera is *driven*, not in what it can see.

### What the descriptors say

```bash
lsusb -v -d 3277:0018 | awk '/bInterfaceNumber        2/,/bInterfaceNumber        4/'
```

The IR camera's VideoControl interface carries two extension units:

```text
EXTENSION_UNIT bUnitID 3   {28f03370-6311-4a2e-ba2c-6890eb334016}   Sonix, vendor
  bmControls 0x00                       <- nothing advertised on the IR path

EXTENSION_UNIT bUnitID 4   {0f3f95dc-2632-4c4e-92c9-a04782f43bc8}   Microsoft Camera XU
  bmControls 0x20 0x01                  <- D5 = selector 0x06 FACE_AUTHENTICATION
                                           D8 = selector 0x09 METADATA
INPUT_TERMINAL Camera Sensor  bmControls 0x00240000   -> Privacy only
PROCESSING_UNIT               bmControls 0x00000000   -> nothing
```

So the IR path exposes **no exposure and no gain control** through standard UVC — the RGB
path does (`0x0024000e`: auto-exposure mode, priority, exposure time absolute), the IR path
does not. Anything that changes IR capture behaviour has to go through the Microsoft XU.

Independent confirmation of the GUID and the bit numbering: the kernel already lights up
`UVCM` (UVC MSXU metadata) on the IR metadata node, which is selector 0x09 / D8.

```bash
v4l2-ctl -d /dev/video3 --list-formats-meta      # [1]: 'UVCM' (UVC MSXU Metadata)
```

### What the firmware answers

`spikes/msxu/probe.py` — read-only, `UVCIOC_CTRL_QUERY` with GET requests only.

```text
unit 4, selector 0x06 (FACE_AUTHENTICATION)
  GET_LEN  = 9        GET_INFO = 0x03  [SET_CUR, GET_CUR]
  GET_MAX  : 01 03 03 00 ...   iface 0x03  flags 0x3  [DISABLED | ALT_FRAME_ILLUM]
  GET_DEF  : 01 03 01 00 ...   iface 0x03  flags 0x1  [DISABLED]
  GET_CUR  : 01 03 01 00 ...   iface 0x03  flags 0x1  [DISABLED]
```

**The camera advertises a dedicated face-authentication capture mode and Linux has never
set it.** Every recording in `corpus/` was made in `DISABLED` — the general-purpose IR
preview mode. Streaming interface `0x03` in the payload is the IR VideoStreaming interface,
matching the descriptor; `flags 0x3` is the exact combination the spec permits for a pin
that is *also* general-purpose (D0 alongside D1), which is a self-consistency check on the
decode.

Modes are from `KSPROPERTY_CAMERACONTROL_EXTENDED_FACEAUTH_MODE`: D0 `DISABLED`,
D1 `ALTERNATIVE_FRAME_ILLUMINATION`, D2 `BACKGROUND_SUBTRACTION`. This device offers D0 and
D1; it does not offer hardware background subtraction.

### Setting it changes nothing measurable

`spikes/msxu/set_mode.py alt` — `SET_CUR` is accepted, reads back `ALT_FRAME_ILLUM`, and
survives the file descriptor closing. Ambient floor, same room, same session:

```text
DISABLED (baseline)     20.407
ALT_FRAME_ILLUM         18.958   20.693   21.213   21.087
```

Session variance swallows the difference.

**Confirmed independently 2026-08-09**, in the bright room at roughly twice the ambient, and
recorded in **reverse order** so that any session drift would work *against* the hypothesis
rather than for it. 35 frames per reading, nobody in frame:

```text
                            floor   amplitude      SNR
ALT_FRAME_ILLUM (as left)  33.404       59.77    180.0
DISABLED                   39.206       60.47    238.1
ALT_FRAME_ILLUM (repeat)   38.872       59.61    116.8
```

The mode's own two readings differ by 16% — more than either differs from `DISABLED` — and
amplitude is flat across all three at 59.6–60.5. Two sessions, two ambient levels, opposite
orderings, same answer.

**The exposure hypothesis is refuted** — this camera already strobed in `DISABLED`, and its
face-auth mode does not shorten the integration time. The no-op is *coherent* rather than
firmware weirdness: this sensor self-strobes 24/7 ([Probe 3](#probe-3--per-frame-mean-luma-the-decisive-test),
four independent runs), so `DISABLED` already behaves as `ALTERNATIVE_FRAME_ILLUMINATION`
and the flag is bookkeeping that tells Windows' pipeline how to interpret frames, not a
switch that changes them. That also retroactively explains Q3: there was never exposure
magic attached to the flag to find. Whatever Windows Hello does in that bright room, it is not buying a lower
ambient floor from the firmware; it is doing something different with the same imagery.
Which is what sent B18's investigation back to the statistics, where the answer was.

The camera was returned to `DISABLED` (its own `GET_DEF`) afterwards. The mode does not
survive a USB replug in any case.

**Left unverified:** whether `ALT_FRAME_ILLUM` populates per-frame illumination tags in the
`UVCM` metadata node. If it does, that is hardware ground truth for ON/OFF phase in place of
`frame::classify` inferring it from luma — a robustness win against exactly the failure
B18 documented, but not a daylight win, and not urgent.

**Not probed on purpose:** the Sonix XU (unit 3). It advertises no controls on the IR path,
and interface 4 of this device is an undriven "Application Specific" interface that looks
like a firmware-update endpoint. Blind vendor `SET_CUR` sweeps are the one irreversible
risk in this area; if the MSXU path is ever exhausted, use `linux-enable-ir-emitter`'s
quirk database rather than guessing.

---

## Still to probe

- [ ] Behaviour in a fully dark room (does AGC push ON frames higher?)
- [x] **Behaviour in daylight (does ambient NIR lift the OFF frames?)** — **YES, and it
      broke the project's liveness statistics.** OFF frames go from ~0.004 to ~93, a factor
      of ~24,000, and the emitter's contribution falls from dominating to ~13% of the
      signal. Full write-up: [B18](11-bug-log.md#b18). This probe was on the list before
      the measurement arrived; the backlog was right and the measurement beat it to it.
- [x] **`v4l2-ctl --list-ctrls`** — for the record, and the record is short: this device
      exposes **no exposure or gain controls at all**, only `region_of_interest_*` and a
      read-only `privacy` bool. The AGC is entirely internal and opaque, so "just shorten
      the exposure" is not available without vendor UVC extension-unit writes — which is
      the thing [Q3](08-open-questions.md#q3--ir-emitter--resolved-no-emitter-code-at-all)
      deleted from the project, along with the USB-bus-hang hazard.
- [x] Whether the OFF frames stay near-black with an IR remote pointed at the sensor
      (validates the floodlight check) — **done, Probe 5. Check verified; found
      [B9](11-bug-log.md#b9) in the process.**
- [ ] A **continuous** IR illuminator, not a pulsed remote — the easier case, but
      currently assumed rather than measured
- [ ] An IR source **synchronised to the strobe phase** — the hard case B9 does not
      cover. Needs a controllable emitter.
- [ ] `v4l2-ctl --list-ctrls` — for the record, not because we intend to write any
- [ ] Does the strobe phase (which of the pair comes first) survive a `STREAMOFF` /
      `STREAMON` cycle, or must ON/OFF be re-classified by luma on every capture?
      Assume the latter until measured.
- [ ] `by-id` behaviour on non-Sonix hardware (see above)

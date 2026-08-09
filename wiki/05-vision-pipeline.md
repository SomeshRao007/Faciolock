# 05 — Vision Pipeline

## Stages

```
V4L2 ─→ cheap liveness ─→ detect ─→ ROI liveness ─→ align ─→ embed ─→ match ─→ verdict
frames  dark-frame,        YuNet    falloff,        5-point   SFace/    cosine
        whole-frame D               motion          landmarks EdgeFace  vs template
        (no box needed)             (needs the box)
```

**Nothing is embedded or matched until liveness has passed.** That is the property that
matters, and it is what "liveness first" was always protecting.

Detection sits in the middle because it has to. The falloff test is defined below as
`D` *restricted to the detected face region* — it cannot run without a box, so it cannot
run before the detector. An earlier version of this page put all of liveness ahead of
detection and was inconsistent with its own definition of the test
([B8](11-bug-log.md#b8)).

The checks that need no box still run first, as an early-out: dark-frame integrity and
whole-frame response reject a screen or a floodlight for a few `f32` reductions without
paying YuNet's 5.6 ms. Detection is only reached by something already behaving as though
our emitter is lighting it.

### Aggregation: liveness is conjunctive, matching is k-of-n

Opposite rules over the same burst. **They must not be unified.**

| | rule | why |
|---|---|---|
| **Liveness** | **every** pair must pass | one contaminated frame is evidence of an attack, and evidence does not average out |
| **Matching** | k of n pairs must agree | one bad frame is a blink or a blur, and flukes *should* average out |

Measured consequence of getting this wrong: a pulsed IR source passed the dark-frame
check on **16.7% of individual pairs**, because the emitter idles between packets. With
the 2-pair capture budget and a per-pair or voting rule, that is a **1-in-36 bypass**.
With conjunctive aggregation it rejects cleanly, with four orders of magnitude of
margin. Data: [B9](11-bug-log.md#b9).

**A liveness verdict is a property of the burst, not of a frame.**

## Camera & IR

### Getting frames

`v4l` crate (`libv4l-rs`, raymanfx). Use the raw `v4l2` backend rather than `libv4l`
so we're not pulling in a userspace format-conversion library. MMAP buffer streaming
is zero-copy — buffers are mapped, not copied or allocated, which matters for the
memory budget. Formats to expect from IR cameras: `GREY` (8-bit NIR, the good case),
`YUYV`, sometimes MJPEG.

MJPEG is worth calling out: it means a JPEG decoder in the daemon, parsing
attacker-influenceable data. Prefer `GREY`/`YUYV` and treat MJPEG-only cameras as a
lower-priority path — another reason the daemon is sandboxed.

### Finding the IR camera

IR cameras on Windows Hello laptops bind to `uvcvideo` and appear as an additional
`/dev/videoN` alongside the RGB one. There is no reliable flag that says "this is the
IR one". Heuristics, in order:

0. **Filter to real capture devices first.** `V4L2_CAP_VIDEO_CAPTURE` must be set.
   On the reference machine two of the four `/dev/videoN` nodes are UVC payload-header
   *metadata* nodes (`UVCH`) that expose no formats and can never produce a frame
   ([10/Probe 4](10-hardware-probe.md#probe-4--device-node-disambiguation)). Skipping
   this step sends the chroma-variance fallback below at a node that will never answer.
1. `GREY` pixel format support — RGB webcams rarely offer it.
2. Device name containing `IR`, `Infrared`, `NIR`.
3. Resolution: IR sensors are typically 340×340 / 400×400 / 640×360, not 1280×720.
4. Last resort: capture one frame from each candidate and check for near-zero
   chroma variance.

On the reference machine steps 1 and 2 both point at the same node and the format list
is decisive on its own — `GREY` is the *only* format that device offers.

### Persisting the choice — **`by-path`, not `by-id`**

`videoN` numbering is not stable across boots, so the resolved device is written to
config by a one-time `faciolockctl diagnose` probe. **The stable name to persist is the
`by-path` symlink.**

`by-id` was the original choice and it is wrong. Both cameras on the reference machine
are interfaces of one USB device reporting a single serial, so udev generates a
colliding `by-id` basename and the last writer wins. The result is two symlinks for
four nodes, **neither of which resolves to the IR camera** — the surviving
`-video-index0` points at the RGB camera. Full evidence:
[10/Probe 4](10-hardware-probe.md#finding-1--by-id-cannot-address-the-ir-camera-on-this-hardware).

An unstable name that fails loudly is recoverable. A stable name that silently resolves
to the wrong camera is not: it would hand the RGB sensor to a pipeline that believes it
is holding IR, and every liveness check in this document assumes IR. `by-path` is
unambiguous — the USB interface number separates the two cameras.

Cost: `by-path` embeds the PCI address, so it is machine-specific and cannot be shipped
as a default. It has to come from the per-machine probe, which is where it was coming
from anyway.

**Regardless of the stored path, the daemon re-verifies the device reports `GREY` at
the expected geometry before trusting it.** Hardware changes, kernels renumber, and the
cost of the check is one ioctl against the failure mode of authenticating off the wrong
sensor.

### The emitter — **RESOLVED: we do not control it**

Many laptops don't turn the IR emitter on when the device is merely opened; it needs
a vendor-specific UVC extension-unit control. `linux-enable-ir-emitter` solves exactly
this by brute-forcing UVC controls until the emitter lights, and has a database of
known-good configurations. Howdy punts to that tool entirely
([02/W4b](02-howdy-teardown.md#w4b--howdy-does-not-control-the-ir-emitter-at-all)).

**We write no emitter code at all.** Measurement on the reference machine
(see below) showed the hardware already strobes the emitter per frame. Where that
holds, we get everything we need by *reading* frames. Where it doesn't, we degrade to
passive checks. Either way we never write a UVC control, which deletes the most
hardware-specific and most dangerous part of the project — including the USB-bus-hang
hazard on motion-reactive ASUS/Sonix hardware.

Emitter *setup* on machines where the LEDs don't come on at all remains the user's
problem, solved by `linux-enable-ir-emitter` as a one-time step. We ship it as a
`Suggests:`, not a dependency.

## Liveness — the differentiating piece

### Physics

Under ~850 nm NIR illumination:

- **Live skin** is a diffuse, subsurface-scattering, three-dimensional reflector. It
  lights up strongly and unevenly when the emitter fires, with intensity falling off
  by distance and surface angle across the face.
- **An LCD/OLED screen** emits essentially nothing in NIR and its glass is largely
  specular. A phone showing a perfect photo of you is, to an IR sensor, a dark
  rectangle with a bright specular blob.
- **A printed photo** is flat. It reflects NIR fairly uniformly — no depth-dependent
  falloff, no subsurface glow. Paper and skin also have visibly different NIR
  reflectance ratios.

### Measured: the hardware strobes for us

Reference machine: ASUS Vivobook, `/dev/video2`, `Y800` greyscale, 640×360 @ 30 fps.
60 consecutive frames, mean luma per frame (`ffmpeg … signalstats`). **Exact commands
and full raw output: [10 — Hardware Probe Log](10-hardware-probe.md).**

```
22.70  23.41  14.35  31.42        ← AGC converging, discard
 0.001 59.91   0.004 61.92
 0.004 61.06   0.004 60.37
 0.004 59.86   0.004 59.33
 …                                ← 27 clean alternating pairs
 0.004 54.10   0.005 53.74
```

The camera alternates emitter-ON and emitter-OFF frames at 30 fps — 15 usable
ON/OFF pairs per second — with no input from us. This is almost certainly the
hardware serving Windows Hello's own liveness path.

Two details that matter:

- **The OFF frames read `0.004`, not `0.000`.** Nonzero means genuine sensor read
  noise, i.e. a real exposure with the emitter dark — not a dropped or synthetic
  frame. It is a true dark reference.
- **The ON frames decay 61.9 → 53.7 over ~2 s.** That's AGC/LED droop.
  **Never threshold on absolute brightness.** Only the ON/OFF ratio and the spatial
  falloff pattern are stable. This is also an argument for pre-warm: let AGC settle
  before authentication instead of during it.

### The strobe difference test

Given an ON/OFF pair:

0. **Dark-frame integrity**: the OFF frame must be dark *relative to the ON frame of
   the same pair* — threshold on `mean(OFF)/mean(ON)`, never on an absolute value of
   `mean(OFF)`. If the ratio is too high, someone is flooding the scene with external
   IR to wash out the difference test. Reject.

   The relative formulation is not pedantry. The OFF floor measured **0.004 in one
   session and 0.0005 in another on the same machine** — an order of magnitude, from
   ambient light alone ([10/Probe 3b](10-hardware-probe.md#probe-3b--repeat-run-24-hours-later)).
   A fixed floor calibrated in a dark room would reject every honest attempt in
   daylight, and one calibrated in daylight would accept a floodlight attack at night.
   The ON/OFF ratio stayed within the same order of magnitude across both runs; it is
   the only quantity here stable enough to threshold.

   *(This check exists only because the hardware gives us true dark frames — it was
   not available in the original design.)*
1. `D = mean(ON) - mean(OFF)`, restricted to the detected face region.
2. **Response test**: a live face produces a large positive `D`.

   **This was predicted to reject a phone or laptop replay outright, and measurement
   refuted it** ([B17](11-bug-log.md#b17)). A phone at maximum brightness showing a photo
   reads `D = 36.2`, against a live minimum of 48.6 — not the ~0 the physics suggests. The
   screen really is dark at 850 nm; the problem is that the pre-box `D` is computed over
   the **whole frame**, and the frame is mostly room. The emitter lights the wall behind the
   phone perfectly well.

   `null_empty_room` had been saying so all along at 55.5 — higher than several live
   captures. A statistic that cannot tell a face from a wall was never going to tell a face
   from a phone in front of a wall.

   The phone attack *is* rejected, but by detection (`NoFace`), not by this check. The
   version of the response test that could reject a screen is the one restricted to the
   detected face region — which only exists when detection succeeds, i.e. exactly the case
   still unrecorded. `response_min` stays advisory until then.
3. **Falloff test**: on a live face, `D` varies smoothly across the region — nose and
   forehead brighten more than cheeks and the sides. On a flat print, `D` is close to
   uniform. This is a crude depth estimate obtained for free from two frames, and it
   is the check that rejects printed photos, which *do* pass the response test.
4. **Motion consistency**: the OFF frames should be near-identical over ~100 ms. A
   video replay on a screen shows frame-rate beating and compression artifacts.

   **The statistic is not the one originally written here.** [B9](11-bug-log.md#b9)
   measured the coefficient of variation of `mean(OFF)` across a burst and it does not
   separate at all — 0.54 for a pulsed attack against 0.33–0.65 for live captures, because
   a live OFF frame is pure sensor read noise and read noise is relatively noisy however
   tiny it is absolutely. What is implemented instead is a **per-pixel mean absolute
   difference between consecutive OFF frames over the face box**, normalised by the pair's
   response. That compares the images rather than their averages, which is a genuinely
   different quantity.

   It reads **0.0000 on every live capture in the corpus**, because a true dark frame on
   this hardware really is black and two black frames really are identical. That is the
   designed behaviour, and it is also why the check has demonstrated no discriminating
   power yet: everything it could catch has an OFF frame that is *not* black, and the only
   such recording we have is the floodlight, which the dark-frame check already rejects
   outright. It needs `spoof_phone` before it is worth anything.

Threshold on the **ratio** `mean(ON)/mean(OFF)` and on the *shape* of the falloff,
never on absolute values — see the AGC droop measured above.

Cost: 4 raw frames = 2 ON/OFF pairs ≈ 130 ms at 30 fps, and a handful of `f32`
reductions. No neural network, no model file, no license question, no writes to the
device, ~50 lines of arithmetic. Compare to MiniFASNet-class PAD networks, which are
~2 MB, need their own inference pass, are trained mostly on RGB, and whose weights
have their own licensing questions.

Note the usable frame rate for *recognition* is 15 fps, not 30 — only the ON frames
carry an image. Budget accordingly.

### Expected attack outcomes

| Presented | ON frame | OFF frame | Rejected by | Status |
|---|---|---|---|---|
| Live face | bright, uneven falloff | black | — (passes) | **verified** — 210 pairs, 0 false rejects; falloff 0.266–0.313 over the real box |
| Empty room | bright | black | **detection**, not liveness | **verified** — `NoFace`. Note it passes every liveness check: the emitter lights a wall perfectly well |
| Phone / laptop screen | ~black | black | response test (`D≈0`) | untested — no sample yet |
| Printed photo | bright, **flat** | black | falloff test | **untested, and the one that matters.** Three recording sessions have not produced a reading that settles it — log in [12](12-falloff-calibration.md) |
| Perfectly flat presentation | bright, uniform | black | flatness floor (0.05) | **verified on synthetic data.** This is *all* the falloff check currently claims |
| Video replay on screen | ~black | black | response test | untested |
| External IR floodlight, continuous | bright | **bright** | dark-frame integrity | **verified** |
| External IR floodlight, **pulsed** | bright | bright *in bursts* | dark-frame integrity, **conjunctive only** | **verified** — [B9](11-bug-log.md#b9) |
| 3D mask, actively IR-lit | bright, uneven | varies | **not reliably rejected** — see below | — |

The pulsed-floodlight row was not in the original threat model and was found by
recording the attack rather than reasoning about it. A per-pair rule let it through
16.7% of the time. This is the argument for the corpus existing at all.

### Capability tiers

The reference machine self-strobes. Other hardware may not, so the tier is **probed
at enrol time by observation only** — capture 30 frames, test for alternation. Zero
writes, zero risk. This replaces the dangerous brute-force probe entirely.

| Tier | Hardware | Liveness available |
|---|---|---|
| `ir-strobe` | self-strobing IR (reference machine) | full — all checks above |
| `ir-constant` | IR cam, emitter steady | passive only: NIR reflectance, flatness, specular-blob |
| `rgb` | no IR camera | none worth the name — **explicit opt-in, warns loudly** |

Same pipeline in all three; the tier selects which checks run and which threshold
applies. It is a config enum, not a second implementation.

`rgb` must require an explicit acknowledgement flag. It is Howdy's security level and
must be labelled as such, never offered as an equal choice.

**This is the security claim of the project and it must be tested, not asserted.**
The corpus in [04](04-architecture.md#the-faciolock-vision-boundary) is how: recorded
strobe bursts of real faces, phone screens, laptop screens, matte prints, glossy
prints, prints on IR-transparent media, and a mask, all replayed through the pipeline
in CI with a pass/fail threshold.

**Known weakness, to be honest about**: a purpose-built attack that presents an
actively NIR-illuminated 3D mask can produce both a response and a falloff. That is a
far higher bar than a printed photo, and it is the bar Windows Hello sits at too.

Fallback on RGB-only hardware: no strobe available. We fall back to a passive PAD
model or to nothing, and we tell the user their configuration is weaker. Deciding
which is in [08](08-open-questions.md).

## Detection

**YuNet** (`face_detection_yunet_*.onnx`, OpenCV Zoo, **MIT**).

- ~85 KB. Yes, kilobytes.
- ~5.6 ms on CPU.
- WIDER Face val: 0.8844 / 0.8656 / 0.7503 (easy/medium/hard).
- Outputs a box **plus the 5 landmarks** we need for alignment — one model, not two.
- The `2026may` variant takes dynamic input shapes, so we can feed native IR
  resolution without a resize.

Hard to beat on any axis for this use case. The "hard" subset score is irrelevant to
us — we need one large, near, frontal face, which is the easy case by construction.

### Measured on our own IR corpus (Phase 1)

**It works on near-infrared**, which was an assumption every later stage inherited and
which nothing had checked. Four live captures detect at 0.549–0.905, an empty room detects
nothing, and the floodlight attack yields only a 0.078-scoring 553×552 box — noise.

Three things the measurement settled that reasoning had got wrong:

- **Input must be square and it must be 320.** YuNet's anchor arithmetic does not survive
  H≠W under either runtime ([B11](11-bug-log.md#b11)), so the 640×360 frame is letterboxed.
  The *size* was then chosen from what it costs the embedding, not from the latency table:
  192 costs 0.82 embedding cosine against a high-resolution reference, 320 costs 0.94.
  [B13](11-bug-log.md#b13).
- **Score threshold 0.3, not OpenCV's 0.9.** An angled IR face scores 0.549.
  [B14](11-bug-log.md#b14).
- **One detection per burst**, its landmarks shared across the frames that get embedded.
  Sharing costs whatever the face moved in ~66 ms (2 px ≈ 0.98 cosine); detecting per frame
  cheaply enough to afford three passes costs far more. k-of-n keeps its independence
  because three different *images* are still embedded — only the alignment is shared.

The decoder consumes 12 output tensors (`cls`/`obj`/`bbox`/`kps` at strides 8/16/32) and
**verifies them by name at load time** rather than trusting graph order. Decoding `obj_8`
as `cls_8` would still produce boxes, just worse ones, for no visible reason.

## Alignment

Similarity transform from the 5 landmarks to the canonical 112×112 ArcFace reference
points. Standard, deterministic, ~20 lines. **Must be bit-identical between enrolment
and verification** or the embeddings silently drift and accuracy collapses — this is
the classic bug in every reimplementation of this pipeline, so it gets a golden-output
regression test.

## Embedding — **DECIDED: SFace** (EdgeFace reversed 2026-08-04)

**The first risk on the list below turned out to be real.** EdgeFace's trained weights
are distributed by their authors under **CC BY-NC-SA 4.0** — NonCommercial — even though
the GitHub code repo is BSD-3. That fails DFSG §6 and is a hard blocker for
[G7](01-goals-and-scope.md). Full analysis: [B10](11-bug-log.md#b10).

**We build against SFace** (Apache 2.0, OpenCV Zoo, ~37 MB, 128-d). Measured at
**85 ms** under `tract` at 112×112, which keeps the warm path at ~318 ms — inside the
400 ms p95 target, with no headroom to spare.

The embedder trait stays. It is what turned this reversal into a config change instead
of a rewrite, and the remaining candidate list is short enough that the next reversal is
plausible: if SFace's residual licence risk (upstream repo states no licence, training
data unstated) does not clear before an ITP, the fallback is dlib's
`dlib_face_recognition_resnet_model_v1` at **CC0**.

The original risk list, kept because it is the reason the trait exists:

- ~~its LICENSE turns out not to cover the trained weights~~ — **this happened**
- `tract` can't load it — did not happen; `tract` loads SFace and YuNet both
- ~~it underperforms on our own IR corpus~~ — **measured in Phase 1, and it does not.**
  Genuine cross-capture cosine for one subject across lighting, angle and glasses:

  ```text
                        dark    angle_left   frontal   glasses
  live_dark_frontal   1.0000        0.6739    0.9003    0.7220
  live_lit_angle_left 0.6739        1.0000    0.6689    0.6003
  live_lit_frontal    0.9003        0.6689    1.0000    0.7987
  live_lit_glasses    0.7220        0.6003    0.7987    1.0000
  ```

  Worst genuine pair **0.6003**, threshold 0.363 — clears it by 1.65×. 4/4 live captures
  matched a template enrolled from a different recording. An RGB-trained embedder survives
  contact with 8-bit NIR.

  **This is the false-reject side only.** The corpus has one subject, so FAR cannot be
  measured from it at all, and the 0.363 threshold remains OpenCV's RGB calibration —
  paper-derived and provisional. wiki/05 requires the default be "chosen from measurement
  on our own corpus" and it currently is not. A second consenting subject is what closes
  that, and nothing else does.

Comparison retained:

| | **SFace** | **EdgeFace-XS (γ=0.6)** |
|---|---|---|
| Source | OpenCV Zoo | Idiap Research Institute |
| License | **Apache 2.0** | **BSD 3-Clause** (repo; weights need confirming) |
| Size | ~37 MB fp32, int8 available | 1.77 M params (~7 MB fp32) |
| Compute | ~65 ms CPU @ 150×150 | 154 MFLOPs — roughly an order of magnitude less |
| LFW | 99.40% | 99.73% |
| CFP-FP | — | 94.71% |
| Embedding | 128-d | 512-d |
| Maturity | shipped in OpenCV, widely deployed | research release, less field-tested |
| Risk | none — license is unambiguous | must verify the LICENSE covers the `.pt`/ONNX weights, and export to ONNX ourselves |

EdgeFace looked better on every technical axis, which is why it was picked. It is
unusable anyway, because technical merit does not survive a licence that Debian cannot
accept.

**Ruled out**: InsightFace `buffalo_l` / ArcFace R100 and everything else from
InsightFace's model zoo. The code is MIT but the **pretrained weights are
non-commercial research use only**. That is a hard blocker for [G7](01-goals-and-scope.md)
— Debian cannot ship them, and neither can we.

**Also ruled out, for exactly the same reason**: EdgeFace, all variants. BSD-3 code,
CC BY-NC-SA 4.0 weights, trained on WebFace260M (research-only).

### The check every model must now pass before it enters `models/`

Two models have now been ruled out by the same failure mode, the second *after* it had
already been adopted as a decision. A repo-root LICENSE is not evidence about weights.

1. **Licence at the weight distribution point** — the HuggingFace model card, release
   asset, or per-model `LICENSE` beside the file. Not the source repo root.
2. **Training-dataset terms.** WebFace260M, MS-Celeb-1M and CASIA-WebFace are all
   research-only, and weights trained on them usually inherit the restriction.
3. **Did the redistributor have the right to grant what they granted?**

SFace passes (1) — OpenCV Zoo ships a per-model Apache 2.0 `LICENSE` next to the ONNX —
and (3) is credible. **(2) is not yet cleared** and must be before an ITP.

**Also available**: dlib's `dlib_face_recognition_resnet_model_v1` is **CC0 / public
domain** (this is what Howdy uses, and it's why Howdy has no license problem). It's
a 2017 ResNet-29 at 99.38% LFW — a clean fallback, but both candidates above beat it
on accuracy and compute.

## Inference runtime

| | `ort` (ONNX Runtime bindings) | `tract` (pure Rust) | `candle` |
|---|---|---|---|
| Native deps | libonnxruntime (C++, ~15 MB) | none | none |
| CPU speed | fastest; 3–5× Python, well-optimised kernels | slower, but adequate at our model sizes | measurably slower than ORT on small CPU models |
| Packaging in Debian | onnxruntime is awkward to package/vendor | trivial — it's just a crate | trivial |
| `MemoryDenyWriteExecute` | may conflict with JIT/codegen | fine | fine |
| ONNX operator coverage | complete | good, but must verify YuNet + chosen embedder load | needs conversion |

For a component that wants to be in the archive and wants an aggressive seccomp
profile, `tract` is attractive despite being slower — at 154 MFLOPs (EdgeFace) the
absolute numbers stay small. But "verify the models actually load in tract" is a
gating spike before this can be decided. Also in [08](08-open-questions.md).

## Matching

- Cosine similarity against the enrolled template set.
- **Multiple templates per user** (glasses on/off, different lighting), matched
  best-of. Requiring one canonical face is the main source of day-to-day FRR.
- Threshold is a security parameter with a real FAR/FRR curve behind it. It goes in
  config with a conservative default, and the default is chosen from measurement on
  our own corpus, not copied from a paper.
- **Require k consecutive agreeing frames** (e.g. 2 of 3) before returning success.
  Nearly free given we already captured a burst for liveness, and it removes
  single-frame flukes.
- Constant-time comparison is not meaningful for a float distance; the mitigation
  against threshold-probing is the rate limiter in [03](03-pam-and-security.md#r5--rate-limit-in-the-daemon-per-user-persistently),
  not timing hygiene.

## Model integrity

Models ship as files on disk that root can replace. `SHA256SUMS` is verified at
daemon startup and the daemon refuses to run on mismatch. This doesn't stop root —
nothing stops root — but it catches corruption and makes tampering noisy in the
journal.

## Sources

- [OpenCV Zoo — YuNet](https://github.com/opencv/opencv_zoo/tree/main/models/face_detection_yunet) (MIT)
- [OpenCV Zoo — SFace](https://github.com/opencv/opencv_zoo/tree/main/models/face_recognition_sface) (Apache 2.0)
- [EdgeFace](https://github.com/otroshi/edgeface) (BSD 3-Clause)
- [InsightFace](https://github.com/deepinsight/insightface) — code MIT, **weights non-commercial**
- [dlib-models](https://github.com/davisking/dlib-models) — face recognition ResNet, CC0
- [libv4l-rs](https://github.com/raymanfx/libv4l-rs)
- [linux-enable-ir-emitter](https://github.com/EmixamPP/linux-enable-ir-emitter)

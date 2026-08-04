# 11 — Bug Log

Bugs hit and fixed, newest first. Kept because several of these are **design** bugs
found by measurement, not typos — the reason they were caught at all is that Phase 0
exists, and the reason they are written down is so the same class gets caught again.

Severity is about what the bug would have cost if it had shipped, not how hard it was
to fix.

| # | Bug | Severity | Status |
|---|---|---|---|
| [B17](#b17) | Response test does **not** reject a screen — the statistic is whole-frame | **high** | design finding, open |
| [B16](#b16) | Alignment sampled at a **half-pixel offset** from OpenCV's convention | **high** | fixed; found only by an external golden |
| [B15](#b15) | Latency benchmark ran inside the CPU's boost window and reported **half** the real cost | medium | corrected; warm-up + 50 samples |
| [B14](#b14) | Detection threshold copied from OpenCV rejected an **angled live face** | medium | fixed, set from the corpus |
| [B13](#b13) | Detector input size chosen from a latency table, never from **accuracy** | **high** | fixed, 192 → 320 |
| [B12](#b12) | Detected on an emitter-**OFF** frame — found zero faces in every capture | medium | fixed, phase measured not assumed |
| [B11](#b11) | Benchmarked `tract` against a published figure from **another machine** | medium | corrected by measuring both on one box |
| [B10](#b10) | EdgeFace **weights** are CC BY-NC-SA, not BSD-3 — DFSG-incompatible | **critical** | Q1 reversed to SFace |
| [B9](#b9) | Dark-frame check defeated by a *pulsed* IR source at per-pair granularity | **critical** | fixed by design change |
| [B8](#b8) | Liveness cannot run before detection, but the pipeline says it does | **high** | wiki corrected |
| [B7](#b7) | Centre-crop ROI does not cover the face — falloff test not evaluable | **high** | **CLOSED** — real face box, Phase 1 |
| [B6](#b6) | `by-id` symlink resolves to the *wrong camera* | **high** | fixed |
| [B5](#b5) | ffmpeg silently duplicates frames when writing to disk | **high** | fixed |
| [B4](#b4) | Capture file with no sidecar joins the corpus undocumented | medium | warns |
| [B3](#b3) | `2>/dev/null` hid a missing binary as "no evidence" | medium | documented |
| [B2](#b2) | Half the `/dev/videoN` nodes are not cameras | low | fixed |
| [B1](#b1) | Dead conditional in `build_pairs` | trivial | removed |

---

## B17 — The response test does not reject a screen {#b17}

**Severity: high**, and it is a *design* finding rather than a coding defect. Recorded the
first time a screen was ever put in front of the camera.

### What the design predicted

[05](05-vision-pipeline.md#the-strobe-difference-test):

> **Response test**: a live face produces a large positive `D`. A screen produces
> ~0 — LCD/OLED emit essentially nothing in NIR. Fails a phone/laptop replay outright.

### What `spoof_phone` measured

A phone at maximum brightness showing a frontal photo of the operator, ~25 cm, recorded
with a same-session `live_lit_frontal` control:

```
class       n     D min      D max
live        5    48.576     75.482
spoof       1    36.173     36.173
attack      1    37.288     37.288
null        1    55.532     55.532
```

**`D` is 36.2, not ~0.** The prediction is wrong, and the reason is that the pre-box
response test is computed over the **whole frame**, which is mostly room. The emitter lights
the wall, the desk and the operator's hands behind the phone perfectly well; the phone
contributes almost nothing and is a small part of the frame. `D` is therefore measuring the
*background*, not the presented object.

The `null_empty_room` row has been saying this all along and it was read as a curiosity
rather than a warning: an empty room scores 55.5, higher than several live captures. A
statistic that cannot distinguish a face from a wall was never going to distinguish a face
from a phone in front of a wall.

### Was the attack rejected?

**Yes — by detection, not by liveness.** End-to-end the verdict is `NoFace`: YuNet finds
nothing in a phone screen under NIR, because the screen is genuinely dark at 850 nm. The
pipeline denies it, and there is now a regression assertion that it must continue to.

But the rejection comes from a stage that was never designed to be a liveness defence, and
that matters for what happens next:

- The residual gap that *does* exist — 36.2 against a live minimum of 48.6 — is **1.34×**.
  Against the dark-frame check's 65×/130× margins that is nothing, and it is one sample of
  one phone in one room. It cannot carry an enforced threshold.
- The untested case is a screen bright enough in NIR for YuNet to find a face in it. A
  larger, brighter laptop panel is the obvious candidate. Detection would then succeed and
  the response check over the **face box** — not the whole frame — becomes the defence.
  That measurement does not exist.

### Consequences

1. `response_min` stays **advisory**. The corpus does not support enforcing it, and the
   number that would separate this one sample has 1.34× of margin.
2. **The response test needs restating in [05](05-vision-pipeline.md).** As specified it
   is a whole-frame early-out, and a whole-frame early-out cannot reject a screen. The
   version that could is the one restricted to the detected face region, which only exists
   when detection succeeds — i.e. exactly the case not yet recorded.
3. `spoof_laptop` moves from nice-to-have to **the next recording that matters**, alongside
   the print.

### Lesson

**A check that has never seen its negative class is a hypothesis, not a defence.** The
response test read as the easy one — screens are dark in NIR, everybody knows that — and it
was the falloff test that carried all the doubt. One three-second recording moved the
response test from "obviously fine" to "does not work as specified", and it cost nothing.
The checks nobody is worried about are the ones with no evidence behind them.

---

## B16 — Alignment sampled half a pixel away from ArcFace's convention {#b16}

**Severity: high.** The silent-drift bug [05](05-vision-pipeline.md#alignment) names as
"the classic bug in every reimplementation of this pipeline", found in our own
reimplementation, exactly where it said to look.

### What happened

`align()` warped the face into the canonical 112×112 chip by inverse mapping, sampling the
source at

```rust
let (sx, sy) = inverse.apply(dx as f32 + 0.5, dy as f32 + 0.5);
sample_bilinear(frame, sx - 0.5, sy - 0.5);
```

i.e. treating destination pixels as having centres at half-integer coordinates. That is
`cv2.resize`'s convention, and it is what `detect`'s letterbox correctly uses.

**`cv2.warpAffine` does not use it.** It maps destination pixel `(dx, dy)` straight through
the inverse matrix with no offset. And `warpAffine` is what produces canonical ArcFace
chips — insightface's `norm_crop` calls it — so it is the convention every embedder of this
family was *trained* against.

At an inverse scale of ~1.42 the mismatch is a systematic shift of

```
0.5 * 1.42 - 0.5  =  ~0.21 px
```

applied to every sampled point of every chip.

### Why nothing else caught it

Everything self-consistent passed. The transform estimator agreed with OpenCV's to 1e-4.
The sum of squared residuals matched to 3e-5. Alignment was byte-deterministic, the inverse
round-tripped, a known transform was recovered exactly, and the identity was the identity.
Cross-capture matching worked — 4/4 live captures matched, worst genuine similarity 0.60.

**A half-pixel offset applied uniformly is invisible to every test that compares our output
to our own output.** Enrolment and verification both shifted by the same 0.21 px, so they
agreed with each other perfectly. The only thing it degraded was agreement with the space
SFace was trained in, and no internal test can see that.

The golden comparison saw it immediately:

```
                        mean |diff|   max   within 2 levels
before (half-pixel)           3.196    87              7.6%
after  (OpenCV convention)    0.000     1            100.0%
```

Note the first attempt at this comparison used a `(x*7 + y*13) % 256` source pattern, which
wraps; a sub-pixel difference beside a wrap boundary swings ~250 levels, giving mean 7.46
and masking the signal in noise. Switching to a 16×24 checkerboard — mostly flat interiors,
high contrast only at block edges — made the residual legible. **The test data has to be
chosen so the defect is louder than the tolerance.**

### Consequences

- Fixed: `align` samples at integer destination coordinates.
- The two conventions now **differ between modules on purpose** — `detect`'s letterbox
  offsets by half a pixel because `cv2.resize` does, `align` does not because
  `cv2.warpAffine` does not. Matching each OpenCV function individually is what matters;
  being internally uniform would be wrong.
- Measured effect on matching was small (worst genuine similarity 0.6003 → 0.5921, within
  noise) — but "small on four captures of one subject" is not evidence it is small in
  general, and it was free to fix.

### Lesson

**A golden generated from your own output only catches drift. It cannot catch being wrong
from the first commit, and it will happily freeze the wrong answer forever.** The reference
has to come from somewhere else. This one cost one `apt install`, one throwaway venv and
about forty lines of Python, and it found a defect that four independent self-consistent
tests could not.

Corollary, from the tolerance: the assertion that eventually caught this was nearly written
loose enough to pass. A mean difference of 3.2 was assumed to be interpolation rounding
because interpolation rounding was the expected answer. **A tolerance wide enough to
swallow a real defect is not a tolerance** — when a residual is larger than it should be,
find out why before widening the bound.

---

## B15 — A benchmark that measured the boost clock {#b15}

**Severity: medium.** Would have put a comfortable, wrong number into
[06](06-performance-budget.md) and hidden that the budget has no headroom.

Phase 1's per-stage latency test used 12 iterations with no warm-up and reported **SFace
at 39 ms**, against the 85 ms already recorded from the Phase 0 spike. A 2.2× discrepancy
in the largest term of the budget, in the flattering direction.

Re-running the spike binary on the same machine in the same session:

```
SFace 112x112   load 199.0 ms   p50 76.19   p95 76.68   min/max 38.98 / 76.92
```

**Bimodal.** The minimum is 38.98 ms and the median is 76.19 ms, because the CPU boosts
for the first handful of inferences and then settles. A 12-sample median lands entirely
inside the boost window; the spike's 50 samples after 5 warm-up do not.

Fixed by 10 warm-up plus 50 measured iterations, which reproduces 76.9 ms and agrees with
the spike. The real correction to [06](06-performance-budget.md) was therefore
85 → 76.9 ms, not 85 → 39.

### Lesson

**A benchmark that does not outlast the boost clock is measuring the clock, not the code.**
Warm-up is not a formality and the sample count has to be large enough that the steady
state dominates the median. A `min` far below the `p50` is the tell, which is the argument
for reporting a distribution rather than a single number — the spike already did, and that
is what caught this.

Same family as [B11](#b11): a measurement was believed because it was convenient. The
control in both cases was running the other implementation on the same box.

---

## B14 — Detection threshold rejected a live face {#b14}

**Severity: medium.** A false reject on an ordinary head turn.

`DetectConfig::score_min` was set to 0.6 by analogy with OpenCV, whose own YuNet default is
0.9. Measured on the corpus at 192×192:

```
live_lit_frontal          0.905      live_lit_glasses          0.890
live_dark_frontal         0.905      live_lit_angle_left       0.549   <- rejected at 0.6
attack_floodlight_remote  0.078                                        <- noise, 553x552 box
null_empty_room           (none)
```

An angled IR face scores 0.549 — below the invented threshold and nowhere near OpenCV's
0.9. Set to **0.3**, which sits ~4× above the noise and ~1.8× below the weakest real face.

Detection is not a security boundary — a spurious box still has to clear liveness and
matching — so it is tuned for recall. One subject, four captures: provisional.

**Lesson:** a threshold from another project's README is calibrated for that project's
data. Ours is greyscale NIR at 640×360; theirs is not.

---

## B13 — Detector input size chosen without ever measuring accuracy {#b13}

**Severity: high.** Silently degraded every embedding, which is the exact failure mode
[05](05-vision-pipeline.md#alignment) warns about — no error, no log line, just a worse
system.

### What happened

`DetectConfig::input_size` was set to 192×192 because [06](06-performance-budget.md) had a
row saying YuNet costs 11 ms there. That is a latency argument, and input size is not only
a latency parameter: a 640×360 frame letterboxed to 192 puts a laptop-distance face at
~45 px, and its five landmarks then scale back up 3.3× into the alignment.

Landmark displacement against a 448×448 reference, full-frame px:

```
size   mean-to-max displacement   YuNet
 160          6.3 - 10.9 px        8.2 ms
 192          5.1 - 12.1 px       11.5 ms
 320          1.8 -  6.2 px       33.0 ms
```

A displacement is not yet a cost, so it was converted into one — embed the same frame from
each size's landmarks and take the cosine against the reference:

```
size   worst embedding cosine vs 448
 160                        0.8861
 192                        0.8235
 320                        0.9368
```

Read against a perturbation curve measured on the same frames — 1 px costs 0.993, 4 px
0.960, 8 px 0.911 — **running at 192 costs more than 8 px of landmark error.** Genuine
cross-capture similarity for the same person reads 0.60–0.90, so detector resolution alone
was eating a large share of the genuine/impostor margin.

Note also that 160 and 192 are **not ordered with respect to each other**. At that scale
the landmark regression is noisy rather than converging, which is a second reason not to
sit there.

### Consequences

- `input_size` default is **320**. Costs 22 ms.
- Detection reverts to **once per burst**, its landmarks shared across the embedded
  frames — which is what [06](06-performance-budget.md) always budgeted. Phase 1 had
  briefly detected per frame to avoid a correlated failure; the measurement says sharing
  landmarks across frames ~66 ms apart costs whatever the face moved in 66 ms, which at
  2 px is 0.98 and at 4 px is 0.96 — an order of magnitude cheaper than the resolution.
  k-of-n keeps its independence regardless, because three different *images* are still
  embedded and only the alignment is shared.

### Lesson

**A parameter that appears in the latency table is not therefore a latency parameter.**
The budget made input size look like a pure speed knob because that is the only column the
budget has. The question "what does this cost in accuracy" was never asked, and it had a
cheap answer available the whole time: perturb the input, measure the output.

---

## B12 — Detection ran on emitter-OFF frames {#b12}

**Severity: medium** as a defect, high as a repeat.

The first run of Phase 1's detection test found **zero faces in every capture**, which
reads like "YuNet does not work on IR" — a conclusion that would have invalidated the
model choice.

It was reading frame index 6. On the reference machine the frame sequence past AGC settle
is `0.001, 59.91, 0.004, 61.92, …` — index 6 is an emitter-**OFF** frame at 0.004 mean
luma. **It was detecting on black.**

Fixed by selecting the brightest frame in the window rather than an index. Every live
capture then detected at 0.549–0.905.

### Lesson

This is [B5](#b5) arriving from a different direction. B5's rule was written for capture:

> ON/OFF phase must be re-established by measuring luma on every capture, never by
> assuming frame `n` is ON because frame `n-1` was OFF.

The rule was obeyed in `frame.rs`, which classifies by luma, and then broken immediately
in the test harness that fed it. **A rule that lives in one module is not a rule.** Any
code that picks a frame — production, test, or throwaway — picks it by measurement.

Also worth recording: the symptom pointed at the wrong layer entirely. "No faces found"
looked like a model or decode problem and was an input-selection problem, and the thing
that distinguished them was that the *empty room* also found nothing, i.e. the result was
identical for a frame with a face and a frame without one.

---

## B11 — Cross-machine benchmark comparison {#b11}

**Severity: medium.** Wrong conclusion about the central Q2 trade-off, stated confidently,
from a measurement that could not support it.

### What happened

With only the `tract` half of the S1 spike run, the numbers were compared against the
"5.6 ms" YuNet figure in [06](06-performance-budget.md) — which comes from OpenCV Zoo's
published benchmark. Normalising per pixel gave:

```
OpenCV published   5.6 ms @ 160x120  =  0.29 us/px
tract measured     7.7 ms @ 160x160  =  0.30 us/px
```

and the conclusion recorded was *"tract is within ~5% of the C++ reference; the premise
that pure Rust costs a large constant factor does not hold."*

**The two numbers are from different computers.** OpenCV's benchmark hardware is not
this laptop, and its published figures are not dated to a CPU we have. Per-pixel
normalisation controls for input size; it does nothing about the machine. The comparison
was meaningless.

### What the same-machine measurement showed

Once `ort` ran on the same box with the same input buffer:

```
YuNet 160x160    tract 7.69 ms    ort 0.45 ms    17x
YuNet 192x192    tract 11.15 ms   ort 1.27 ms    8.8x
YuNet 320x320    tract 33.94 ms   ort 1.67 ms    20x
SFace 112x112    tract 85.25 ms   ort 30.85 ms   2.8x
```

Not 5%. **Between 3× and 20×.** The original "pure Rust is slower" premise in Q2 was
correct, and the analysis had talked itself out of it.

### Why it mattered

Q2 is the decision this spike exists to inform, and the erroneous finding pointed at
the answer the project was already leaning toward. A measurement that conveniently
confirms the existing preference is the one that most needs a control, and it did not
get one until the other half of the spike ran.

The decision does not actually flip — `tract` still clears the latency budget at
~318 ms and still wins on attack surface — but it was very nearly made on a number that
was not real.

### Lesson

**A benchmark is only a comparison when both sides run on the same machine.** Published
figures set expectations; they do not settle A-versus-B. If a spike exists to compare
two things, it is not finished until both have been measured under it — reporting half
of a comparison as though it were the whole is how this happened.

Related, from the same pass: "non-square input fails" was recorded as a `tract` defect
before `ort` was tested. `ort` fails identically — it is a limitation of the YuNet model.
Same root cause: one implementation was measured and its behaviour attributed to the
implementation rather than to what was shared.

---

## B10 — EdgeFace weights are non-commercial, not BSD-3 {#b10}

**Severity: critical.** Kills [Q1](08-open-questions.md#q1--embedding-model-sface-or-edgeface--decided-edgeface)'s
decision outright, and would have been a licence violation shipped in a Debian package.

### What happened

[08/Q1](08-open-questions.md) staged exactly this check:

> - [ ] Confirm EdgeFace's BSD-3 LICENSE covers the **trained weights**, not just code

The GitHub repo makes it look fine. `LICENSE` at the root is a standard, unmodified
BSD 3-Clause with no non-commercial carve-out, and the weights are committed *inside
that repo* at `checkpoints/edgeface_xs_gamma_06.pt`. `checkpoints/README.md` mentions no
separate terms. On that evidence the answer is "yes, BSD-3 covers the weights".

**That evidence is incomplete and the conclusion is wrong.** The authors also publish
the same weights on HuggingFace, and there they declare a different licence:

```
Idiap/EdgeFace-XS-GAMMA -> cc-by-nc-sa-4.0
Idiap/EdgeFace-Base     -> cc-by-nc-sa-4.0
Idiap/EdgeFace-S-GAMMA  -> cc-by-nc-sa-4.0
Idiap/EdgeFace-XXS      -> cc-by-nc-sa-4.0
```

Not an inference from a repo tag — it is in the model card's own frontmatter:

```yaml
---
license: cc-by-nc-sa-4.0
---
```

**CC BY-NC-SA 4.0 is NonCommercial.** That fails DFSG §6, "No Discrimination Against
Fields of Endeavor". Debian cannot ship it. Neither can we, given
[Q6](08-open-questions.md#q6--target-debian-archive-or-ppa-and-see--decided-archive)
is decided and binding.

### Why the two licences disagree

The model card states EdgeFace was **trained on WebFace260M**, whose own terms are
research-only. A permissive licence on training code and a restrictive one on the
resulting weights is the consistent, deliberate position — not a mistake. It is exactly
the InsightFace situation already ruled out in [05](05-vision-pipeline.md#embedding):
*code MIT, weights non-commercial.*

The project walked into the same trap a second time, with a different model.

### Consequences

- **Q1 reverses to SFace** (Apache 2.0, OpenCV Zoo). The embedder trait staged in
  [05](05-vision-pipeline.md#embedding) is what makes this a config change instead of a
  rewrite — that abstraction has now paid for itself.
- The latency budget gets worse: SFace measured **85 ms** under `tract` against
  EdgeFace's projected ~10–15 ms. Still inside the p95 target, but the headroom is gone.
  See [B11 note in Q2](08-open-questions.md#q2--inference-runtime-ort-tract-or-candle).
- 512-d embeddings become 128-d, and ~7 MB becomes ~37 MB.

### The lesson, which is the reusable part

**A licence file in a source repo does not license the weights inside it, and finding a
permissive LICENSE is not the end of the check — it is the beginning.** Weights are a
separate artifact with separate provenance, and the authoritative statement is wherever
the authors *distribute the weights*, which is frequently not the code repo.

The check that actually works, and is now mandatory before any model enters `models/`:

1. Licence at the **weight distribution point** (HuggingFace card, release asset,
   model zoo per-model `LICENSE`), not just the source repo root.
2. **Training-dataset terms.** WebFace260M, MS-Celeb-1M, and CASIA-WebFace are all
   research-only, and a model trained on them usually inherits a restriction.
3. Whether the redistributor had the right to grant what they granted.

SFace passes 1 (OpenCV Zoo ships a per-model Apache 2.0 `LICENSE` beside the ONNX) and
3 is credible. Point 2 carries a residual, documented risk: the upstream
[zhongyy/SFace](https://github.com/zhongyy/SFace) repo declares no licence at all, and
the training data is not stated in the zoo's model card. **This is not yet fully
cleared** and must be before an ITP.

dlib's `dlib_face_recognition_resnet_model_v1` remains the fallback-of-last-resort at
**CC0 / public domain** — unambiguous, and the reason Howdy has no licence problem. It
costs accuracy (99.38% LFW) and is a 2017 ResNet-29, but it is the only candidate whose
licensing is beyond argument.

---

## B9 — Dark-frame integrity check is defeated by a pulsed IR source {#b9}

**Severity: critical.** This is an authentication bypass.

### What happened

`attack_floodlight_remote` was recorded by holding an ordinary TV remote against the
lens — the external-IR-floodlight attack from
[05](05-vision-pipeline.md#the-strobe-difference-test). The check is meant to notice
that the OFF frame is no longer dark and reject.

Per-pair, it mostly does. But not always:

```
pair   mean(ON)  mean(OFF)      OFF/ON
   0    60.0472    0.00026     4.36e-6   <-- reads as perfectly clean
   1    82.9296    9.29104     1.12e-1   <-- caught
   2    88.8702    0.00016     1.77e-6   <-- reads as perfectly clean
   3    91.1450    8.56571     9.40e-2   <-- caught
   4    92.6711    0.00026     2.83e-6   <-- reads as perfectly clean
```

**7 of 42 pairs (16.7%) read as completely clean.** An IR remote does not emit
continuously — it sends a modulated packet and then idles before repeating. Any OFF
frame landing in a gap is a genuine dark frame, and the check has nothing to complain
about.

### Why it is critical

[05](05-vision-pipeline.md) budgets **4 frames = 2 ON/OFF pairs** per authentication.
At a 16.7% per-pair pass rate, an attacker whose source happens to be pulsed gets both
pairs clean **2.8% of the time — roughly 1 attempt in 36.**

For a factor that authenticates `sudo` and the lock screen, 1-in-36 is not a
mitigation, it is a door. And a deliberate attacker would pulse *on purpose*, because
they do not need to know our frame timing — they only need to be dark often enough to
get lucky, and they can retry.

### The fix that did not work

The obvious idea was the motion-consistency check already in the design: "the OFF
frames should be near-identical over ~100 ms". A pulsed source makes them wildly
inconsistent, so the coefficient of variation of `mean(OFF)` across the burst should
expose it. Measured:

```
attack_floodlight_remote   mean(OFF) avg=5.65887  CV=0.54
live_lit_frontal           mean(OFF) avg=0.00016  CV=0.42
live_lit_frontal           mean(OFF) avg=0.00014  CV=0.65
null_empty_room            mean(OFF) avg=0.00085  CV=0.33
```

**CV does not separate them at all** — 0.54 for the attack sits inside the live range
of 0.33–0.65. A live OFF frame is pure sensor read noise, and read noise is *relatively*
noisy even though it is absolutely tiny. Relative spread is the wrong statistic here.

### The fix that works

Aggregate the per-pair ratio **conjunctively**: reject if *any* pair in the burst shows
contamination, rather than thresholding a median or requiring k-of-n to pass.

```
RECORDING                          median          MAX   verdict@1e-3
attack_floodlight_remote         7.18e-02     1.30e-01         REJECT
live_dark_frontal                3.21e-06     5.36e-06          allow
live_lit_angle_left              1.88e-06     5.03e-06          allow
live_lit_frontal                 2.00e-06     3.76e-06          allow
live_lit_frontal                 1.45e-06     4.43e-06          allow
live_lit_glasses                 1.73e-06     3.88e-06          allow
null_empty_room                  1.00e-05     1.54e-05          allow
```

Worst live reading `1.54e-05`, attack `1.30e-01` — **four orders of magnitude apart**.
A threshold at `1e-3` sits ~65× above the worst honest capture and ~130× below the
attack. Zero false rejections across **210 live pairs**.

### Consequences, now binding

1. **Liveness aggregation is conjunctive; matching aggregation is k-of-n.** These are
   opposite rules and must not be confused. `05` already specifies "require k
   consecutive agreeing frames" — that is about *identity*, where the risk is a
   single-frame fluke. For *liveness* the risk runs the other way: one contaminated
   frame is proof of an attack, and the burst must fail as a whole. Any refactor that
   unifies them reintroduces this bug.
2. **A liveness check is a property of the burst, not of a frame.** Per-frame verdicts
   must never be reduced by voting.
3. The 2-pair capture budget in [06](06-performance-budget.md) was sized for latency,
   not for attack margin. Conjunctive aggregation is what makes 2 pairs safe; without
   it, the margin needed ~6 pairs and would have blown the latency budget.

### What is still unknown

The remote is a *pulsed* source with gaps we did not choose. A continuous IR
illuminator would be caught on every pair and is the easier case. An attacker who
synchronises their pulse to the sensor's strobe phase is a harder case and is **not**
tested by this corpus — it needs a controllable IR source. Queued in
[10](10-hardware-probe.md#still-to-probe).

---

## B8 — Liveness cannot run before detection, but the pipeline says it does {#b8}

**Severity: high.** Internal contradiction in the design, surfaced by trying to build it.

[05](05-vision-pipeline.md) opens with:

```
V4L2 capture ─→ IR liveness ─→ detect ─→ align ─→ embed ─→ match
```

> Liveness runs **first**, before recognition.

But the strobe-difference test in the same document says:

> `D = mean(ON) - mean(OFF)`, restricted to **the detected face region**.

Both cannot be true. The falloff test needs a face box, and the face box comes from
YuNet, which the diagram places *after* liveness.

**Resolution:** detection runs first, on the ON frame. Liveness then uses that box.
The principle being protected — *never embed or match something that has not passed
liveness* — is intact, because embedding and matching still come last. It is only
detection that has to move, and detection is 85 KB and ~5.6 ms.

Corrected pipeline:

```
V4L2 capture ─→ detect ─→ IR liveness ─→ align ─→ embed ─→ match
                (YuNet)   (uses the box)
```

The cheap checks that need no box — dark-frame integrity, whole-frame response — can
still run before detection as an early-out, and should, since they reject a screen
without paying for detection at all.

---

## B7 — Centre-crop ROI does not cover the face {#b7}

**Severity: high** — it is what blocks the project's one unproven claim.

`spikes/probe` used a centred 50% crop as a stand-in for a face box, on the assumption
that at laptop distance the face fills the middle of the frame. The cell maps say
otherwise. Mean response per cell, % of peak:

```
live_lit_frontal                     null_empty_room
   8   16   14   12   48  100          70   76   76   72   66   53
   9   10   10   66   47   94          77   82   81   78   72   59
   9    5   21   79   52   76          84   88   84   81   78   65
   6   25   38   74   69   77          90   95   90   87   84   70
  19   44   53   87   80   91          94  100   99   96   90   75
  39   54   57   92   79   94          77   66   73   80   81   71
```

The "frontal" capture peaks at the **bottom-right corner** and is nearly dark at the
top-left. The subject is at the edge of the ROI, not in the middle of it.

Consequence: `falloffCV` is measuring a subject-versus-background gradient, not facial
curvature. Two recordings of the *same* pose gave **0.31 and 0.64** — a 2× spread that
is entirely explained by the operator sitting in a slightly different place.

**This blocks S5 independently of the missing print sample.** Even with a print in hand,
comparing its falloff to a live face is meaningless while neither measurement is taken
over the face. Detection has to land first — which is the same conclusion as
[B8](#b8), reached from the other direction.

Marked in the source as a known ceiling:

```rust
// ponytail: centre crop instead of a real face box. [...] Swap for the YuNet
// detection box in Phase 1 -- at that point the ROI tracks the face instead of
// assuming it is centred, and off-centre captures stop being noise.
```

The metric itself is sound — `domed_response_has_higher_falloff_cv_than_flat` passes on
synthetic data, so the arithmetic distinguishes a curved reflector from a flat one. It
is the input that is wrong, not the formula.

### CLOSED, Phase 1 (2026-08-04)

YuNet landed and the ROI is now the detected face box. Falloff CV over the corpus, worst
pair per capture:

```
live_lit_frontal     0.3130      live_lit_glasses     0.2989
live_dark_frontal    0.2900      live_lit_angle_left  0.2661
```

**0.266–0.313, a 1.18× spread**, against the centre crop's 0.31 and 0.64 for the *same
pose*.
The prediction that the old spread was operator position rather than facial curvature
holds: with the ROI tracking the face, it mostly goes away.

This is what set the degenerate-flatness floor at 0.05 —
[05](05-vision-pipeline.md#liveness--the-differentiating-piece), `config.rs`. Replayed in
CI by `tests/liveness_corpus.rs`.

**What this does *not* close:** the live-versus-print separation. B7 was one of two
blockers on that claim and it is the one that is gone. The other — no printed photo has
ever been recorded — remains, and until it is, the falloff check claims only that it
rejects a perfectly flat presentation.

---

## B6 — `by-id` symlink resolves to the wrong camera {#b6}

**Severity: high.** Would have authenticated off the RGB sensor while believing it held IR.

[04](04-architecture.md) and [05](05-vision-pipeline.md) both specified persisting the
camera as a `/dev/v4l/by-id/` symlink, because `videoN` renumbers across boots. Correct
problem, wrong fix.

Both cameras are interfaces of one USB device reporting a single serial (`SN0001`), so
udev generates a colliding basename and the last writer wins. Four nodes, two symlinks,
**neither pointing at the IR camera**:

```
usb-Sonix_..._SN0001-video-index0 -> ../../video0   <-- RGB camera
usb-Sonix_..._SN0001-video-index1 -> ../../video3   <-- IR *metadata* node
```

Fixed by switching to `by-path`, where the USB interface number separates them
(`:1.0` RGB, `:1.2` IR). Full write-up:
[10/Probe 4](10-hardware-probe.md#probe-4--device-node-disambiguation).

### Recurrence, Phase 1

The same class bit again, in the golden-test harness. Detection goldens were keyed by the
capture's **label**, and labels are not unique — a second `live_lit_frontal` take was
recorded as a same-session control and the lookup silently returned the *first* take's
golden. Our decode was compared against OpenCV's reading of a different recording, from a
different day, with the operator sitting somewhere else. It surfaced as `box IoU 0.699`,
which reads like a decode defect and was nothing of the kind.

Fixed by keying on the filename stem, which carries the UTC timestamp. **The lesson
transferred but the vigilance did not**: the wiki already said a stable-looking name that
resolves to the wrong object is the dangerous kind, and the harness was written with a
non-unique key anyway. Worth noting that this one failed *loudly* — an assertion caught
it — which is the difference between B6 and this, and the whole argument for the
assertions being tight.

**Lesson:** a stable name that silently resolves to the wrong device is worse than an
unstable name that fails loudly. The daemon now re-verifies format and geometry on open
regardless of what the config says, because the config can be stale or wrong and one
ioctl is cheap against authenticating off the wrong sensor.

---

## B5 — ffmpeg silently duplicates frames when writing to disk {#b5}

**Severity: high.** Would have silently corrupted the entire corpus.

`-f rawvideo` rate-converts output to constant 30 fps and duplicates frames to fill it.
A 30-frame capture came back as 15 distinct sensor frames, each written twice, turning
the sensor's `ON,off,ON,off` into `ON,ON,off,off`.

The file is the right size. Every frame is valid. Nothing errors. Only the pairing is
wrong — and the pairing is what every liveness check depends on.

Fixed with `-fps_mode passthrough`. Full write-up:
[10/Probe 3c](10-hardware-probe.md#probe-3c--recording-to-disk-duplicates-frames-unless-you-stop-it).

**How it was caught:** the probe's output was cross-checked against the known-good
`ffmpeg signalstats` numbers already in [10](10-hardware-probe.md), and every luma value
appeared exactly twice. Without a prior independent measurement to compare against,
this would have looked like plausible data.

**Lesson, and it generalises past ffmpeg:** never assume the frames you were handed are
the frames the sensor produced. `faciolockd` reads V4L2 buffers directly and avoids this
specific bug, but dropped, repeated, and reordered buffers under load are the same
class. **ON/OFF phase must be re-established by measuring luma on every capture**, never
by assuming frame `n` is ON because frame `n-1` was OFF.

---

## B4 — Undocumented capture joins the corpus {#b4}

`live_lit_frontal_20260803T183008Z.gray` exists as a complete 90-frame file with no
`.json` sidecar, so there is no record of what was in front of the camera. It cannot be
trusted as a positive or a negative sample.

The sidecar is deliberately written *after* ffmpeg succeeds, so a sidecar always
describes a real file — but the converse was not enforced. `spikes/probe` now warns on
a missing sidecar rather than analysing it silently.

Root cause not established: the capture is byte-exact at 90 frames, so ffmpeg completed
and the script exited between that and the sidecar write. Interrupted run is the likely
explanation. Flagged rather than guessed at.

---

## B3 — `2>/dev/null` hid a missing binary as "no evidence" {#b3}

Probe 1 originally printed four device names and nothing else. `v4l2-ctl` was not
installed, and the redirect swallowed "command not found" — leaving output that read
like "there is nothing there" rather than "the tool is missing".

**Lesson:** a probe that degrades silently to "no evidence" is worse than one that fails
loudly, because absence of evidence reads as evidence of absence. Device enumeration in
the daemon must distinguish *absent*, *inaccessible*, and *present-but-not-IR* rather
than collapsing all three into an empty result.

---

## B2 — Half the `/dev/videoN` nodes are not cameras {#b2}

`/dev/video1` and `/dev/video3` are UVC payload-header metadata nodes (`UVCH`), one per
camera. They expose no capture formats and can never yield a frame.

Enumeration must filter on `V4L2_CAP_VIDEO_CAPTURE` before applying any IR heuristic.
Without it, the "capture one frame and check chroma variance" fallback in
[05](05-vision-pipeline.md#finding-the-ir-camera) is aimed at nodes that will never
answer. Details: [10/Probe 4](10-hardware-probe.md#finding-2--half-the-nodes-are-not-cameras).

---

## B1 — Dead conditional in `build_pairs` {#b1}

A leftover `if` with an empty body and a comment claiming the work happened elsewhere.
Removed before it ever compiled. Noted only because a conditional that looks like it
enforces something and enforces nothing is exactly the shape of a bug that survives
review.

---

## Environment friction (not bugs, but they cost time)

| Thing | Resolution |
|---|---|
| `sudo` needs a TTY the agent does not have | user runs privileged commands |
| `cargo` from repo root fails — no workspace yet | `--manifest-path spikes/probe/Cargo.toml` |
| `cargo clippy` not on `PATH` | lives at `/usr/lib/rust-1.93/bin/cargo-clippy` |
| `cargo fix` refuses without VCS | applied the two lints by hand |
| `mawk` has no `asort` | hand-rolled the sort |

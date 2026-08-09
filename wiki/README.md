# Faciolock — Knowledge Wiki

IR face authentication for Linux via PAM. Rust. Built to look and behave like a
platform component, not an add-on.

**Status: Phase 1 built.** `crates/faciolock-vision` takes frames in and produces a
verdict out — liveness, YuNet detection, alignment, SFace embedding, k-of-n matching — with
no camera, no sockets and no PAM. 78 tests, of which the ones that need neither the
recordings nor the models always run. Detection and alignment are pinned against
independent OpenCV references, which is what caught [B16](11-bug-log.md#b16).

**Phase 2 in progress.** `crates/faciolock-camera` (V4L2 capture, IR discovery) and
`crates/faciolock-ipc` (the wire types) are built. First contact with real hardware found
[B18](11-bug-log.md#b18) — every strobe statistic was a ratio, and ambient daylight
saturates a ratio — which is now fixed by moving to the difference domain and by an
**ambient gate**: above a measured floor this hardware reports "cannot verify" and PAM
falls through to the password, rather than guessing.

**The gate is at the wrong layer, and that is the open question of Phase 2.** It was
derived to protect `dark_ratio_max`, which genuinely dies in ambient light, but it sits in
front of the whole burst and so it also disables `falloff` — which is computed on the ON−OFF
*difference*, where the ambient pedestal cancels exactly, and which measurement now shows is
**unaffected by ambient right up to a pedestal of 51.7**. Daylight was written up as a
hardware ceiling; that was wrong, on a same-hardware fact (Windows Hello succeeds in the
room where Faciolock refuses, on this laptop). What blocks daylight is the negative class:
`falloff` has never been calibrated against a print, so opening the gate would trade a
measured refusal for an unmeasured acceptance. One afternoon of recording closes it —
**[13](13-daylight.md)** now holds that question in one place, with the ways out ranked.

One thing stated there that was not stated plainly anywhere before: the ambient gate is a
**UX hint, not a security control**, since [B20](11-bug-log.md#b20) showed the attacker sets
its input.

**`faciolockd` now answers, and Phase 2's exit criterion 4 is met** — `SOCK_SEQPACKET` at
0600, SHA256-verified models, a warm stream, and a real verdict from the real camera in
365 ms ([09](09-roadmap.md#phase-2--camera--daemon-skeleton-week-3)). The first run of it
found [B22](11-bug-log.md#b22): two crates each discarded the AGC settle frames, leaving 1
usable frame of 6, so **every** live authentication would have failed. 119 passing tests did
not see it, because the seam between two correct crates has no corpus.

The frame burst is now **measured at 404 ms** against a 130 ms estimate
([B19](11-bug-log.md#b19)), so the warm path is ~673 ms against a 400 ms target and
[06](06-performance-budget.md) is restated.

## Index

| Page | What's in it |
|---|---|
| [01 — Goals & Scope](01-goals-and-scope.md) | What we're building, what we're explicitly not |
| [02 — Howdy Teardown](02-howdy-teardown.md) | How Howdy actually works, measured weaknesses |
| [03 — PAM & Security Model](03-pam-and-security.md) | PAM contract, threat model, attack surface |
| [04 — Architecture](04-architecture.md) | Daemon + thin PAM module + CLI, privilege split |
| [05 — Vision Pipeline](05-vision-pipeline.md) | Camera, IR, models, licenses, liveness, matching |
| [06 — Performance Budget](06-performance-budget.md) | Latency/memory targets and where they go |
| [07 — Desktop & Packaging](07-desktop-and-packaging.md) | KDE/SDDM/sudo, Debian/Ubuntu upstreaming |
| [08 — Open Questions](08-open-questions.md) | Decisions needed before any code is written |
| [09 — Roadmap](09-roadmap.md) | 8 phases, exit criteria, risk register |
| [10 — Hardware Probe Log](10-hardware-probe.md) | Exact commands + raw output from the reference machine |
| [11 — Bug Log](11-bug-log.md) | Bugs hit and fixed, with the design changes they forced |
| [12 — Falloff Calibration Log](12-falloff-calibration.md) | Recording sessions and readings for the falloff check; S5 is still open |
| [13 — Daylight](13-daylight.md) | The daylight question in one place: what blocks it, the ways out ranked by cost, what is already refuted |

## One-paragraph summary

A privilege-separated design copied from `fprintd`: a small, unprivileged,
socket-activated daemon owns the camera and the ML; a ~400-line PAM module owns
nothing but a Unix socket. Detection and embedding run on permissively-licensed
ONNX models (YuNet MIT / SFace Apache-2.0) so the whole thing is redistributable by
Debian — EdgeFace was ruled out when its *weights* turned out to be non-commercial
([B10](11-bug-log.md#b10)). Liveness is arithmetic, not a neural net: the IR
camera already alternates emitter-lit and dark frames, so differencing them rejects
screens, prints, and IR-floodlight attacks for the cost of a few `f32` reductions and
no writes to the device. Enrolled templates are sealed to the TPM. Target: p95 unlock
under 400 ms wall-clock, zero RSS when idle.

## Decisions so far

| # | Decision | Date |
|---|---|---|
| Q6 | Debian/Ubuntu archive is the goal; design constrained accordingly | 2026-08-03 |
| ~~Q1~~ | ~~EdgeFace-XS for embedding~~ — **reversed, weights are non-commercial** | ~~2026-08-03~~ |
| Q1 | **SFace** for embedding (Apache 2.0). EdgeFace ruled out — [B10](11-bug-log.md#b10) | 2026-08-04 |
| Q2 | **`tract`** for inference — zero native deps beat `ort`'s 118 ms | 2026-08-04 |
| Q3 | No emitter control code — hardware self-strobes, verified by measurement | 2026-08-03 |
| Q7 | v1 scope is the **full vertical** — see [09](09-roadmap.md) | 2026-08-03 |
| P1-a | Matching is **2-of-3**; [06](06-performance-budget.md)'s budget restated to match | 2026-08-04 |
| P1-b | Uncalibrated liveness checks enforce a **degenerate floor only**, and report `enforced` vs `advisory` | 2026-08-04 |
| P1-c | CI corpus is **synthetic and committed**; the real recordings stay local | 2026-08-04 |
| P1-e | Capability tier is **enrolment-time config, never inferred per burst** | 2026-08-04 |
| P1-f | Detector runs at **320×320** — measured on embedding accuracy, not latency ([B13](11-bug-log.md#b13)) | 2026-08-04 |
| P2-a | **`v4l` crate** for capture, not hand-rolled ioctls — correct struct layouts on architectures that cannot be tested here | 2026-08-06 |
| P2-b | **`toml` + `serde`** for daemon config; both already in the archive, so no packaging cost | 2026-08-06 |
| P2-c | **Fresh frames only** — camera stays warm and streaming, queue flushed at request time | 2026-08-06 |
| P2-d | **Copy-out, not zero-copy** — 1.4 MB and ~150 µs against a 400 ms budget; the memory-budget argument for zero-copy was wrong by three orders of magnitude | 2026-08-06 |
| P2-e | **Strobe statistics move to the difference domain**; a ratio saturates against ambient IR ([B18](11-bug-log.md#b18)) | 2026-08-06 |
| P2-f | **Ambient gate at floor 0.04**, before any biometric work. Above it the answer is "cannot verify", not a guess | 2026-08-06 |
| P2-g | **`Environment` is exempt from the failure budget** — weather must not disable face unlock | 2026-08-06 |
| P2-h | **No RGB camera in v1.** Measured: it cannot see the emitter (SNR 0.85 vs the IR camera's 1,500–15,000), so it cannot do strobe liveness. AND-ing it in would rely on a branch that reads *true* for attackers in daylight; OR-ing it in lets a photo unlock. Deferred to v2 as an explicit opt-in tier | 2026-08-06 |

Still open: Q4 (IPC), Q5 (TPM timing), Q9 (name), and whether the real recordings are ever
published (deferred to ITP — publishing is irreversible, so it is not decided early).

## Reference hardware

ASUS Vivobook. IR camera at `/dev/video2`: `Y800` greyscale, 640×360 @ 30 fps,
`rawvideo` (no MJPEG decode needed), hardware-strobed emitter at 15 ON/OFF pairs per
second. Commands and raw output: [10](10-hardware-probe.md). Interpretation:
[05](05-vision-pipeline.md#measured-the-hardware-strobes-for-us).

Addressed by **`by-path`** (`…-usb-0:7:1.2-video-index0`). `by-id` collides on this
hardware and resolves to the RGB camera —
[10/Probe 4](10-hardware-probe.md#probe-4--device-node-disambiguation).

## Non-negotiables

1. No ML, no image decoding, no camera I/O inside the PAM module's address space.
2. Fail closed. Any error path returns a PAM failure code, never `PAM_SUCCESS`.
3. Every shipped model file must be redistributable under a DFSG-compatible license.
4. Never the sole authentication factor; password fallback always reachable.
5. **Liveness aggregates conjunctively — every pair in the burst must pass.** Matching
   aggregates k-of-n. Unifying the two reintroduces a measured 1-in-36 bypass
   ([B9](11-bug-log.md#b9)).

## Verified so far

> **Every capture below except the daylight ones was recorded after dark, and until
> 2026-08-06 nothing recorded that fact.** Ambient near-infrared turned out to be the
> variable the whole corpus was uniform in, and it breaks every ratio-domain statistic
> ([B18](11-bug-log.md#b18)). Rows are qualified accordingly. A claim measured in one
> lighting condition is a claim about that condition.

| Claim | Status |
|---|---|
| The ambient gate keys on a quantity **only the attacker can raise** | **refuted 2026-08-09** — presenting a bright object 40 cm from the lens collapses the floor 2,170×: the operator's own face read 28.221 and was *gated*, a ceramic mug in the same minute read 0.013 and *passed*. The AGC shortens exposure for a bright near object and the pedestal follows, so the attacker sets the gate's input ([B20](11-bug-log.md#b20)) |
| Falloff separates a live face from a **flat print** | **still unmeasured, after three recording attempts.** A flat blank page was captured 2026-08-09 and cannot settle it: with no face to detect, the ROI must be chosen, and across valid placements CV spans 0.036–1.71 (median 0.249) against a worst live of 0.2661 — wider than the live band, so the choice decides the answer ([B21](11-bug-log.md#b21)). Needs a flat presentation of a FACE, so detection supplies the ROI. A blank page is an *optimistic* stand-in, not a conservative one: print albedo adds variance falloff cannot distinguish from depth |
| The camera reports **exposure time** per frame, which would fix B20's gate | **refuted 2026-08-09.** The `UVCM` metadata node streams, and over 30 frames it carries **zero `CaptureStats`**. This firmware does not report its integration time, so pedestal ÷ exposure — the one candidate for a gate input the attacker cannot pull down — has no input here ([10/Probe 8](10-hardware-probe.md#probe-8--the-metadata-node)) |
| The camera reports **per-frame illumination**, ON or OFF | **verified 2026-08-09**, and unprompted: the tag is present in `DISABLED`, no mode change needed. Validated against the strobe 26/26, mean(ON) 99.90 against mean(OFF) 74.33. Hardware ground truth for the phase `frame::classify` infers from luma, which removes the 3.5-levels-of-headroom fragility in [B18](11-bug-log.md#b18). Robustness, not daylight; unbuilt, and needs its own decision entry |
| Hardware self-strobes, every frame | **measured**, 4 independent runs, and now in daylight too — the emitter never stops |
| ~~15 ON/OFF pairs/sec~~ | **withdrawn.** 30 fps was the *advertised* rate; delivered is 12–15 fps, so ~6–7 pairs/sec ([B19](11-bug-log.md#b19)) |
| Dark-frame integrity rejects an IR floodlight | **verified at negligible ambient IR only** — 4 orders of margin and 0 false rejects across 210 live pairs, all nocturnal. In daylight the same check rejects **100% of honest captures** (0.664 against a 0.001 threshold). The check is a ratio and ambient light is an offset ([B18](11-bug-log.md#b18)) |
| The strobe is detectable in daylight | **verified** — as a *difference*: SNR 549 whole-frame, 882 over the face box, sign regularity 1.000. `classify` was rewritten to measure it ([B18](11-bug-log.md#b18)) |
| **Magnitude** statistics can distinguish a floodlight from a live face above ambient ~1 | **refuted, by a matched pair** — same room, same position, 90 s apart, ambient within 1% (13.91 vs 14.05). Every magnitude tried — ratio, SNR, response — ranked the attack at or above the live face. `dark_ratio_max` cannot be rescued ([B18](11-bug-log.md#b18)) |
| ~~That refutation makes daylight a **hardware** ceiling~~ | **withdrawn 2026-08-08.** Three errors: the 940 nm-optics explanation is refuted by its own premise (Windows Hello succeeds in that room *on this same laptop*); both captures in the matched pair contain the operator's live face, so it compared live against live and no spoof was present; and every candidate in it was a magnitude, which is the class that provably fails |
| **Falloff is ambient-invariant** — the pedestal cancels in the ON−OFF difference | **verified across the corpus.** Daylit live band CV 0.307–0.385 sits *inside* the nocturnal band 0.266–0.348; the problem room reads 0.3071 mid-band and a capture at pedestal 51.7 reads 0.3847, the corpus maximum. The gate refuses bursts this check can still read ([B18](11-bug-log.md#b18)) |
| Falloff can cover for `dark_ratio_max` against IR injection | **refuted, same table** — attack captures read live-like (0.3173, 0.3963) because they contain live faces. Falloff measures subject *geometry*; it cannot see injected IR |
| The camera's **face-authentication mode** would lower the ambient floor | **refuted by measurement.** The IR camera advertises `MSXU_CONTROL_FACE_AUTHENTICATION` and Linux had never set it — every corpus capture was taken in `DISABLED`. Setting `ALT_FRAME_ILLUM` works, persists, and moves the floor not at all: 20.41 baseline vs 18.96/20.69/21.21/21.09 ([10/Probe 7](10-hardware-probe.md#probe-7--msxu-face-authentication-mode)). **Confirmed again 2026-08-09** in the bright room at ~2× the ambient and in reverse order, so drift worked against the hypothesis: 39.206 `DISABLED` vs 33.404/38.872 `ALT`, the mode's own repeat spread exceeding the between-mode gap, amplitude flat at 59.6–60.5. Coherent, because this sensor already self-strobes 24/7 — `DISABLED` *is* alternating illumination and the flag is bookkeeping |
| Faciolock works in a normally lit daytime room | **not yet, and it stays a non-goal pending calibration** ([01](01-goals-and-scope.md)). The blocker is the missing negative class, not the sensor: `falloff` enforces a 0.05 flatness floor against a live band bottoming at 0.2661 because **no print has ever been recorded**. Opening the gate today trades a measured refusal for an unmeasured acceptance |
| The RGB camera could supply liveness where IR cannot | **refuted by measurement** — it has an IR-cut filter and cannot see the emitter at all: strobe amplitude 0.049 and SNR 0.85 against the IR camera's 48–81 and 1,500–15,000 |
| YuNet detects a face in **daylight** IR | **verified** — and the face-box emitter response is 2.2× the whole-frame one, as inverse-square predicts |
| One contaminated pair sinks a burst (conjunctive aggregation) | **verified** — replayed in CI on synthetic and recorded captures |
| YuNet detects a face in **near-infrared** | **verified** — 4/4 live captures, 0 on an empty room, 0 on the floodlight attack |
| SFace embeddings discriminate on **near-infrared** | **verified** — genuine cross-capture cosine 0.60–0.90 against a 0.363 threshold |
| Falloff is measurable over a real face box | **verified** — 0.266–0.313, [B7 closed](11-bug-log.md#b7) |
| ~~Response test rejects screens~~ | **refuted** — a phone reads D=36 against a live minimum of 48.6, not ~0. The statistic is whole-frame, so it measures the room ([B17](11-bug-log.md#b17)) |
| A phone screen showing a photo is rejected | **verified** — but by *detection* (`NoFace`), not by any liveness check |
| **Falloff test rejects printed photos** | **untested, and deliberately deferred (2026-08-04).** [B7](11-bug-log.md#b7) is closed; no print has been recorded. The check therefore enforces only a degenerate-flatness floor and claims nothing about prints |
| False accept rate | **unmeasurable** — the corpus has one subject. The 0.363 cosine threshold is paper-derived || Falloff separates a live face from a **flat print** | **undetermined.** Three recording sessions; no reading that settles it. A flat page was captured on 2026-08-09, but with no face in it YuNet supplies no ROI, and across the 501 placements lying fully on the page the CV spans 0.0361–1.7116 (median 0.2486) against a live band of 0.2661–0.3847 — so the answer depends on the placement rule. Resolving it needs a printed face, so that detection supplies the ROI as it would in the pipeline ([12](12-falloff-calibration.md)) |

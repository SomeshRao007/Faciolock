# Faciolock — Task Tracker

Scope: **full vertical**. Plan and rationale in [wiki/09-roadmap.md](../../wiki/09-roadmap.md).
Decisions in [wiki/README.md](../../wiki/README.md).

Status: **Phase 1 built.** `crates/faciolock-vision` is complete end to end — 72 tests,
clippy clean. Two items outstanding, both named below rather than quietly dropped:
the OpenCV golden reference (needs one apt install) and the spoof recordings.

---

## Phase 0 — Spikes & corpus (week 1)

### Tooling — done

- [x] toolchain: `rustc` / `cargo` 1.93.1 via apt
- [x] `tools/record.sh` — resolves IR camera by `by-path`, verifies GREY + geometry,
      records raw Y800 + JSON sidecar
- [x] `spikes/probe` — zero-dependency analyser: tier detection, D, ON/OFF ratio,
      OFF/ON dark ratio, falloff CV, centre/edge ratio, p05/p50/p95
- [x] `cargo test` — 13 self-checks, run with no camera and no corpus
- [x] wiki corrected: `by-id` → `by-path`, metadata-node filtering, ratio-based
      dark-frame check, Probes 1/3b/3c/4 written up

### S3 — capture

- [x] classify ON/OFF frames by mean luma; **alternation confirmed from our own code**
      (independently reproduces the ffmpeg result in wiki/10)
- [x] found and fixed ffmpeg frame duplication that silently destroys ON/OFF pairing
      ([wiki/10 Probe 3c](../../wiki/10-hardware-probe.md#probe-3c--recording-to-disk-duplicates-frames-unless-you-stop-it))
- [ ] ~~Rust + `v4l` capture~~ — **deferred to Phase 2 on purpose.** ffmpeg answers
      "can we get frames" today; the real question is whether the `v4l` crate does
      MMAP zero-copy, and that belongs where the code lands.

### S4 — corpus

- [x] live face: lit room, frontal / angled / glasses on
- [x] live face: dark room
- [x] `attack_floodlight_remote` — IR remote at the sensor
- [x] `null_empty_room` — baseline
- [ ] **`spoof_print_matte` / `spoof_print_glossy`** — **DEFERRED by the user 2026-08-04.**
      Not dropped, and not quietly pending: the consequence is that the falloff test's
      print-rejection claim stays unproven, so `falloff_cv_min` remains a degenerate
      flatness floor and the README must keep saying so. Nothing in the code depends on
      this landing; only the strength of the claim does.
- [x] `spoof_phone` — recorded with a same-session control. **Rejected end to end
      (`NoFace`), but by detection, not by the response test** — which measurement refuted
      ([B17](../../wiki/11-bug-log.md#b17)).
- [ ] `spoof_laptop` — **now the priority after the print.** A larger, brighter panel is the
      case where detection may succeed, and then the response check over the face box is
      the actual defence. Untested.
- [ ] continuous IR illuminator (the remote is pulsed; continuous is assumed, not measured)

### S5 — thresholds

- [x] compute D, ON/OFF ratio, spatial falloff across corpus
- [x] **dark-frame integrity threshold: `max(OFF/ON) > 1e-3` → reject.**
      Verified: 4 orders of magnitude margin, 0 false rejects in 210 live pairs.
- [x] found [B9](../../wiki/11-bug-log.md#b9) — pulsed IR defeats per-pair aggregation
      (16.7% pass rate → 1-in-36 bypass at the 2-pair budget). Liveness must aggregate
      conjunctively.
- [ ] **separation between live and print** — **one of the two blockers is now gone.**
      [B7](../../wiki/11-bug-log.md#b7) is closed and falloff is measurable over a real face
      box. What remains is simply that no printed photo has ever been recorded. Until one
      is, the falloff check enforces only a degenerate-flatness floor and claims nothing
      about prints.

### Newly required before S5 can finish

- [x] YuNet detection, to give the falloff test a real face box
      ([B7](../../wiki/11-bug-log.md#b7)) — **done in Phase 1, B7 CLOSED.** Live falloff CV
      over the real box is 0.26–0.39 (1.48× spread) against the centre crop's 0.31/0.64 for
      the same pose, confirming the old spread was operator position, not curvature.

### S1/S2 — runtime & model

- [x] S1 — load YuNet + SFace ONNX under `tract`. **Both load.** YuNet 7.7 ms @160×160,
      11.2 ms @192×192; SFace 85.3 ms @112×112. Single-threaded.
- [x] S1 — `ort` measured too: YuNet 0.45 ms @160×160, 1.27 ms @192×192; SFace 30.9 ms.
      **`ort` is 3–20× faster than `tract`.**
- [x] S1 — `tract`-specific gap: **no quantized ops** (`QLinearAdd`). `ort` runs int8.
- [x] S1 — ~~non-square is a tract limit~~ — **wrong, it is a YuNet model limit**; `ort`
      fails identically. Frame must be letterboxed square either way.
      ([B11](../../wiki/11-bug-log.md#b11))
- [x] S1 — budgets: **~318 ms (`tract`) / ~200 ms (`ort`)**, both inside the 400 ms p95
- [x] **S1 — Q2 CLOSED: `tract`.** Decided on zero native deps (binary links only
      libc/libm/libgcc_s) and attack surface, not speed. Accepts 82 ms headroom and no
      int8. `ort` removed from the spike and from the system.
- [x] S2 — confirm EdgeFace BSD-3 covers trained weights — **NO, they are CC BY-NC-SA
      4.0.** Q1 reversed to SFace. [B10](../../wiki/11-bug-log.md#b10)
- [x] ~~S2 — export EdgeFace to ONNX~~ — dropped, model ruled out
- [ ] S2 — clear SFace's residual licence risk (upstream repo states no licence, training
      data unstated) before ITP

**Exit:** ~~runtime chosen with data~~ **done (`tract`)**; live-vs-print separation
demonstrated or refuted — **not done.**

> **Exit criterion is still not met, but is now reachable.** The real face box landed in
> Phase 1; the printed photo has not. One trip to a printer closes Phase 0.

---

## Phase 1 — `faciolock-vision` (week 2)

Frames in, verdict out. No camera, no sockets, no PAM. Nothing in this phase touches
`/etc`, systemd, or any live system setting.

### Decisions taken at Phase 1 kickoff (2026-08-04)

| # | Decision | Consequence |
|---|---|---|
| P1-a | **Matching is 2-of-3**, not 2-of-2 | 3 SFace passes = 255 ms → warm path ~403 ms. [06](../../wiki/06-performance-budget.md) restated rather than the code bent to fit the number. |
| P1-b | **Uncalibrated liveness checks enforce a degenerate floor only** | Falloff/response/motion have no spoof sample. Enforce a flatness floor strictly below the worst measured live value; report every check as `enforced` or `advisory` in the verdict. |
| P1-c | **CI corpus is synthetic and committed; real recordings stay local** | Debian buildd has no network — a downloading test does not run in the archive. Synthetic proves the logic; the local corpus proves the thresholds. Whether real data is ever published is deferred to ITP. |
| P1-d | **Golden references come from OpenCV**, generated in a disposable venv | A self-generated golden freezes a wrong decode as readily as a right one. |
| P1-e | **Tier is enrolment-time config, never inferred per burst** | A floodlight that washes out the strobe would otherwise *downgrade* the machine to a weaker tier. Non-conforming burst → reject, never downgrade. |

### Build order — detection first ([B7](../../wiki/11-bug-log.md#b7)/[B8](../../wiki/11-bug-log.md#b8))

- [x] **S0** workspace + `crates/faciolock-vision` skeleton, `exclude = ["spikes"]`
- [x] **S0** wiki updated for P1-a (budget), P1-b (liveness posture), P1-e (tier)
- [x] **S1** `frame.rs` — Frame/Burst, ON/OFF classification by mean luma (ported from
      `spikes/probe`, already proven against the corpus)
- [x] **S1** burst conformance: reject a burst that does not match the *configured* tier
      — wrong alternation, or fewer than the minimum pairs. **Never downgrade.**
- [x] **S2** `detect.rs` — YuNet under `tract`: letterbox to square (pad 0, gray→3ch
      replicate, both pinned constants), decode `cls`/`obj`/`bbox`/`kps` at strides
      8/16/32, NMS
- [x] **S2** detect returns boxes and landmarks in **full-frame coordinates** — the
      inverse letterbox lives inside `detect.rs`, not at its call sites
- [x] **S2** behavioural check: exactly one face on `live_*`, none on `null_empty_room`
- [x] **S2b** **CHECKPOINT — closes [B7](../../wiki/11-bug-log.md#b7).** Re-run the whole
      corpus falloff with real face boxes. Record the true live falloff distribution;
      the 0.31/0.64 spread should tighten. This is the number P1-b's floor is set from.
- [x] **S3** `liveness.rs` — pre-box checks (dark-frame integrity, whole-frame response)
      exposed separately from post-box checks (falloff, motion), so `lib.rs` can
      early-out before paying for YuNet
- [x] **S3** conjunctive aggregation across the burst, with a **vacuous-conjunction
      guard**: zero or too-few pairs is a reject, not an empty-set pass
      ([B9](../../wiki/11-bug-log.md#b9))
- [x] **S3** per-check `enforced` / `advisory` status carried in the verdict (P1-b)
- [x] **S4** venv + OpenCV reference generator → `tools/gen-goldens.py`. **Found
      [B16](../../wiki/11-bug-log.md#b16)** — alignment sampled half a pixel off ArcFace's
      convention. Invisible to four self-consistent tests; caught by the external reference
      on the first run.
- [x] **S4** `align.rs` — Umeyama similarity transform, closed-form 2×2 SVD, **no linalg
      crate** (a dependency's SVD bit-behaviour can shift across versions and would
      undermine the determinism the golden test exists to protect)
- [x] **S4** golden test covers the **whole detect→align chain**, not Umeyama alone.
      Decode vs OpenCV: box IoU 0.998-0.999, worst landmark 0.05-0.19 px. Warp vs OpenCV:
      mean |diff| 0.000, max 1, 100% within 2 levels.
- [x] **S5** `embed.rs` — Embedder trait + SFace (`data` in raw 0-255, `fc1` out, L2
      normalise), cosine matching, best-of-N templates, 2-of-3 agreement
- [x] **S6** `lib.rs` — pipeline wiring; **reason-carrying `Verdict` enum** (`NoFace` /
      `InsufficientBurst` / `LivenessReject{check}` / `NoMatch` / `Match`), since Phase 3's
      IPC and Phase 4's audit log both freeze these variants
- [x] **S6** enrolment API (burst → template) — the corpus match test cannot exist without it
- [x] **S7** synthetic corpus generator, committed (P1-c)
- [x] **S7** real-corpus replay: loud `DEGRADED` banner when absent, never a silent pass
      ([B3](../../wiki/11-bug-log.md#b3)); `FACIOLOCK_CORPUS_REQUIRED=1` makes absence fatal
- [x] **S7** [B9](../../wiki/11-bug-log.md#b9) regression: synthetic burst with exactly
      one contaminated pair must reject
- [x] **S7** port `spikes/probe`'s property tests (domed-vs-flat, gain invariance, tier
      classification) — the only camera-free liveness tests that exist
- [x] **S7** model SHA256 pinned in tests; a decode validated against `2026may` is not
      validated against `2023mar`
- [x] **S8** measure: landmark delta 192 vs 320, and embedding cosine sensitivity to
      ±0.5/1/2/4 px landmark perturbation. Detector input size is config, not a constant.
- [x] **S8** measure: FRR for 2-of-2 and 2-of-3 across the corpus; real warm-path latency

### Carried into Phase 1 from Phase 0

- [x] **user action:** record `spoof_phone` — done, with a control. Result: the attack is
      denied, the response test is refuted ([B17](../../wiki/11-bug-log.md#b17)).
- [ ] **user action, deferred:** `spoof_print_matte` / `spoof_print_glossy`. Protocol is in
      [corpus/README.md](../../corpus/README.md#recording-a-spoof--the-protocol) for
      whenever it happens. The risk it leaves open is unchanged and is in
      [09](../../wiki/09-roadmap.md)'s register: if falloff turns out not to separate, the
      fix is a liveness redesign, and finding that out later costs more than finding it out
      now. That is a known, accepted cost, not an oversight.

### Known limits of Phase 1, to be stated and not papered over

- **One subject in the corpus, so FAR is unmeasurable.** The cosine threshold will be
  OpenCV's RGB-calibrated 0.363, documented as paper-derived and provisional. It does not
  meet [05](../../wiki/05-vision-pipeline.md#matching)'s own "measured on our own corpus"
  standard and must not be described as if it does.
- **The whole stack is RGB-trained running on NIR.** YuNet is WIDER Face, SFace is LFW.
  Whether either works on greyscale IR at ~45 px is assumption, not measurement, until S2.
- **CI corpus provisioning is unresolved**, deliberately (P1-c). Named blocker, not drift.

**Exit:** `cargo test` replays the corpus and asserts verdicts with no camera attached;
[B7](../../wiki/11-bug-log.md#b7) closed by a real face box; alignment frozen against an
external reference.

---

## Phase 2 — Camera & daemon skeleton (week 3)

- [ ] `v4l` MMAP zero-copy capture
- [ ] device discovery via **`by-path`** symlink — not `videoN` (renumbers), not
      `by-id` (collides, resolves to the RGB camera — [wiki/10 Probe 4](../../wiki/10-hardware-probe.md#probe-4--device-node-disambiguation))
- [ ] filter to `V4L2_CAP_VIDEO_CAPTURE` first — 2 of 4 nodes are UVC metadata nodes
- [ ] IR heuristics: GREY format → name → resolution → chroma variance
- [ ] re-verify format + geometry on open, regardless of stored config path
- [ ] read-only capability probe (30 frames, alternation test) — **must never write**
- [ ] `faciolockd` skeleton + config loading
- [ ] **measure the real frame-burst cost** — 130 ms is an estimate and only 82 ms of
      headroom sits behind it (Phase 2 exit condition)
- [ ] model SHA256 verification at startup, refuse to run on mismatch
- [ ] camera-busy handling: reply fast, don't fight for the device

**Exit:** daemon answers a hand-rolled socket request from the real camera, <400ms warm.

---

## Phase 3 — PAM module & IPC (week 4) — SECURITY CRITICAL

- [ ] `faciolock-ipc`: fixed-layout binary protocol, hard size caps
- [ ] `faciolock-pam` cdylib: `pam_sm_authenticate` + `pam_sm_setcred`
- [ ] `catch_unwind` at FFI boundary
- [ ] single `PAM_SUCCESS` construction site — audit that no other path reaches it
- [ ] remote-session rejection (`PAM_RHOST`, `SSH_CONNECTION`, `SSH_CLIENT`, `SSH_TTY`)
- [ ] `SO_PEERCRED` check before reading request body
- [ ] cancellation: socket close → abort, release camera, log as cancelled (not failed)
- [ ] **CI: assert `cargo tree -p faciolock-pam` == libc + faciolock-ipc only**
- [ ] verify every failure path falls through to password prompt

**Exit:** `sudo` authenticates by face. Daemon killed mid-auth → password prompt, no hang.

---

## Phase 4 — Storage, TPM, policy (week 5)

- [ ] template store keyed by **uid**, multiple templates per user
- [ ] TPM2 sealing via `tpm2-tss`, conservative PCR set
- [ ] documented re-enrol path for firmware/kernel updates
- [ ] non-TPM fallback: root-only file, reduced guarantee documented
- [ ] rate limiter: per-user exponential backoff
- [ ] rate limiter persistence across daemon restarts
- [ ] lockout: N failures → face disabled until successful password auth
- [ ] audit log: verdict, reason, liveness score, distance, peer PID/UID

**Exit:** template copied to another machine is unusable; backoff survives daemon kill.

---

## Phase 5 — CLI & enrolment (week 6)

- [ ] `faciolockctl enroll` — guided multi-capture, quality feedback, tier detection
- [ ] `faciolockctl list` / `remove`
- [ ] `faciolockctl test` / `diagnose`
- [ ] `faciolockctl bench` — p50/p95/p99, not mean
- [ ] polkit policy for privileged operations

**Exit:** someone who hasn't read the wiki can enrol and use it from the CLI alone.

---

## Phase 6 — Integration & hardening (week 7)

- [ ] systemd `.socket` + `.service`, socket activation
- [ ] idle-exit timeout
- [ ] full sandbox profile → `systemd-analyze security` ≤ 2.0
- [ ] pre-warm session agent on compositor idle/lock signal
- [ ] PAM service: `sudo`
- [ ] PAM service: `su`
- [ ] PAM service: `kde` (kscreenlocker)
- [ ] PAM service: `sddm`
- [ ] PAM service: `polkit-1`
- [ ] PAM service: `login`
- [ ] `cargo-fuzz` on IPC parser
- [ ] `cargo-fuzz` on frame-handling path

**Exit:** zero RSS idle. KDE unlocks by face AND password entry stays instant.

---

## Phase 7 — Packaging & release (week 8)

- [ ] `faciolock` package (daemon, CLI, units, config)
- [ ] `libpam-faciolock` package (module + pam-configs profile)
- [ ] `faciolock-models` package (ONNX + vendored licences)
- [ ] `/usr/share/pam-configs/faciolock` — `Default: no`, `Priority: 512`
- [ ] reproducible builds
- [ ] README with security ceiling stated plainly
- [ ] hardware compatibility list (seed: ASUS Vivobook, /dev/video2, Y800 640×360)
- [ ] documented recovery path if module misbehaves
- [ ] security policy / CVE contact
- [ ] PPA release

**Exit:** clean install on fresh Kubuntu VM → enrol → unlock → uninstall, no manual
`/etc/pam.d` editing at any point.

---

## Beyond v1

- [ ] KDE settings module (KCM) — QML/Kirigami
- [ ] Debian ITP → archive
- [ ] independent security review
- [ ] GNOME/GDM support
- [ ] Q9 — name collision check (crates.io, Debian archive, trademarks) **before publishing**

---

## Review

### Phase 1 — `faciolock-vision` (2026-08-04)

**Built.** `crates/faciolock-vision`, one dependency (`tract-onnx`), 8 modules, 72 tests,
clippy clean. Frames in, verdict out; no camera, no sockets, no PAM. Nothing outside the
project tree was touched.

| Module | What it owns |
|---|---|
| `frame.rs` | Frame/Burst, ON/OFF classification, tier conformance |
| `liveness.rs` | four checks, conjunctive aggregation, pre-box / post-box split |
| `detect.rs` | YuNet: letterbox, decode 12 tensors, NMS, **inverse letterbox** |
| `align.rs` | Umeyama similarity → canonical 112×112, closed-form, no linalg crate |
| `embed.rs` | Embedder trait + SFace, L2 normalise, cosine, k-of-n |
| `pipeline.rs` | orchestration, reason-carrying `Verdict`, enrolment |
| `config.rs` | every threshold, with the measurement behind it in the doc comment |
| `testing.rs` | synthetic capture generators — the committed CI corpus |

#### What measurement changed

Five things were decided by argument and turned out wrong when measured. All five are in
[wiki/11](../../wiki/11-bug-log.md).

| Was | Is | Why |
|---|---|---|
| detector at 192×192 | **320×320** | 192 costs 0.82 embedding cosine — worse than 8 px of landmark error ([B13](../../wiki/11-bug-log.md#b13)) |
| score threshold 0.6 | **0.3** | an angled live face scores 0.549 ([B14](../../wiki/11-bug-log.md#b14)) |
| detect per embedded frame | **once per burst** | sharing landmarks costs ~0.98; small-input detection costs 0.82 |
| SFace at 85 ms | **76.9 ms** | and a first measurement said 39 ms, from inside the CPU boost window ([B15](../../wiki/11-bug-log.md#b15)) |
| falloff CV 0.31–0.64 | **0.26–0.39** | the old spread was operator position, not curvature ([B7 closed](../../wiki/11-bug-log.md#b7)) |

#### What is now verified that was not

- **YuNet detects faces in near-infrared** — 4/4 live, 0 on an empty room, 0 on the
  floodlight attack. Every later stage inherited this assumption; nothing had checked it.
- **SFace discriminates on near-infrared** — genuine cross-capture cosine 0.60–0.90 against
  a 0.363 threshold. An RGB-trained embedder survives 8-bit NIR.
- **B7 closed** — falloff is measurable over a real face box.
- **Conjunctive aggregation holds** — one contaminated pair in six sinks the burst, in both
  synthetic and recorded replays.

#### What is deliberately *not* claimed

- **Print rejection.** Still the project's one unproven claim. B7 is closed; the missing
  printed photo is not. The falloff check enforces a degenerate-flatness floor at 0.05 and
  claims only that it rejects a perfectly flat presentation.
- **FAR.** Unmeasurable — one subject. The 0.363 cosine threshold is OpenCV's RGB
  calibration and is labelled paper-derived everywhere it appears.
- **Screen rejection.** No `spoof_phone` recording, so the response check stays advisory
  and the motion check has demonstrated no discriminating power at all (it reads 0.0000 on
  every live capture, because two true dark frames really are identical).
- **p95 latency.** The ~399 ms figure is a sum of per-stage medians, which is not a p95,
  and 130 ms of it is a frame-burst estimate that has still never been measured.

Three of the four liveness checks are `advisory` and say so in the verdict, so the audit
log in Phase 4 will be truthful without further work.

#### Outstanding

1. ~~OpenCV golden reference~~ — **done**, and it found [B16](../../wiki/11-bug-log.md#b16).
2. **`spoof_phone` recording** — 30 seconds, no printer needed, calibrates the response check.
3. **`spoof_print_*`** — the one that matters.
4. **Second consenting subject** — the only thing that makes FAR measurable.

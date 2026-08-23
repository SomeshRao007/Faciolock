# 08 — Open Questions

Decisions needed before code. Each is a real fork in the road, not a preference.

---

## Q1 — Embedding model: SFace or EdgeFace? — **REVERSED: SFace**

**Decision (2026-08-04): SFace.** The EdgeFace decision of 2026-08-03 is withdrawn.

The gating check it was staged on came back negative:

- [x] Confirm EdgeFace's BSD-3 LICENSE covers the trained weights — **NO.**
      The authors distribute the weights on HuggingFace under **CC BY-NC-SA 4.0**
      (NonCommercial), while the GitHub repo carries BSD-3 for the code. All four
      official `Idiap/EdgeFace-*` model cards declare it in their own frontmatter.
      NonCommercial fails DFSG §6, so Debian cannot ship it and neither can we while
      [Q6](#q6--target-debian-archive-or-ppa-and-see--decided-archive) stands.
      Full analysis: [B10](11-bug-log.md#b10).
- [x] Confirm the model loads under the runtime Q2 picks — **SFace loads under `tract`**,
      85 ms at 112×112. Measured, see Q2 below.

This is the second model ruled out for exactly the same reason as InsightFace: permissive
code, non-commercial weights. The embedder trait staged in
[05](05-vision-pipeline.md#embedding) is what made this a config change rather than a
rewrite.

**Cost of the reversal:** 128-d embeddings instead of 512-d, ~37 MB instead of ~7 MB,
and 85 ms instead of a projected 10–15 ms. The budget still closes — see Q2 — but the
headroom is gone.

**Residual risk, not yet cleared:** SFace's Apache 2.0 grant comes from OpenCV Zoo,
which ships a per-model `LICENSE` beside the ONNX. The upstream
[zhongyy/SFace](https://github.com/zhongyy/SFace) repo declares no licence at all, and
the training dataset is not stated. Must be resolved before an ITP.
`dlib_face_recognition_resnet_model_v1` (**CC0**) is the fallback-of-last-resort.

### Original framing

**SFace** (Apache 2.0, OpenCV Zoo) — 37 MB, ~65 ms/inference, 99.40% LFW, 128-d.
Battle-tested, shipped inside OpenCV, int8 variants published, license unambiguous.

**EdgeFace-XS** (BSD 3-Clause, Idiap) — ~7 MB, 154 MFLOPs (≈an order of magnitude
less compute), 99.73% LFW, 512-d. Better on every technical axis. Costs: it's a
research release with less field deployment, we'd export ONNX ourselves, and we must
confirm the BSD license explicitly covers the *weights*, not just the code.

Third option: build against a trait, ship SFace as default, evaluate EdgeFace on our
own corpus, switch if it wins. Costs a little abstraction; de-risks the choice.

---

## Q2 — Inference runtime — **DECIDED: `tract`**

**Decision (2026-08-04): `tract`.** Both candidates were measured on the reference
machine and both meet the latency target, so this was decided on footprint and attack
surface rather than on speed.

What it buys, measured:

- **Zero native dependencies.** The built binary links `libc`, `libm`, `libgcc_s` and
  nothing else. `ort` needs 8 packages / 72.1 MB.
- **~62 MB total install** against ~134 MB.
- No C++ in the process that parses V4L2 buffers.
- `MemoryDenyWriteExecute=yes` works, so the
  [`systemd-analyze security` ≤ 2.0 commitment](06-performance-budget.md) is reachable
  rather than an open question.
- Debian packaging is nothing to do — it is a crate. No SONAME coupling to onnxruntime
  across releases.

What it costs, measured:

- **318 ms warm path against `ort`'s 200 ms**, leaving **82 ms of headroom** under the
  400 ms p95 target instead of 200 ms.
- **No int8.** `QLinearAdd` is unimplemented, so quantization is permanently unavailable
  as an optimisation lever.
- Operator gaps are a standing risk for any future model swap.

**The open risk this decision accepts:** the 130 ms frame-burst term is an estimate, not
a measurement, and it is the number most likely to move — cameras stall, laptops
throttle, and a lock screen runs under load. If it is 30% optimistic, `tract` lands at
~360 ms and the margin is nearly gone. **Measuring the real frame-burst cost is a Phase 2
exit condition**, and if it blows the budget the embedder trait plus this recorded
comparison make switching a contained change rather than a re-investigation.

`ort` numbers are retained below deliberately. A rejected option with its measurements
intact is what makes a reversal cheap.

### Measured 2026-08-04 (S1 spike, complete)

Intel Core Ultra 5 225H, **single-threaded both sides**, release build, 50 iterations
after warm-up. `ort` 2.0.0-rc.13 against the distro's onnxruntime 1.23.2, loaded with
`load-dynamic`. Same input buffer fed to both.

| Model | Input | `tract` p50 | `ort` p50 | ratio |
|---|---|---|---|---|
| YuNet `2026may` | 160×160 | 7.69 ms | **0.45 ms** | **17×** |
| YuNet `2026may` | 192×192 | 11.15 ms | **1.27 ms** | **8.8×** |
| YuNet `2026may` | 320×320 | 33.94 ms | **1.67 ms** | **20×** |
| YuNet `2023mar` | 640×640 | 145.0 ms | — | |
| SFace | 112×112 | 85.25 ms | **30.85 ms** | **2.8×** |
| YuNet `2023mar_int8` | 640×640 | **FAILS** — `QLinearAdd` | 11.72 ms | ort only |
| YuNet `2026may` | 640×360 | FAILS | FAILS | **model limit, see below** |
| YuNet `2026may` | 160×120 | FAILS | FAILS | **model limit** |

**Both runtimes load and run everything we need. The gating question is answered yes
for both** — nothing here forces a decision, which means Q2 is a genuine trade-off
rather than a constraint.

**`ort` is 3–20× faster.** Not the modest gap the original framing assumed.

### Correction: non-square is a model limitation, not a `tract` limitation

An earlier pass recorded "non-square inputs fail" as a `tract` defect. **It is not.**
`ort` fails on exactly the same inputs, and its error is legible:

```
Non-zero status code returned while running Add node. Name:'Add_50'
Attempting to broadcast an axis by a dimension other than 1.  44 by 45
```

YuNet `2026may`'s internal anchor/stride arithmetic does not survive H≠W. **Our 640×360
frame must be letterboxed into a square before detection regardless of which runtime we
pick.** That is a real design constraint on the pipeline and it belongs in
[05](05-vision-pipeline.md), not in this comparison.

The one true `tract`-specific gap that remains is **no quantized-operator support**
(`QLinearAdd`), which rules out the int8 variants. `ort` runs them.

### Budget with the measured numbers

Warm path from [06](06-performance-budget.md), SFace embedder, detection at 192×192:

| Stage | `tract` | `ort` |
|---|---|---|
| connect + request | 1 ms | 1 ms |
| frame burst (4 @ 30 fps) | 130 ms | 130 ms |
| liveness arithmetic | 2 ms | 2 ms |
| YuNet @ 192×192 | 11 ms | 1 ms |
| alignment | 1 ms | 1 ms |
| SFace ×2 | 170 ms | 62 ms |
| match + policy + response | 3 ms | 3 ms |
| **Total** | **~318 ms** | **~200 ms** |

**Both clear the 400 ms p95 target.** `tract` does it with 82 ms of headroom and zero
native dependencies; `ort` with 200 ms of headroom and a C++ shared library.

With `tract`, SFace is **53%** of the budget and the frame burst 41%. With `ort` the
frame burst dominates at 65% and further model optimisation buys almost nothing —
which is the shape the original EdgeFace projection assumed.

### Where the trade-off actually sits

Everything below is measured on the reference machine, not estimated.

| | `tract` | `ort` | delta |
|---|---|---|---|
| **Latency** | | | |
| YuNet @ 192×192 | 11.15 ms | 1.27 ms | ort **8.8× faster** |
| SFace @ 112×112 | 85.25 ms | 30.85 ms | ort **2.8× faster** |
| Warm-path total | 318 ms | 200 ms | ort **−118 ms (−37%)** |
| Headroom under the 400 ms p95 target | 82 ms | 200 ms | ort **2.4× the margin** |
| **Footprint** | | | |
| Runtime dependency packages | **0** | **8** | |
| Runtime dependency size | **0 MB** | **72.1 MB** | |
| Stripped daemon binary | 24.65 MB | 24.78 MB | equivalent |
| Total install (binary + SFace + deps) | **~62 MB** | **~134 MB** | tract **−54%** |
| **Capability** | | | |
| Quantized (int8) models | **no** — `QLinearAdd` | yes — 11.72 ms @ 640×640 | ort only |
| ONNX operator coverage | good, gaps exist | complete | |
| Non-square input | fails | fails | **model limit, not a differentiator** |
| **Security / packaging** | | | |
| C++ in the process parsing camera frames | **no** | yes | |
| `MemoryDenyWriteExecute=yes` | fine | **unverified** | |
| Debian packaging | a crate, nothing to do | `Depends: libonnxruntime1.23`, already in universe `+dfsg` | |
| SONAME-skew risk across releases | none | real — pinned to onnxruntime 1.23 | |

The 8 runtime packages, for the record — `apt install libonnxruntime-dev` pulled all of
these, none were present beforehand:

```
libdnnl3.6            46246 KB   <- oneDNN, 64% of the total on its own
libonnxruntime1.23    20097 KB
libonnx1t64            4078 KB
libxnnpack0.20241108   1886 KB
libonnxruntime-providers 910 KB
libre2-11               424 KB
libpthreadpool0         162 KB
libcpuinfo0              70 KB
                      -------
                      72.1 MB
```

**How much the dependency actually costs is a matter of who pays.** For an end user it
is nothing: a packaged `faciolock` would declare `Depends: libonnxruntime1.23` and apt
would pull it invisibly. The manual install above is what a *developer* does, not what a
user experiences. What the 72 MB genuinely buys is a larger install, a larger attack
surface in the daemon, eight more packages that can carry a CVE, and a hard version
coupling to onnxruntime's SONAME across Debian releases.

**Both options meet the performance target.** `tract` is not "fast enough with
concessions" — 318 ms sits inside a 400 ms p95 budget. The real question is whether
**82 ms of headroom** is enough margin once real cameras stall, the machine is under
load, or a laptop is thermally throttled — because the 130 ms frame burst is itself an
estimate, and it is the term most likely to move.

Neither is decided here. See [09](09-roadmap.md) risk register for what each choice
leaves exposed.

### Correction to the packaging argument

The claim below that "onnxruntime is awkward to package for Debian" is **out of date**.
Ubuntu ships `libonnxruntime-dev 1.23.2+dfsg-6ubuntu1` in universe, and the `+dfsg`
suffix means the Debian repackaging work is already done. `ort`'s packaging disadvantage
is therefore much weaker than assumed when Q2 was framed.

What survives as a genuine `ort` cost: a C++ shared library in the daemon's address
space, and the `MemoryDenyWriteExecute=yes` question is still unanswered. Also worth
noting from the spike — `ort`'s convenient `download-binaries` feature pulls a TLS stack
and `openssl-sys` at build time, which is not something to ship in a distro package.

### Original framing

**`ort`** — fastest, complete ONNX operator coverage. Costs: a ~15 MB C++ native
dependency that is awkward to package for Debian, and it may fight
`MemoryDenyWriteExecute=yes` in the sandbox.

**`tract`** — pure Rust, no native deps, trivial to package, sandbox-friendly.
Costs: slower, and operator coverage must be verified against YuNet and the chosen
embedder before we can commit. Given EdgeFace is 154 MFLOPs, "slower" may be
irrelevant in absolute terms.

**`candle`** — pure Rust, HuggingFace ecosystem. Benchmarks put it behind ORT on
small CPU models; weakest fit for a latency-sensitive CPU-only path.

This interacts with Q1 and with the archive goal (Q6). Suggested resolution: a
**one-day spike** — load YuNet + both embedders in `tract` and in `ort`, measure. Let
the number decide instead of the argument.

---

## Q3 — IR emitter — **RESOLVED: no emitter code at all**

**Measurement (2026-08-03) settled this.** The reference machine's IR camera
(`/dev/video2`, Y800, 640×360 @ 30 fps) already alternates emitter-ON and
emitter-OFF frames in hardware, 15 pairs per second, with no input from us. Raw data
in [05](05-vision-pipeline.md#measured-the-hardware-strobes-for-us).

We therefore write **zero** UVC control code. Deleted from scope:

- native `UVCIOC_CTRL_QUERY` implementation
- the per-laptop emitter configuration database
- the hardware denylist for motion-reactive ASUS/Sonix devices
- the USB-bus-hang risk entirely
- any runtime dependency on `linux-enable-ir-emitter` (downgraded to `Suggests:`)

Capability detection becomes a **read-only observation** — capture 30 frames, test
for alternation — instead of a write-probe. Strictly safer and strictly simpler.

Bonus the hardware handed us: true dark frames enable an external-IR-floodlight
check that the original design could not perform.

Caveat: this is measured on **one** laptop. The `ir-constant` tier exists for
hardware that doesn't self-strobe, and falls back to passive checks.

### Original framing

**Native** (`UVCIOC_CTRL_QUERY` ioctls): gives per-frame emitter control, which the
strobe-difference liveness scheme in [05](05-vision-pipeline.md) *requires* — an
external tool can only turn the emitter on and leave it on. Costs: this is the most
hardware-specific, most painful part of the project, and we'd be rebuilding a
per-device configuration database that already exists.

**Depend on it**: saves that work, but reduces liveness to passive-only, which
substantially weakens the project's main differentiator.

**Hybrid**: use their configuration database (it's data, and the licensing allows
reuse) with our own ioctl code. Probably the answer, but it needs checking.

---

## Q4 — IPC: raw Unix socket, D-Bus, or both?

Raw socket keeps the root-process dependency tree at `libc` — the strongest security
property in the design, and the one a distro reviewer checks first. D-Bus gives free
KDE/polkit/settings integration, which is most of "feels built-in".

The hybrid (raw socket for the PAM hot path, D-Bus for CLI and settings UI) gets
both, at the cost of two IPC surfaces in the daemon. Recommend the hybrid unless
there's a reason not to.

---

## Q5 — TPM sealing: v1 or later? — **DECIDED (2026-08-23): later**

**Decision: v1 ships a root-only key file. The TPM is deferred, with the seam built.**
Recorded as P4-a.

The store is encrypted either way — that part was never the question. What was decided is
*where the key lives*, and the answer is a 32-byte file at `/var/lib/faciolock/key`, mode
0600, created with `create_new` + `mode` so it never exists at a looser mode for even one
syscall.

**What that buys and what it does not.** A copied `.tpl` is worthless, which closes A3's
theft half against anyone who grabbed the template. A full disk image is not stopped, because
the key is on the same disk. wiki/03 R6 states the reduced guarantee plainly rather than
implying a protection that is not there.

**Why the seam and not the feature.** `store::KeyProvider` is an enum whose discriminant is
written into the file *and* into the AEAD's associated data. Adding `Tpm = 1` later produces
a file that old code refuses rather than one it silently misreads, so deferring costs a
format migration of exactly nothing.

**What would re-open it.** Not "we have time" — a threat that the key file does not answer.
The obvious one is a machine that leaves the owner's control while powered off, which is the
laptop case this project targets, so this decision is worth revisiting before the PPA release
rather than after.

**The packaging argument turned out not to bind.** It was assumed that TPM meant carrying a
`tpm2-tss` native dependency against Q2's grain. Checked 2026-08-23:
`librust-tss-esapi-dev 7.5.1-4` and `libtss2-dev 4.1.3-6` are both **in the Ubuntu archive**,
so the TPM path costs one build-dependency, not the archive story. It is still a C library
linked into the process that parses attacker-controlled pixels, which is a real cost and the
one that survives — but the packaging objection does not, and the record should not keep
citing it.

The reference machine has TPM 2.0 at `/dev/tpmrm0`, so whenever this is picked up the
hardware is there to develop against.

### PCR selection, for whoever picks this up

Unchanged and still the trap that made this a question. Binding to PCR 7 (secure-boot state)
adds protection against booting a live USB on the owner's own laptop, and costs a **silent**
re-enrolment on every firmware update or secure-boot key rotation — silent because nothing
fails loudly; face unlock simply stops working and the password still does. Binding to
nothing meaningful still binds to the *chip*, which is the machine-theft threat, and survives
updates. Sealing with no PCR policy is the conservative answer, not the weak one; PCR 0 or 4
would break on every kernel update and should not be considered.

---

## Q6 — Target: Debian archive, or PPA and see? — **DECIDED: archive**

**Decision (2026-08-03): the Debian/Ubuntu archive is the goal, and the design is
shaped around it from commit one.**

Consequences, now binding:

- InsightFace / ArcFace pretrained weights are out. Non-negotiable.
- Pure-Rust inference (`tract`) is strongly preferred over `ort`'s vendored C++
  onnxruntime — this pre-loads Q2 toward `tract`, pending verification that it can
  actually load our models.
- Models ship as a separate binary package with vendored licenses.
- Reproducible builds, opt-in `pam-configs` profile, `systemd-analyze security`
  score, and the full [upstreaming checklist](07-desktop-and-packaging.md#the-upstreaming-checklist)
  are requirements, not nice-to-haves.
- Sequencing is still PPA → ITP → archive; a track record is a prerequisite, not a
  detour.

### Original framing

The archive goal (G7) is what rules out InsightFace's models and pushes toward pure-
Rust runtimes and away from vendored dependencies. It is a real constraint on the
technical design, and it is slow — ITP, Rust crate packaging, security review, likely
a year+.

If the archive is a "someday, maybe", several constraints relax and the project moves
faster. If it's the actual goal, it should shape decisions from commit one. Worth
being explicit now, because retrofitting license and packaging cleanliness later is
much more expensive than starting with it.

---

## Q7 — Scope of v1?

**Thin slice**: `sudo` only, one hardcoded camera, no TPM, no rate limiting, no KCM
— but end-to-end real, on your actual laptop, in ~2 weeks. Proves the hard parts
(PAM plumbing, IR capture, strobe liveness) early.

**Full vertical**: all PAM services, enrollment CLI, rate limiting, sandbox, packaging
— ~2 months before anything works.

Thin slice first is almost certainly right; the risk is entirely in the hardware and
the liveness scheme, and both get exercised on day one.

---

## Q8 — Does your laptop actually have an IR camera?

Everything above assumes yes. If not, the whole differentiating story (active strobe
liveness) can't be developed or tested locally, and the project becomes RGB-only —
which is Howdy's security level with better engineering.

Run this and paste the output:

```bash
for d in /dev/video*; do echo "=== $d"; v4l2-ctl -d "$d" --info --list-formats 2>/dev/null | grep -E "Card type|\[[0-9]\]"; done
```

A device offering `GREY` at a squarish low resolution (340×340, 400×400, 640×360) is
the IR camera. Install `v4l-utils` if `v4l2-ctl` is missing.

---

## Q9 — Name

`Faciolock` is the directory name. Before anything is published: check for collisions
on crates.io, in the Debian archive, and for trademarks. Cheap now, expensive later.

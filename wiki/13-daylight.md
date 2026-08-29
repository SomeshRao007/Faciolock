# 13 — Daylight

The daylight story is currently spread across [01](01-goals-and-scope.md) (the non-goal),
[B18](11-bug-log.md#b18) (why it broke), [B20](11-bug-log.md#b20) (why the fix is unsafe),
[B21](11-bug-log.md#b21) (why the calibration failed) and [12](12-falloff-calibration.md)
(the readings). Four documents, one question. This page holds the question and ranks the
ways out. It is a **research and sequencing page** — it changes no threshold and licenses
no device write.

---

## The thing that is easy to get wrong

**The camera sees the face perfectly well in daylight. What fails is proving it is alive.**

Those are two different subsystems and only one of them is broken:

| | daylight status |
|---|---|
| Camera delivers frames | works |
| Emitter strobes | works — measured in daylight, never misses a beat ([B18](11-bug-log.md#b18)) |
| YuNet finds the face | works — verified, and the face-box emitter response is 2.2× the whole-frame one |
| SFace would recognise it | untested in daylight, but nothing about it depends on ambient |
| **Liveness proves it is a face and not a photo** | **this is what fails** |

So "the IR camera is physically detecting my face, why is daylight a problem" is the right
observation with the wrong conclusion attached. Detection is not liveness. A printed
photograph also gets detected — that is the whole point of a printed photograph.

Every liveness statistic the project started with was a **ratio** of emitter-lit to dark.
Ambient near-infrared adds a pedestal to both, and a ratio saturates against a pedestal:
15,000:1 after dark becomes 1.15:1 at 9am, while the emitter's own contribution barely
moves. The refusal in daylight is the system declining to guess, not the system going blind.

The one-line proof that this is solvable in software on this hardware: **Windows Hello
authenticates in the same room, on the same laptop, through the same sensor, emitter and
glass.** Optics cannot differ from themselves.

---

## What is actually blocking it

Three things, and they are not equally hard.

### 1. The negative class does not exist — this is the real blocker

`falloff_cv` is the check that should carry daylight. It runs on the ON−OFF **difference**,
where the ambient pedestal cancels exactly, leaving the emitter's inverse-square pattern
across the face box — subject *geometry*, which ambient light cannot change. That
invariance is measured, not just derived: the daylit live band (CV 0.307–0.385, including a
capture at pedestal 51.7) sits **inside** the nocturnal band (0.266–0.348).

It enforces a floor of **0.05**. That number is a degenerate-flatness floor — it rejects a
perfectly uniform image and claims nothing else — because **no printed face has ever been
recorded**. Opening the gate today would trade a *measured refusal* for an *unmeasured
acceptance*.

### 2. The ambient gate keys on a quantity the attacker sets ([B20](11-bug-log.md#b20))

Presenting a bright object 40 cm from the lens collapses the measured floor 2,170×. The
operator's own face read 28.221 and was **gated**; a ceramic mug in the same minute read
0.013 and **passed**. The AGC shortens exposure for a bright near object and the pedestal
follows.

**Consequence for how this is described:** the ambient gate is **not currently a security
control.** It is a UX hint that produces an honest "too bright to verify" for honest users,
and an attacker walks under it. The load-bearing layer for the presented-object class is
`dark_ratio_max`, alone, at a **1.8× margin** (1.79e-3 against 1e-3). That should be said
plainly anywhere the gate is relied on, and it should be said before PAM is built on top of
it.

### 3. `dark_ratio_max` cannot survive daylight and has no replacement

It is a ratio — `mean(OFF)/mean(ON) > 1e-3 → reject` — and it reads **0.664 against 0.001**
on an honest daylight capture, rejecting 100% of them. Four candidate replacements were
measured; the daylight floodlight recording refuted all four, three of them by ranking the
attack as *more* live-looking than a live face. This is the finding that stands: **no
magnitude statistic detects IR injection at high ambient.** Falloff cannot cover for it —
falloff reads geometry and cannot see injected IR.

### The strobe *pattern* survives daylight; the *ratio* does not

Measured 2026-08-23 at 15:21 IST in the operator's room, by `faciolockctl diagnose` — 30
frames, whole-frame mean luma, the same statistic wiki/10 Probe 3 recorded at night:

```text
daylight, 15:21   69.660  18.855  77.518  21.064  77.433  21.263  77.518  21.429  …
after dark, 22:0x 53.922   0.000  57.930   0.001  57.528   0.001  57.220   0.001  …
```

Both readings are the same command on the same machine the same evening, which is what makes
the pair worth more than either number alone:

| | wiki/10 Probe 3 (night) | daylight, 15:21 | after dark, same day |
|---|---|---|---|
| OFF floor | ~0.004 | **~21** | 0.000–0.001 |
| ON level | ~54–62 | ~77 | ~54–58 |
| ratio | 14,000:1 (100,000:1 on 3b) | **3.5:1** | **89,857:1** |
| alternation | perfect from frame 5 | **1.00** | **1.00** |

**The emitter is not what changed. The floor is.** Ambient infrared lifts the dark frames by
four orders of magnitude and the ratio collapses with them — which is [B18], seen from a new
angle and confirmed by a second statistic. The ON level barely moves (77 against 54–58) and
the OFF floor moves by everything. But the *alternation is untouched*: **1.00 in both runs**,
not one missed beat, across a 25,000× swing in the ratio.

That difference is load-bearing rather than decorative. `dark_ratio_max` is a magnitude
statistic and it is the check daylight kills. The **phase** is not a magnitude, and nothing
measured so far suggests ambient light disturbs it: ambient adds a constant to both halves,
which moves a ratio and leaves a pattern where it was.

Two consequences, one taken and one not:

- **Taken.** `faciolockctl diagnose` gates its tier verdict on the alternation and treats the
  ratio only as a sanity floor (P5-e). A classifier keyed on the ratio would have answered
  "no strobe, this must be an `ir-constant` machine" in this room, in daylight, with the
  emitter strobing perfectly — and writing that tier is the demotion P1-e exists to prevent.
- **Not taken, and it is not a liveness check yet.** An "is it alternating" check is not a
  replacement for `dark_ratio_max`, because an attacker who can inject IR can inject it
  *strobed*. Section 2 above is the reason: the gate must not key on a quantity the attacker
  sets, and phase is such a quantity once they know the period. What this measurement
  establishes is narrower and worth writing down anyway — **there is a strobe statistic that
  daylight alone does not destroy**, which is more than was true this morning.

---

## Candidate unlocks, cheapest first

### A. Record a printed face — hours, one errand, unblocks everything else

Status: **done 2026-08-19, in the dark. The answer is negative, and the daylight half of the
errand is still outstanding.**

A magazine cover was recorded at 23:45 IST with a same-session live control — pedestal 0.000
on all five takes, so this is the *nocturnal* set. The plan had it as the cross-check and the
bright-room set as the main event; they arrived in the opposite order.

What it settled, at night: **falloff CV does not separate a live face from a flat print.**
0.2733 against a worst live of 0.2661 — above it. A printed face clears every enforced
liveness check
([B24](11-bug-log.md#b24)). Outcome 2 of the two listed at the end of this section, which
means falloff alone is insufficient and this page's central question does not have the cheap
answer.

What it did **not** settle: anything about daylight prints, because there are none. And
`ctr/edge`, which appeared to separate — 0.876–0.918 for prints against 1.446–1.774 for live —
turns out to separate by ~0.2% rather than 0.53 once measured at the burst length the daemon
actually uses, on one print object and one human subject ([B24](11-bug-log.md#b24)). No
threshold was added, advisory or otherwise.

**The remaining errand, re-ranked by what the review actually found:** a **second live
subject** first — it needs no printer, no errand and no purchase, and every live number in
this project comes from one person's head. Then a second print object, propped rather than
hand-held and sized to match the live control's box. Then the whole set in the bright room,
which is the only thing here that speaks to daylight.

[B21](11-bug-log.md#b21) is the correction that matters here. **It supersedes B18's
recording list**, which said "no printer and no printed face are required — any flat object
is a valid negative." That was wrong, and a day of recording proved it wrong:

- A blank flat page has **no face**, so YuNet returns no box and the region of interest has
  to be *chosen*. Across the 501 placements lying fully on the page, CV spans **0.036 to
  1.71** — wider than the entire live band. The choice decides the answer, not the physics.
- A blank page is also an **optimistic** stand-in, not a conservative one. A printed face
  carries albedo structure — dark brows, bright forehead, shadowed sockets — and albedo
  enters the ON−OFF difference exactly like geometry does, because the check cannot tell
  them apart. A real print should read *higher* than a blank page, i.e. closer to live.

So what is needed is a **flat, head-sized, printed face**, so that detection supplies the
ROI the way it would in the running pipeline.

**No printer is required, and the face need not be the enrolled user's** — falloff asks
whether a surface is flat, not who is on it. Any of these work: a magazine cover, a film
poster, a product box with a face on it, a page from a print shop, a book jacket. Held at
~40 cm, filling the frame the way a head does, in the bright room.

The set that closes it, all in the high-ambient room, via `tools/record-negatives.sh`:

| take | label | what it settles |
|---|---|---|
| live control, this session's room | `live_bright_control` | pins the live band for *today's* light, so the flat takes are compared against a same-session baseline |
| flat printed face | `flat_printed_face` | the calibration point. The load-bearing one |
| the same print, curved over a mug or a head-shaped form | `nonflat_printed_face_curved` | the presentation designed to *manufacture* falloff — the likeliest to reach the live band |
| the print **plus** the IR remote | `attack_flat_floodlight` | the only capture that speaks to standing `dark_ratio_max` down |
| tablet at full brightness | `spoof_tablet_bright` | costs 90 s, calibrates nothing — the corpus has no daylight screen capture |
| the same flat print at night | `flat_printed_face`, re-run via `record-negatives.sh 2` | cross-check: CV appears to *rise* with ambient, the direction that could push a flat surface past a nocturnally-set floor. A separate session — one ambient description per matched set |

The prefixes are load-bearing, not cosmetic: `ambient_corpus.rs` scores `flat_*` and
`nonflat_*` as separate classes, because pooling a ceramic mug into the flat margin is how
the 2026-08-09 run reported a number that meant nothing ([B20](11-bug-log.md#b20)).

**A colour print may simply not be detected.** Many CMYK inks are weak or near-transparent
at the emitter's NIR wavelength, so a magazine face can wash out and return no box. That is
a real outcome, not a failed errand — a print refused at *detection*, the way screens
already are — and the test reports which box each row used rather than assuming.

A screen cannot substitute. `spoof_phone` reads `no face`, because the IR camera sees the
emitter reflected off the glass rather than the displayed image — screens are refused by
detection and never reach the check.

**Outcomes and what each one means.** Both are useful; only one of them is bad news:

- Print CV lands well under the live minimum of 0.2661 → set the floor between them with
  margin, move the gate behind falloff, daylight opens.
- Print CV overlaps the live band → falloff alone is not sufficient, and Phase 0's S5 exits
  with the answer it was written to allow ("or evidence that falloff alone is insufficient
  and passive texture checks are also needed"). That is a real result, reached for the price
  of one afternoon, and it is far cheaper to learn now than after PAM ships.

### ~~B. Read per-frame exposure from the metadata node~~ — **dead, measured 2026-08-09**

Status: **refuted.** [10/Probe 8](10-hardware-probe.md#probe-8--the-metadata-node).

The idea was that `MSXU_CONTROL_METADATA` might carry
`KSCAMERA_METADATA_CAPTURESTATS` with an exposure time, which would let ambient irradiance
be computed as pedestal ÷ integration time — a gate input the AGC, and therefore the
attacker, no longer sits between.

The node was streamed. Over 30 frames it produced **29 `FrameIllumination` blobs and zero
`CaptureStats`**. This firmware does not report its exposure time, so the quantity option B
was built on does not exist here. **B20 does not get fixed this way**, and the ambient gate
stays what [13's blocker 2](#2-the-ambient-gate-keys-on-a-quantity-the-attacker-sets-b20)
says it is: a UX hint, with `dark_ratio_max` load-bearing alone at 1.8×.

**The consolation prize turned out to be the real find.** The firmware tags every frame
ON or OFF, in `DISABLED` mode, without being asked — hardware ground truth for the phase
that `frame::classify` currently infers from luma. Validated against the strobe: 26 frames,
perfect alternation, mean(ON) 99.90 against mean(OFF) 74.33, zero disagreements. That
removes the 3.5-levels-of-headroom fragility [B18](11-bug-log.md#b18) recorded, and it is a
**robustness** win rather than a daylight one. It needs its own decision entry before it is
built, because taking phase from the device means trusting the device about something the
pipeline currently derives for itself.

### C. Point the auto-exposure metering region at the face box — unexplored, and it needs its own probe

Status: **speculative. Do not act on this section without probing it first.**

[10](10-hardware-probe.md)'s control listing recorded that the IR path exposes "no exposure
or gain controls at all, only `region_of_interest_*`". The `region_of_interest_*` half of
that sentence was written off and never followed up. In UVC 1.5 that control family exists
precisely to tell the camera **which part of the frame to meter on**. B20 is the AGC
metering on the attacker's prop because the prop is the nearest bright thing. Metering on
the detected face box instead is at least the right shape of answer.

Current state on this device:

```text
region_of_interest_rectangle   0x00981ae1 (rect)      value=(0,0)/1x1  flags=has-payload, has-min-max
region_of_interest_auto_ctrls  0x00981ae2 (bitmask)   max=0x00000001   value=0x00000010
```

Three reasons this is a probe and not a plan:

1. **The firmware's own reporting is inconsistent** — `value` (0x10) is outside `max`
   (0x1). Something is wrong in either the descriptor or the driver's reading of it, and
   that has to be understood before anything is written.
2. **The bit meanings are not recorded here on purpose.** They must be read out of the UVC
   1.5 spec (`CT_REGION_OF_INTEREST`, `bmAutoControls`) and quoted, not recalled. This wiki
   has already paid once for correct-sounding physics that nobody checked
   ([B18](11-bug-log.md#b18)'s 940 nm story).
3. **It is a device *write*,** and [Q3](08-open-questions.md#q3--ir-emitter--resolved-no-emitter-code-at-all)
   deliberately removed device writes from this project, along with the USB-bus-hang hazard
   that comes with them. It is a *standard* UVC control rather than a blind vendor
   extension-unit sweep, which is a materially lower risk than the Sonix XU — but "lower"
   is not "none", and it deserves its own probe entry and its own decision line.

If it works it is worth a lot: it attacks B20 at the mechanism rather than at the symptom.
If it is a no-op it costs one afternoon, like `ALT_FRAME_ILLUM` did.

### D. Landmark-anchored falloff — after A, never before it

Status: **idea only.**

`falloff_cv` throws away everything except the spread of the 6×6 cells. The emitter's
inverse-square dome over a real face has a *location*: the brightest cell should sit on the
nose, and YuNet already returns the five landmarks that say where the nose is. A check that
asks "is the difference-image maximum where the nose is" uses a prior a flat print cannot
satisfy and a curved print has to work much harder to fake — and it costs nothing extra to
compute, since both the difference cells and the landmarks are already in hand.

**This must not be built before A exists.** [B21](11-bug-log.md#b21) is exactly the failure
of tuning a liveness statistic with no negative class to validate it against; a second,
cleverer statistic calibrated the same way would be the same bug with a better story.

### E. Passive NIR reflectance / texture — the fallback if A comes back badly

Status: **literature, not measurement. Recorded so it is not re-derived from scratch.**

Skin and printed matter do not reflect near-infrared the same way, and this is the standard
basis for passive NIR spoof detection in the literature. Two things follow, and only the
second is safe to lean on:

- Any specific claim about how a given ink or paper behaves at this sensor's wavelength is
  **unverified here** and must not be quoted as if it were measured. The project has one
  sensor, one band, and no spectrometer.
- The corpus can answer it empirically without any of that theory: the same recordings
  from A, run through a texture statistic instead of a geometric one, say whether the
  separation exists on *this* hardware.

Which is the point — this option also waits on A. Nearly everything does.

---

## What is not the blocker

These were each proposed, investigated, and refuted. Recorded so they are not proposed
again.

| Proposal | Why not |
|---|---|
| The sensor is the wrong hardware for daylight; needs 940 nm behind a bandpass filter | **Refuted by its own premise.** Windows Hello succeeds in that room on this same laptop. Optics cannot differ from themselves ([B18](11-bug-log.md#b18) correction) |
| The camera's face-authentication mode would lower the ambient floor | **Refuted twice by measurement.** `ALT_FRAME_ILLUM` sets, persists, and moves the floor not at all — the mode's own repeat spread exceeds the between-mode gap ([10/Probe 7](10-hardware-probe.md#probe-7--msxu-face-authentication-mode)) |
| Just shorten the exposure | **Not available.** The IR path exposes no exposure or gain control through standard UVC. The AGC is internal and opaque. (Option C is about *where it meters*, not about setting it) |
| Use the RGB camera for liveness where IR cannot | **Refuted.** It has an IR-cut filter and cannot see the emitter at all — strobe SNR 0.85 against the IR camera's 1,500–15,000 ([P2-h](README.md#decisions-so-far)) |
| Find a better magnitude statistic | **Refuted, four times over.** Ratios, SNRs and responses all carry the pedestal and converge as it rises. Three of the four rank a daylight attack as more live-looking than a live face |
| Average liveness across the burst to get a cleaner signal | **Refuted before it was tried.** [B9](11-bug-log.md#b9) measured a 1-in-36 bypass from exactly that. Liveness aggregates conjunctively |

---

## Sequencing: does this block Phase 3?

**Mostly no, and the reason is that the two failures live in different crates.** Daylight is
a `faciolock-vision` calibration problem. Phase 3 is `faciolock-pam` plus `faciolock-ipc` —
FFI safety, privilege checks, remote-session rejection, cancellation. None of that changes
based on where a falloff threshold lands.

Two couplings are real, and one of them is already handled:

- **The IPC contract freeze.** [B18](11-bug-log.md#b18) required the environment verdict
  category to exist before the wire is frozen. It does, and the wire is deliberately narrow
  — `faciolock_ipc::Verdict` collapses `Environment` to `Unavailable` and carries **no
  counting policy at all**. That is the right split and it should stay that way:
  **categories on the wire, failure-budget policy in the daemon**, so [B20](11-bug-log.md#b20)'s
  re-decision of the `Environment` exemption in Phase 4 is a daemon change and not a
  protocol break.
- **The story PAM is built on.** Before `PAM_SUCCESS` has a construction site, the wiki has
  to say — and now does, above — that the ambient gate is a UX hint rather than a security
  control, and that `dark_ratio_max` is load-bearing alone at 1.8×.

**The daemon was written on 2026-08-09 and criterion 4 is met**
([09](09-roadmap.md#phase-2--camera--daemon-skeleton-week-3)). Its first end-to-end run
found [B22](11-bug-log.md#b22) — two crates each discarding the AGC settle frames, one
usable frame out of six, every live authentication failing — which 119 passing tests had not
seen, because the seam between two correct crates has no corpus. That is the argument for
running a criterion rather than reasoning about it.

What that run also showed, in one line: `Op::Auth` → `Environment { ambient_floor: 85.44 }`
→ `Unavailable`, in 365 ms. **A new corpus maximum**, well over the previous 51.74, in an
ordinary afternoon room. The plumbing is complete and the daylight refusal is now the only
thing between it and a working `sudo` unlock.

So Phase 2's criterion 2 is the last one open, and option **A** above is what closes it.
The order is: **record the printed face** → calibrate `falloff` → Phase 3. Only the first
step is blocked on anything, and what it is blocked on is an errand.

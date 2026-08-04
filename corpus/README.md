# Corpus

Raw IR captures from the reference machine. This is the only Phase 0 artifact that
survives into the final tree ([wiki/09](../wiki/09-roadmap.md)) — the spike code is
throwaway, the recordings are not. Everything in
[wiki/05](../wiki/05-vision-pipeline.md) that claims to reject a spoof gets replayed
against these files in CI.

## Format

Each capture is two files sharing a basename:

| File | Contents |
|---|---|
| `<label>_<utc>.gray` | raw Y800, 640×360, one byte per pixel, frames back to back, no header |
| `<label>_<utc>.json` | device, geometry, frame count, and what was in front of the camera |

`frames_actual` in the sidecar is derived from the size on disk, not from what was
requested — ffmpeg can deliver short.

A file is `width * height * frames` bytes exactly. `spikes/probe` refuses anything that
is not a whole number of frames rather than guessing at the geometry.

## Recording

```bash
tools/record.sh <label> [frames] [note]
```

The recorder resolves the IR camera by `by-path` and verifies it offers `GREY` at
640×360 before recording — `by-id` collides on this hardware and resolves to the RGB
camera ([wiki/10 Probe 4](../wiki/10-hardware-probe.md#probe-4--device-node-disambiguation)).

90 frames ≈ 3 s ≈ 45 ON/OFF pairs ≈ 20 MB. Budget ~20 MB per take.

**Do not record with plain `ffmpeg -f rawvideo`.** It duplicates frames and destroys
the ON/OFF pairing without any visible symptom —
[wiki/10 Probe 3c](../wiki/10-hardware-probe.md#probe-3c--recording-to-disk-duplicates-frames-unless-you-stop-it).
`record.sh` passes `-fps_mode passthrough` for this reason.

## Analysing

```bash
spikes/probe/target/release/probe corpus/<file>.gray
```

Reports tier, per-frame ON/OFF classification, and per pair: `D`, `ON/OFF`, `OFF/ON`,
falloff CV, centre/edge ratio. It prints numbers and no verdict — thresholds come from
the data, not the other way round.

## Recording a spoof — the protocol

The negative class is what the whole security claim rests on, and a badly recorded spoof
is worse than none: it produces a comfortable number that means nothing. Four rules apply
to every spoof take.

**1. Record a fresh `live_lit_frontal` in the same session, first.**

This is the most important instruction on the page. The OFF floor moved **10× between two
sessions on the same machine** from ambient light alone
([wiki/10 Probe 3b](../wiki/10-hardware-probe.md#probe-3b--repeat-run-24-hours-later)), and
the ON series droops 13% within a single capture. Comparing today's spoof against a live
capture from a different day compares two things that differ in more ways than the one
being tested. The control take costs three seconds.

**2. Same room, same lighting, same position for the control and the spoof.**
Change exactly one thing: what is in front of the camera.

**3. Present the spoof at the size a real face appears.** A liveness check that only
rejects a spoof held too far away has not rejected anything. Match the *apparent* size —
what fills the frame — not the physical distance.

**4. Present the strongest version of the attack.** Maximum screen brightness, the
best-quality print, the most convincing photo. A defence that only beats a weak attack is
not a defence, and there is no credit for making the attacker's job harder than it is.

### `spoof_phone` — a screen showing your face

What it tests: the **response test**. An LCD or OLED emits essentially nothing at ~850 nm,
so `D = mean(ON) - mean(OFF)` over the face should collapse toward zero while a live face
reads 53–75.

Setup:

- A frontal, well-lit photo of your own face — ideally one that *would* match if it were
  real. A weak photo tests nothing.
- Full screen, no browser chrome, no dark-mode filter. **Screen brightness at maximum**,
  and turn off adaptive/auto brightness so it does not dim mid-take.
- Hold the phone so the displayed face fills roughly what your real face fills. A phone
  screen is about half a head wide, so that is roughly half your normal distance — around
  20–25 cm rather than 40–50 cm.
- Hold steady for the full three seconds.

```bash
tools/record.sh live_lit_frontal 90 "control take, same session as spoof_phone"
```
```bash
tools/record.sh spoof_phone 90 "phone at max brightness, frontal photo, ~25cm, lit room"
```

Worth a second take with the phone at normal distance (~45 cm), labelled in the note — the
two distances bracket the attack and cost one command.

### `spoof_print_matte` / `spoof_print_glossy` — a printed photo

What it tests: the **falloff test**, and this is the one that matters. A print passes the
response test — paper reflects NIR perfectly well — so falloff is the only check standing
between a printed photo and a successful spoof. [wiki/09](../wiki/09-roadmap.md) lists a
falloff failure as a risk that forces a liveness redesign, so the sooner this exists the
cheaper it is to be wrong.

Setup:

- **Print at life size.** A face is roughly 14 cm wide and 20 cm tall, so print at A5 or
  larger and hold it where your face normally sits. This matters more here than anywhere
  else: falloff is a depth measurement, and a small print held close has different depth
  geometry from a life-size print at arm's length. A wallet-sized photo is the wrong test.
- Frontal, sharp, well-lit source photo. Print it as large as the paper allows.
- **Both finishes.** Matte and glossy differ in NIR: glossy adds a specular highlight that
  matte does not, and either could pass or fail for reasons the other does not.
- Keep it flat. A curled print has curvature, which is the very thing the check looks for —
  a bowed print is a *different* and more interesting attack, worth its own take with a
  note saying so, but it is not the flat-print test.

```bash
tools/record.sh live_lit_frontal 90 "control take, same session as the prints"
```
```bash
tools/record.sh spoof_print_matte 90 "A5 matte, life size, flat, ~45cm, lit room"
```
```bash
tools/record.sh spoof_print_glossy 90 "A5 glossy, life size, flat, ~45cm, lit room"
```

Plain office paper is worth one more take if it is easy — it is more diffuse than photo
paper and may behave differently again.

### Checking a take before you trust it

```bash
cargo test --release -p faciolock-vision --test liveness_corpus -- --nocapture
```

New captures are picked up automatically from their sidecars. Read the
`class_separation_report` output: it groups every capture by its `<class>_` prefix and
prints the range each metric spans. **The gap between the `live` row and the `spoof` row is
the separation that check provides.** If the ranges overlap, that check does not work and
no threshold will rescue it — which is a real result and needs recording rather than
retrying until it looks better.

Sanity checks on the take itself, before reading any of that:

- `pairs` should be ~42. Far fewer means the capture did not alternate, and a spoof that
  fails to form a burst is being rejected for the wrong reason.
- For `spoof_print_*`, `face` should read `yes` — a print the detector cannot find is not
  testing the falloff check at all, it is testing the detector.
- For `spoof_phone`, `face: no` is a plausible and interesting outcome: a screen may be too
  dark in NIR for YuNet to find anything. Note it either way.

## Naming

`<class>_<condition>_<variant>` in `[a-z0-9_]`:

- `live_lit_frontal`, `live_dark_frontal`, `live_lit_glasses`, `live_lit_angle_left`
- `spoof_print_matte`, `spoof_print_glossy`, `spoof_phone`, `spoof_laptop`
- `attack_floodlight_remote`
- `null_empty_room`

The `<class>` prefix is what CI groups on, so it has to be right: `live_*` must pass,
`spoof_*` and `attack_*` must fail.

## What is here, and what is missing

**Recorded:** live face across lighting and angle; IR floodlight attack (an ordinary
remote pointed at the sensor).

**Missing — and it matters:** no printed photo, no screen. Protocol for recording both is
above; it is the negative class every liveness claim past dark-frame integrity needs.

[wiki/09](../wiki/09-roadmap.md) names the falloff-vs-print separation as *the one
genuinely unproven claim in the project*. The response test (`D≈0`) that rejects screens
and the dark-frame check that rejects floodlights are both easy; the print is the hard
case, because a print **passes** the response test and only the falloff test stands
between it and a successful spoof.

Until a `spoof_print_*` recording exists, that claim is asserted rather than
demonstrated, which is the standard [wiki/05](../wiki/05-vision-pipeline.md) sets for
itself:

> This is the security claim of the project and it must be tested, not asserted.

The live-face recordings establish the positive-class distribution, so adding the
negative later costs one 3-second take and one command. **Liveness thresholds must not
be finalised before then.**

## Not in version control

Raw captures are large and contain the operator's face. Add `corpus/*.gray` to
`.gitignore` when the repo is initialised, and keep the `.json` sidecars — they are
small, they carry no biometric data, and they document what was recorded.

CI needs *some* corpus to replay. Options when that becomes real: a small committed
subset from a consenting contributor, or a release-asset tarball fetched by the test
harness. Not decided.

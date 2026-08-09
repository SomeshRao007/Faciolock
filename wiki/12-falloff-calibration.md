# 12 — Falloff Calibration Log

Measurement log for the falloff check ([05](05-vision-pipeline.md)). Append-only, in the
same spirit as [10](10-hardware-probe.md): what was recorded, what the numbers were, what
remains undetermined. Kept separate from [11](11-bug-log.md) because most of what is here
is measurement rather than defect.

The open question is Phase 0's S5 ([09](09-roadmap.md)): does the falloff check separate a
live face from a flat presentation? As of this writing it is not answered.

---

## What the check computes

The emitter sits beside the lens, so it acts as a point source at the camera, and intensity
falls off with the square of distance. Over a real face at ~40 cm the nose is a few
centimetres closer than the cheeks, so the emitter's contribution varies across the face
box. Over a flat surface at the same distance it varies much less.

Procedure: subtract the emitter-OFF frame from the ON frame (this cancels the ambient
pedestal exactly — see [B18](11-bug-log.md#b18)), tile the region of interest into a 6×6
grid, take the mean per cell, and report the coefficient of variation across cells.

The threshold currently in the code is 0.05, which is a degenerate-flatness floor. It was
not derived from any flat recording, and [05](05-vision-pipeline.md) records that the
printed-photo row is untested.

---

## Live distribution

Twelve captures with a detected face box, across three lighting regimes.

```text
class          n     min CV     max CV
nocturnal      7     0.2661     0.3481
daylit         4     0.3071     0.3847
```

The daylit band sits inside the nocturnal band. That is consistent with the check being
ambient-invariant by construction, and it includes a capture at ambient pedestal 51.7.

---

## Session 2026-08-09 (morning) — three presented objects

Recorded via `tools/record-negatives.sh` in the operator's bright room, one lighting
description across all takes.

Frames were reviewed after the fact. The objects recorded were a crinkled snack wrapper, a
bowed and tilted card, and a ceramic mug. None of the three is a flat surface held square to
the lens, which is what the takes had asked for. They were relabelled from `flat_*` to
`nonflat_*` so the filenames describe their contents, and their sidecar notes carry the
same annotation.

The mug is the only one of the three in which YuNet reported a face box; it read CV 0.6067,
above the live band. A cylinder is expected to read high, so this is not informative about
flat presentations either way. The mug is also the capture that prompted
[B20](11-bug-log.md#b20), which is about the ambient gate rather than about falloff.

---

## Session 2026-08-09 (evening) — a flat page

`flat_printed_page_20260809T064713Z`. A spiral notebook, blank lightly-striped pages, held
flat and square to the lens at roughly 40 cm. Ambient pedestal 7.898. Frame review confirms
the surface is flat, matte, light-coloured and not tilted.

Because a blank page contains no face, YuNet returns nothing and the region of interest has
to be supplied by the test rather than by detection. Three placements were tried, and the
result depends on which is used:

| ROI rule | CV |
|---|---|
| Fixed box at the median live position (296, 111) | 0.4489 |
| Box placed by maximising the weakest grid cell | 0.1085 |
| Every placement lying fully on the page (501 of them) | 0.0361 – 1.7116, median 0.2486 |

The first straddles the top edge of the notebook — frame review shows roughly a third of
that box falling on dark background above the page — so it includes a bright/dark step that
is not a property of the surface.

The second lies entirely on the page. It also, by construction, selects the most uniformly
lit patch available, which places it near the bottom of the achievable range.

The third is the distribution the other two are drawing single samples from. Its median,
0.2486, is close to the minimum of the live band, 0.2661.

**Status: undetermined.** The spread across valid placements overlaps the live band, so
these recordings do not establish whether the check separates the two classes.

---

## What would resolve it

In the running pipeline the region of interest is never chosen — YuNet supplies it. Against
a printed photograph of a face, YuNet would return a box on the printed face, and that box
is the one whose CV decides the outcome. Reproducing that requires a printed face; the
operator does not currently have access to a printer or to any printed face.

Two further considerations for whoever records it:

- A blank page carries no albedo structure. A photograph of a face carries a great deal —
  dark brows, bright forehead, shadowed eye sockets — and albedo variation contributes to
  the same statistic as geometric variation. A blank page is therefore expected to read
  lower than a photograph, and is a lower bound on the class rather than a sample of it.
- A photograph curved over a head-shaped form is the presentation designed to defeat this
  check specifically. It is untested and would need the same printed face.

Until then the 0.05 floor claims what [05](05-vision-pipeline.md) says it claims: that a
perfectly uniform image is rejected. Nothing about printed photographs.

---

## Method notes

`falloff_over_placements` in `crates/faciolock-vision/tests/ambient_corpus.rs` sweeps the
box across the frame at a 12 px step and keeps every position whose weakest grid cell has an
emitter response above 8, which is the condition for the box lying on the presented object
rather than across its edge. It returns the whole distribution rather than a single value,
and the test declines to print a separation verdict when the flat class comes only from
chosen placements. That behaviour was added after a run on 2026-08-09 reported a 2.45×
separation which came from the placement rule rather than from the data.

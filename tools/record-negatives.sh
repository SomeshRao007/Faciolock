#!/usr/bin/env bash
# Record the negative class -- the recordings that unlock daylight.
#
# The project has one genuinely unproven claim, Phase 0's S5, and it has been open since
# day one: does the falloff check separate a live face from a flat surface? `falloff` is
# ambient-invariant (wiki/11 B18, measured), so it is the check that can work in a bright
# room -- but it enforces only a degenerate-flatness floor of 0.05 against a live band
# bottoming at 0.2661, because **the corpus contains no flat presentation at all**. Until it
# does, the ambient gate cannot be rescoped: that would trade a measured refusal for an
# unmeasured acceptance.
#
# Five captures, ~8 minutes. Run it IN THE BRIGHT ROOM -- the one where Windows Hello works
# and Faciolock refuses. That is the regime in question, and CV appears to rise with
# ambient (0.3847 at pedestal 51.7 is the corpus maximum), which is the direction that
# could push a print past a floor set at night.
#
# **No printer, no printed face, no props to buy.** Falloff measures whether the emitter's
# response across a region is flat or domed -- surface geometry and IR reflectance. It does
# not care what is depicted, so the negative class is any RIGID FLAT object held square to
# the lens where a head would be: an open book pressed flat, a sheet taped to a hardcover.
#
# "Flat" is the whole experiment and it is easy to get wrong. The 2026-08-09 session
# recorded a crinkled snack wrapper, a bowed card and a ceramic mug. All three are 3D, none
# is falloff's threat model, and the run reported a meaningless margin until the captures
# were relabelled `nonflat_*` ([B20](../wiki/11-bug-log.md#b20)). If it creases, bows or
# curves, it measures nothing.
#
# What that costs, stated plainly: these captures have no face for YuNet to find, so they
# are measured over a **fixed** box (median of the corpus's twelve detected live boxes) and
# they cannot replay an end-to-end attack -- a real spoof must pass detection too, and these
# deliberately do not. What they *do* calibrate is the flatness floor, which is the single
# quantity blocking a gate rescope. The test labels those rows so the two never get
# conflated.
#
# **A screen is not a substitute.** The corpus already holds `spoof_phone` and it reads
# `no face`: the IR camera sees the emitter reflected off the glass, not the displayed
# image, so a screen is refused by *detection* and never reaches falloff. Take 5 records a
# tablet anyway because the corpus has no daylight version and it costs 90 seconds -- but it
# cannot calibrate the check, and if it is the only negative recorded, nothing is unblocked.
#
# Usage:  tools/record-negatives.sh            everything, prompting between takes
#         tools/record-negatives.sh 3          just take 3
#
# ponytail: a prompt loop over record.sh, which already handles the device, the sidecar
# and the -fps_mode passthrough that B5 needs. No new capture code.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RECORD="${HERE}/record.sh"

# label | frames | note | what to physically do
TAKES=(
"live_bright_control|90|frontal, ~40cm, control take for this session|Sit ~40 cm from the camera, face it. The positive control: it pins the live falloff band for TODAY'S room, so the flat takes are compared against a same-session baseline rather than against nocturnal numbers."
"flat_printed_page|90|LIGHT printed page, rigid and flat, square to lens, ~40cm|Open a book about ten pages from the back and rest the thick part in your palm, so the few remaining pages lie FLAT on top of it. Show the camera a LIGHT-COLOURED PAGE WITH TEXT. Hold it where your head was, ~40 cm, covering the middle of the frame. Square to the lens, not tilted. TWO TRAPS: (1) nothing that crinkles or bows -- the 2026-08-09 session recorded a wrapper, a bowed card and a mug, and none of them measured anything; (2) NOT a dark cover -- a print attack is a face on light paper, and a dark surface absorbs the emitter so the difference collapses into noise, which reads as HIGH CV and falsely looks live. THIS IS THE ONE THAT MATTERS."
"flat_blank_card|90|BLANK light page, rigid and flat, square to lens, ~40cm|Same book, same grip, but turn to a BLANK page -- the endpapers at the very front or back of most books are blank. Light-coloured again, rigid, square to the lens, same ~40 cm. Ink varies NIR reflectance and lifts CV, so the printed page is the realistic attack and a blank page is the floor beneath it. The pair brackets the flat class instead of guessing where inside it a real print sits."
"attack_flat_floodlight|90|flat printed page AND IR remote at sensor, button down|BOTH at once: the printed page at ~40 cm, and the IR remote at the sensor with the button held down. A spoof plus injected IR -- the only capture that speaks to standing dark_ratio_max down in bright light."
"spoof_tablet_bright|90|tablet at max brightness, frontal face photo, ~30cm|Tablet at maximum brightness showing a frontal face, ~30 cm. Expect NO FACE: the IR camera sees the emitter reflected off the glass, not the picture, so this is refused by detection and never reaches falloff. Recorded because the corpus has no daylight screen capture, not because it calibrates anything."
)

if [[ ! -x "$RECORD" ]]; then
    echo "error: $RECORD is not executable" >&2
    exit 1
fi

# Refuse a non-interactive stdin. This script turns a bare Enter into a live recording, so
# piping anything into it records the room without a person present to know it is happening
# -- which is how a junk 6-frame capture reached corpus/ on 2026-08-08 during a dry run of
# this very file. A camera tool must not be drivable by accident.
if [[ ! -t 0 ]]; then
    echo "error: this records from the camera and needs an interactive terminal." >&2
    echo "       Run it directly, not through a pipe or a heredoc." >&2
    exit 2
fi

# One ambient description for the whole session -- the takes must be comparable, and a
# session where the light changed between takes is not a matched set.
echo "These captures are only worth recording as a MATCHED SET, in the bright room."
echo
read -rp "Describe the lighting (e.g. 'big room, indirect sun ~10m, lights on'): " ambient
if [[ -z "$ambient" ]]; then
    echo "error: the ambient description is the one field B18 was caused by omitting" >&2
    exit 2
fi

only="${1:-}"
n=0
for take in "${TAKES[@]}"; do
    n=$((n + 1))
    [[ -n "$only" && "$only" != "$n" ]] && continue

    IFS='|' read -r label frames note instruction <<< "$take"

    echo
    echo "════════════════════════════════════════════════════════════════════"
    echo "  Take $n of ${#TAKES[@]}:  $label"
    echo "════════════════════════════════════════════════════════════════════"
    echo
    echo "  $instruction"
    echo
    read -rp "  Ready? [Enter to record, s to skip, q to stop] " answer
    case "$answer" in
        q|Q) echo "stopped"; exit 0 ;;
        s|S) echo "skipped $label"; continue ;;
    esac

    "$RECORD" "$label" "$frames" "$note" "$ambient"
done

echo
echo "Done. Now measure -- both of these read the corpus, neither needs the camera:"
echo
echo "  cargo test -p faciolock-vision --test ambient_corpus falloff -- --nocapture"
echo "  cargo test -p faciolock-vision --test liveness_corpus -- --nocapture"
echo
echo "The falloff test prints the margin directly: worst live CV vs best FLAT CV."
echo "Separated -> a flatness floor fits inside it, and a per-check gate rescope becomes"
echo "arguable. Overlapping -> falloff cannot carry liveness alone and the daylight"
echo "non-goal stands as written. Either way it is an answer, which is the point."

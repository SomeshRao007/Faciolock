#!/usr/bin/env bash
# Record the negative class -- the recordings that unlock daylight.
#
# S5 IS ANSWERED AND THE ANSWER IS NO (2026-08-19, wiki/11 B24). The magazine take this
# script asks for was recorded, and `falloff` does NOT separate a live face from a flat
# print: the print reads CV 0.2733 against a worst live face of 0.2661 -- above it, so no
# threshold exists at any value. A flat printed face clears every enforced liveness check.
# `centre_edge_ratio` looked like the replacement and did not survive checking either.
#
# So this script is no longer calibrating a floor. It is now the tool for the recordings
# that would settle what replaces falloff, and the takes below changed accordingly.
#
# TWO PROTOCOL RULES THE 2026-08-19 SESSION BROKE, both of which cost real conclusions:
#
#   - MATCHED APPARENT SIZE. That session's print gave a detected box of 58x72 px against
#     the same-session live control's 99x129 -- roughly half the linear size, which
#     corpus/README.md rule 3 already forbade. (Checking showed this did NOT by itself
#     explain the result -- within the live class a bigger box reads slightly *lower*, the
#     wrong sign -- but a protocol rule you cannot show is harmless is a rule you follow.)
#     Get the printed face CLOSE ENOUGH that its box is about the size a real head gives.
#   - NO HAND-HOLDING. The headline print number turned out to be the single worst pair of a
#     7.4 s hand-held capture that drifted; most pairs of that same capture read far higher.
#     Prop it, tape it, tripod it. The daemon judges a 0.25 s burst, so a capture whose value
#     depends on when you sampled it measures nothing.
#
# AND ONE THE SESSION DID NOT BREAK, BECAUSE NOBODY HAD EVER STATED IT: every live capture in
# this project is the same person. Take 2 below is a second subject, and it is the highest
# value recording available -- it needs no printer, no purchase and no errand.
#
# Six captures, ~10 minutes. Run it IN THE BRIGHT ROOM -- the one where Windows Hello works
# and Faciolock refuses -- because every print capture in the corpus is nocturnal (pedestal
# 0.000) and daylight is the regime that motivated the whole errand.
#
# **It must be a printed FACE, and that is the correction B21 cost a day to learn**
# (wiki/11 B21, wiki/13 section A). The earlier instruction here said any flat object would
# do because falloff does not care what is depicted. Both halves of that were wrong:
#
#   - A blank page has no face, so YuNet returns no box and this test has to *choose* the
#     ROI. Across the 501 placements lying fully on the page, CV spanned 0.036 to 1.71 --
#     wider than the entire live band. The choice decided the answer, not the physics.
#   - A blank page is also OPTIMISTIC, not conservative. A printed face carries albedo
#     structure -- dark brows, bright forehead, shadowed sockets -- and albedo enters the
#     ON-OFF difference exactly like geometry, because the check cannot tell them apart. A
#     real print should read HIGHER than a blank page, i.e. closer to live.
#
# The face need not be the enrolled user's -- these takes ask what a flat surface does, not
# who is on it. Detection then supplies the ROI the way it does in the running pipeline.
# **That is necessary but not sufficient**: a detected box is still ONE placement, and B24
# showed the same print spans roughly 0.37 to 1.57 across placements on its own surface. A
# take is only worth recording if the presentation is held steady enough that any 0.25 s
# window of it reads the same.
#
# "Flat" is the whole experiment and it is easy to get wrong. The 2026-08-09 session
# recorded a crinkled snack wrapper, a bowed card and a ceramic mug. All three are 3D, none
# is falloff's threat model, and the run reported a meaningless margin until the captures
# were relabelled `nonflat_*` ([B20](../wiki/11-bug-log.md#b20)). Paper bows under its own
# weight, so takes 2 and 3 need a rigid backing. If it creases, bows or curves, it is not a
# flat take -- which is what take 4 records deliberately.
#
# One thing worth knowing before reading the output: many CMYK inks are weak or transparent
# at the emitter's NIR wavelength, so a colour-printed face can wash out and return no box.
# That is itself a result -- a print refused at *detection*, the way screens already are --
# and the test reports which path each row took rather than assuming.
#
# **A screen is not a substitute.** The corpus already holds `spoof_phone` and it reads
# `no face`: the IR camera sees the emitter reflected off the glass, not the displayed
# image, so a screen is refused by *detection* and never reaches falloff. Take 6 records a
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
"live_bright_control|90|frontal, ~40cm, control take for this session|Sit ~40 cm from the camera, face it. The positive control: it pins the live band for TODAY'S room, so the print takes are compared against a same-session baseline rather than against nocturnal numbers. It also sets the SIZE the print takes have to match -- the 2026-08-19 session's print came in at half the linear box size of its own control, which made the two classes incomparable and cost the session its conclusion."
"live_second_subject|90|A DIFFERENT PERSON, frontal, ~40cm|THE MOST VALUABLE TAKE ON THIS LIST, and the only one that needs nothing but a second pair of hands. Every live number in this project -- all twelve captures -- is the same operator's head. Any floor derived from that is calibrated on one person's geometry and one skin tone in NIR, and the risk it carries is locking out real users, which no recording has ever tested. Ask anyone nearby to sit ~40 cm from the camera and face it. A flatter face, a beard, glasses, a different hairline, darker or lighter NIR skin response -- all of it is unsampled today."
"flat_printed_face|90|printed face, PROPPED not held, square, sized to match take 1's box|The print take, with the two things the 2026-08-19 session got wrong. (1) PROP IT -- tape it to a box, lean it against a stand. That session's headline number turned out to be the single worst pair of a 7.4 s hand-held capture that drifted; the daemon judges a 0.25 s burst, so a reading that depends on when you sampled it measures nothing. (2) SIZE IT -- move it close enough that the printed face is about as big in frame as your head was in take 1. That session's print came in at roughly half the linear box size of its own control, which corpus/README.md rule 3 already forbade. Rigid backing, square to the lens, not tilted."
"nonflat_printed_face_curved|90|printed face curved over a SPHERICAL head-sized form, apex centred|The same print curved over something ROUND and head-sized -- a ball, a balloon, a bowl -- not a mug. The 2026-08-19 take used a mug, which is a cylinder curved on one axis only with its apex placement uncontrolled, so it tested one arbitrary curvature rather than the attack. Aim the apex at the middle of the detected box. Recorded as nonflat_ so it is scored as its own class and never pooled into the flat margin."
"attack_flat_floodlight|90|flat magazine cover AND IR remote at sensor, button down|BOTH at once: the cover flat on its rigid backing at ~40 cm, and the IR remote at the sensor with the button held down. A spoof plus injected IR -- the only capture that speaks to standing dark_ratio_max down in bright light."
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
echo "READ THE OUTPUT LIKE THIS -- falloff is already answered, so it is not the question:"
echo
echo "  falloff CV   settled 2026-08-19. The print reads ABOVE the worst live face, so the"
echo "               classes interleave and no threshold exists. Expect overlap; it is not"
echo "               news, and it is not a reason to re-tune the 0.05 floor."
echo "  ctr/edge     the live-vs-print gap here is the thing under test, and it is thin:"
echo "               ~0.2% at the burst length the daemon uses. Look at take 2 FIRST --"
echo "               if a second subject reads below the existing live floor of 1.446,"
echo "               the floor was one person's geometry and there is nothing to gate on."
echo "  box sizes    if the print's detected box is not close to take 1's, the session"
echo "               broke corpus rule 3 and its numbers are weaker (wiki/11 B24)."
echo
echo "Still missing if this was not run in daylight: every print capture in the corpus is"
echo "nocturnal (pedestal 0.000), and daylight is the regime the whole errand was for."

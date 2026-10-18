#!/usr/bin/env bash
# Record a raw Y800 burst from the IR camera into corpus/ with a metadata sidecar.
#
# ffmpeg does the capture; this only resolves the device, names the
# output, and records the conditions so a file is still self-describing in a month.
#
# Usage:  tools/record.sh <label> [frames] [note] [ambient]
# e.g.    tools/record.sh live_lit_frontal
#         tools/record.sh spoof_print_matte 90 "glossy 6x4, held at 40cm" "night, lamp"
#
# ponytail: no argument parser, no --flags. Four positionals is the whole interface.

set -euo pipefail

CORPUS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/corpus"

WIDTH=640
HEIGHT=360
FPS=30
PIXFMT=gray          # V4L2 'GREY' / Y800, the only format the IR camera offers

label="${1:-}"
frames="${2:-90}"    # 90 frames; how long that takes is measured, not assumed -- see below
note="${3:-}"

# Ambient lighting. Recorded because it is the variable that decides how visible this
# hardware's emitter strobe is at all: a set of captures made only after dark, with nothing
# recording that fact, describes one lighting condition without saying so.
ambient="${4:-}"

if [[ -z "$label" ]]; then
    echo "usage: $0 <label> [frames] [note] [ambient]" >&2
    echo "  label must match [a-z0-9_]+  e.g. live_lit_frontal, spoof_print_matte" >&2
    exit 2
fi

if [[ ! "$label" =~ ^[a-z0-9_]+$ ]]; then
    echo "error: label must be [a-z0-9_]+, got '$label'" >&2
    exit 2
fi

if [[ ! "$frames" =~ ^[0-9]+$ ]] || (( frames < 1 )); then
    echo "error: frames must be a positive integer, got '$frames'" >&2
    exit 2
fi

# --- resolve the IR camera -------------------------------------------------
#
# by-path, not by-id: by-id collides between the RGB and IR interfaces of this
# USB device and resolves to the RGB camera.
#
# We do not trust the symlink blindly -- the format check below is what actually
# confirms we have the IR sensor.

device=""
for path in /dev/v4l/by-path/*-video-index0; do
    [[ -e "$path" ]] || continue
    if v4l2-ctl -d "$path" --list-formats 2>/dev/null | grep -q "'GREY'"; then
        device="$path"
        break
    fi
done

if [[ -z "$device" ]]; then
    echo "error: no capture device offering GREY found under /dev/v4l/by-path/" >&2
    echo "       list them with: v4l2-ctl --list-devices" >&2
    exit 1
fi

resolved="$(readlink -f "$device")"

# Confirm geometry too. A GREY device at the wrong resolution is not our camera.
if ! v4l2-ctl -d "$device" --list-formats-ext 2>/dev/null | grep -q "${WIDTH}x${HEIGHT}"; then
    echo "error: $resolved offers GREY but not ${WIDTH}x${HEIGHT}" >&2
    exit 1
fi

# --- record ----------------------------------------------------------------

mkdir -p "$CORPUS_DIR"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
base="${CORPUS_DIR}/${label}_${stamp}"

if [[ -z "$ambient" ]]; then
    echo "warning: no ambient description given -- pass a 4th argument, e.g. 'daylight, blinds open'" >&2
fi

echo "device : $device"
echo "         -> $resolved"
echo "format : ${PIXFMT} ${WIDTH}x${HEIGHT} @ ${FPS}fps requested"
echo "frames : ${frames}"
echo "ambient: ${ambient:-(unrecorded)}"
echo "output : ${base}.gray"
echo
echo "Get into position. Recording starts in 3s..."
sleep 3
echo "RECORDING"

# -fps_mode passthrough is load-bearing, not tidiness.
#
# Without it ffmpeg rate-converts to a constant 30 fps on the way out and duplicates
# frames to fill gaps. Measured: a 30-frame capture came back as 15 distinct sensor
# frames, each written twice. That turns the sensor's ON,off,ON,off into ON,ON,off,off
# and destroys the pairing every liveness check depends on -- silently, because the
# file is the right size and every individual frame is valid.
#
# A signalstats probe with -f null never hits this because it encodes nothing and
# so does no frame-rate conversion. Any capture written to disk needs this flag.
started=$(date +%s.%N)

ffmpeg -hide_banner -loglevel error \
    -f v4l2 -input_format "$PIXFMT" -video_size "${WIDTH}x${HEIGHT}" -framerate "$FPS" \
    -i "$device" \
    -frames:v "$frames" -fps_mode passthrough -f rawvideo -pix_fmt "$PIXFMT" \
    "${base}.gray"

elapsed=$(echo "$(date +%s.%N) - $started" | bc)

echo "done"

# --- sidecar ---------------------------------------------------------------
#
# Written after ffmpeg succeeds, so a sidecar always describes a real file.
# actual_frames is derived from size on disk, not from what we asked for --
# ffmpeg can deliver short.

size=$(stat -c%s "${base}.gray")
frame_bytes=$(( WIDTH * HEIGHT ))
actual=$(( size / frame_bytes ))

if (( size % frame_bytes != 0 )); then
    echo "warning: ${size} bytes is not a whole number of ${frame_bytes}-byte frames" >&2
fi

# fps_delivered, not fps.
#
# The old sidecar recorded "fps": 30 -- the value this script *asked for*, written as
# though it were an observation. It is not: the sensor delivered 14.7 fps,
# and `VIDIOC_G_PARM` reports the configured rate too, so nothing in the project had ever
# measured the real one. A capture that misdescribes itself is worse
# than one that says nothing, because the wrong number gets quoted.
fps_delivered=$(echo "scale=2; ${actual} / ${elapsed}" | bc)

cat > "${base}.json" <<EOF
{
  "label": "${label}",
  "recorded_utc": "${stamp}",
  "recorded_local": "$(date +%Y-%m-%dT%H:%M:%S%z)",
  "ambient": "${ambient}",
  "device_bypath": "${device}",
  "device_resolved": "${resolved}",
  "width": ${WIDTH},
  "height": ${HEIGHT},
  "pixel_format": "GREY",
  "fps_requested": ${FPS},
  "fps_delivered": ${fps_delivered},
  "elapsed_s": ${elapsed},
  "frames_requested": ${frames},
  "frames_actual": ${actual},
  "bytes": ${size},
  "note": "${note}"
}
EOF

echo "wrote  ${base}.json  (${actual} frames, $(( size / 1024 / 1024 )) MB, ${fps_delivered} fps delivered)"

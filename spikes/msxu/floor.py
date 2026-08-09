#!/usr/bin/env python3
"""Ambient floor and strobe amplitude of a Y800 capture.

Reports the same statistic the pipeline gates on, so numbers here are directly
comparable to wiki/11 B18's table:

    ambient floor = min over frames of the frame mean      (Burst::from_frames)
    phase         = each frame vs its immediate neighbours (frame::classify, post-B18)

Usage:  floor.py corpus/live_room_lit_*.gray      # analyse a recording
        floor.py --live 60                        # capture from the IR camera and analyse

ponytail: no numpy (not installed). sum() over a bytes object is C-speed and 90 frames
takes under a second.
"""

import statistics
import subprocess
import sys
import tempfile

W, H = 640, 360
FRAME = W * H

# Frames discarded while the AGC converges -- the tests' AGC_SETTLE, wiki/10 Probe 3.
# Validated: with this skip, live_room_lit reads 13.909 and attack_floodlight_room
# 14.052, matching B18's matched-pair table to three decimals. Without it they read
# 3.4 and 4.6, because the first frames are captured mid-ramp.
AGC_SETTLE = 5


def frame_means(path):
    with open(path, "rb") as f:
        data = f.read()
    n = len(data) // FRAME
    means = [sum(data[i * FRAME:(i + 1) * FRAME]) / FRAME for i in range(n)]
    return means[AGC_SETTLE:]


def classify(means):
    """Phase by comparison with immediate neighbours -- frame::classify after B18.

    A global split of the range fails when the series drifts more than the emitter's
    amplitude, which is exactly what daylight does.
    """
    phase = []
    for i, m in enumerate(means):
        nbrs = [means[j] for j in (i - 1, i + 1) if 0 <= j < len(means)]
        phase.append("ON" if m > sum(nbrs) / len(nbrs) else "OFF")
    return phase


def report(name, means):
    if len(means) < 3:
        print(f"{name}: too few frames ({len(means)})")
        return

    phase = classify(means)
    on = [m for m, p in zip(means, phase) if p == "ON"]
    off = [m for m, p in zip(means, phase) if p == "OFF"]
    floor = min(means)

    print(f"\n{name}   ({len(means)} frames)")
    print(f"  ambient floor      {floor:8.3f}     <- what the gate measures")
    if on and off:
        amp = statistics.fmean(on) - statistics.fmean(off)
        print(f"  mean ON / OFF      {statistics.fmean(on):8.3f} / {statistics.fmean(off):.3f}")
        print(f"  strobe amplitude   {amp:8.3f}")
        print(f"  old OFF/ON ratio   {statistics.fmean(off) / statistics.fmean(on):8.4f}")
        # Same-phase step is the noise floor classify() measures the emitter against.
        same = [abs(b - a) for a, b in zip(on, on[1:])] + \
               [abs(b - a) for a, b in zip(off, off[1:])]
        noise = statistics.median(same) if same else 0.0
        print(f"  same-phase noise   {noise:8.4f}")
        print(f"  whole-frame SNR    {amp / noise:8.1f}" if noise > 0 else
              "  whole-frame SNR         inf  (noise floor is exactly 0)")
    print(f"  ON/OFF pattern     {''.join('1' if p == 'ON' else '0' for p in phase[:40])}")


def capture(frames, device="/dev/video2"):
    out = tempfile.NamedTemporaryFile(suffix=".gray", delete=False)
    out.close()
    subprocess.run(
        # -fps_mode passthrough is load-bearing: without it ffmpeg rate-converts to a
        # constant 30 fps and duplicates frames, turning ON,off,ON,off into ON,ON,off,off.
        # wiki/11 B5, and tools/record.sh carries the same flag for the same reason.
        ["ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "v4l2",
         "-input_format", "gray", "-video_size", f"{W}x{H}", "-framerate", "30",
         "-i", device, "-frames:v", str(frames), "-fps_mode", "passthrough",
         "-f", "rawvideo", "-pix_fmt", "gray", "-y", out.name],
        check=True,
    )
    return out.name


def main():
    args = sys.argv[1:]
    if args and args[0] == "--live":
        n = int(args[1]) if len(args) > 1 else 60
        path = capture(n)
        report(f"live capture ({n} frames)", frame_means(path))
        return
    if not args:
        print(__doc__)
        sys.exit(2)
    for path in args:
        report(path.split("/")[-1], frame_means(path))


if __name__ == "__main__":
    main()

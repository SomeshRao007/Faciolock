#!/usr/bin/env python3
"""Read-only probe of the Microsoft Camera Extension Unit (MSXU) on the IR camera.

Issues GET requests only -- never SET_CUR. Nothing on the device changes.

Why this exists: the IR camera's MSXU (unit 4, GUID {0f3f95dc-...}) advertises
MSXU_CONTROL_FACE_AUTHENTICATION (selector 0x06, bit D5 of bmControls). Windows Hello
sets that control; Linux never has. If the firmware switches capture mode when it is
set, the ambient-IR ceiling on strobe liveness is a software gap, not a hardware one.

Spec: Microsoft Extensions to USB Video Class 1.5, section 2.2.2.6.
Modes, from KSPROPERTY_CAMERACONTROL_EXTENDED_FACEAUTH_MODE:
    D0 DISABLED (general-purpose IR)  D1 ALTERNATIVE_FRAME_ILLUMINATION  D2 BACKGROUND_SUBTRACTION
"""

import ctypes
import fcntl
import struct
import sys

# struct uvc_xu_control_query { u8 unit; u8 selector; u8 query; u16 size; u8 *data; }
# x86-64 layout: 1+1+1+pad, u16 @4, pad, ptr @8 -- 16 bytes total.
_QUERY_FMT = "=BBBxHxxQ"
UVCIOC_CTRL_QUERY = (3 << 30) | (struct.calcsize(_QUERY_FMT) << 16) | (ord("u") << 8) | 0x21

# Video Class-Specific Request Codes, UVC 1.5 spec A.8.
QUERIES = {
    "SET_CUR": 0x01,
    "GET_CUR": 0x81, "GET_MIN": 0x82, "GET_MAX": 0x83,
    "GET_RES": 0x84, "GET_LEN": 0x85, "GET_INFO": 0x86, "GET_DEF": 0x87,
}

MSXU_UNIT = 4
SEL_FACE_AUTH = 0x06
SEL_METADATA = 0x09


def query(fd, unit, selector, query_code, size, data=None):
    """`data` is the outbound payload for SET_CUR; GET requests leave it None.

    The buffer length must equal the device's GET_LEN exactly or the driver returns
    EINVAL -- the kernel checks it against the descriptor, not against what we send.
    """
    buf = ctypes.create_string_buffer(data if data is not None else size, size)
    req = struct.pack(_QUERY_FMT, unit, selector, query_code, size,
                      ctypes.addressof(buf))
    fcntl.ioctl(fd, UVCIOC_CTRL_QUERY, req)
    return buf.raw


def get_len(fd, unit, selector):
    return struct.unpack("<H", query(fd, unit, selector, QUERIES["GET_LEN"], 2))[0]


def decode_info(b):
    bits = [(0, "SET_CUR"), (1, "GET_CUR"), (2, "GET_MIN/MAX/RES"),
            (3, "AUTOUPDATE"), (4, "ASYNCHRONOUS"), (5, "DISABLED_BY_AUTO")]
    return ", ".join(n for i, n in bits if b & (1 << i)) or "none"


def decode_face_auth(raw):
    """bNumEntries, then per entry: bStreamingInterface + dwControlFlags."""
    if not raw:
        return "  (empty)"
    n = raw[0]
    if n == 0:
        return "  bNumEntries=0"
    stride = (len(raw) - 1) // n
    out = [f"  bNumEntries={n}  (stride {stride} B/entry)"]
    for i in range(n):
        e = raw[1 + i * stride: 1 + (i + 1) * stride]
        iface, flags = e[0], int.from_bytes(e[1:5], "little")
        modes = [n for b, n in ((0, "DISABLED"), (1, "ALT_FRAME_ILLUM"),
                                (2, "BACKGROUND_SUBTRACTION")) if flags & (1 << b)]
        out.append(f"    iface 0x{iface:02x}  flags 0x{flags:08x}  "
                   f"[{', '.join(modes) or '-'}]")
    return "\n".join(out)


def probe(fd, selector, name, decoder=None):
    print(f"\n=== unit {MSXU_UNIT}, selector 0x{selector:02x} ({name}) ===")
    try:
        length = get_len(fd, MSXU_UNIT, selector)
    except OSError as e:
        print(f"  GET_LEN failed: {e}")
        return
    print(f"  GET_LEN  = {length} bytes")

    try:
        info = query(fd, MSXU_UNIT, selector, QUERIES["GET_INFO"], 1)[0]
        print(f"  GET_INFO = 0x{info:02x}  [{decode_info(info)}]")
    except OSError as e:
        print(f"  GET_INFO failed: {e}")

    for q in ("GET_MIN", "GET_MAX", "GET_RES", "GET_DEF", "GET_CUR"):
        try:
            raw = query(fd, MSXU_UNIT, selector, QUERIES[q], length)
        except OSError as e:
            print(f"  {q:8s}: failed: {e}")
            continue
        print(f"  {q:8s}: {raw.hex(' ')}")
        if decoder:
            print(decoder(raw))


MODES = {"disabled": 0x01, "alt": 0x02, "bgsub": 0x04}


def set_mode(fd, mode_bit, iface=0x03):
    """SET_CUR one mode on one streaming interface.

    Payload mirrors GET_CUR's shape, learned from the device rather than assumed:
    bNumEntries=1, then bStreamingInterface + dwControlFlags over an 8-byte stride.
    Exactly one of D0/D1/D2 may be set per interface (spec 2.2.2.6).
    """
    length = get_len(fd, MSXU_UNIT, SEL_FACE_AUTH)
    payload = bytes([1, iface]) + mode_bit.to_bytes(4, "little")
    payload += b"\x00" * (length - len(payload))
    query(fd, MSXU_UNIT, SEL_FACE_AUTH, QUERIES["SET_CUR"], length, payload)
    return query(fd, MSXU_UNIT, SEL_FACE_AUTH, QUERIES["GET_CUR"], length)


def capture_means(dev, frames=40):
    """Per-frame mean luma via the same ffmpeg invocation record.sh uses.

    `-fps_mode passthrough` is not optional: without it ffmpeg duplicates frames to fill
    a constant 30 fps and turns ON,off,ON,off into ON,ON,off,off.
    """
    import subprocess
    w, h = 640, 360
    out = subprocess.run(
        ["ffmpeg", "-hide_banner", "-loglevel", "error",
         "-f", "v4l2", "-input_format", "gray", "-video_size", f"{w}x{h}",
         "-framerate", "30", "-i", dev, "-frames:v", str(frames),
         "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "gray", "-"],
        capture_output=True, check=True).stdout
    n = len(out) // (w * h)
    return [sum(out[i * w * h:(i + 1) * w * h]) / (w * h) for i in range(n)]


def report(label, means, settle=5):
    """Ambient floor and strobe amplitude, in the difference domain, where the ambient pedestal cancels."""
    m = means[settle:]
    if len(m) < 3:
        print(f"  {label}: only {len(m)} frames, cannot measure")
        return None
    steps = [abs(b - a) for a, b in zip(m, m[1:])]
    same = [abs(c - a) for a, c in zip(m, m[2:])]
    amp = sorted(steps)[len(steps) // 2]
    noise = sorted(same)[len(same) // 2]
    floor = min(m)
    snr = amp / noise if noise > 0 else float("inf")
    print(f"  {label:<28} floor {floor:8.3f}   amplitude {amp:7.2f}   SNR {snr:9.1f}"
          f"   ({len(m)} frames)")
    return floor


def main():
    args = sys.argv[1:]
    dev = "/dev/video2"
    if args and args[0].startswith("/dev/"):
        dev, args = args[0], args[1:]

    if not args:
        print(f"MSXU read-only probe on {dev} -- GET requests only, nothing is written.")
        with open(dev, "rb", buffering=0) as f:
            probe(f.fileno(), SEL_FACE_AUTH, "FACE_AUTHENTICATION", decode_face_auth)
            probe(f.fileno(), SEL_METADATA, "METADATA")
        return

    if args[0] == "set":
        mode = args[1]
        with open(dev, "rb", buffering=0) as f:
            cur = set_mode(f.fileno(), MODES[mode])
        print(f"SET_CUR {mode} -> GET_CUR {cur.hex(' ')}")
        print(decode_face_auth(cur))
        return

    if args[0] == "experiment":
        # Matched pair, seconds apart, same room: the only design that can attribute a
        # change to the mode rather than to the light. No face needed -- the ambient floor
        # is a property of the room.
        print(f"MSXU face-auth experiment on {dev}\n")
        with open(dev, "rb", buffering=0) as f:
            before_state = query(f.fileno(), MSXU_UNIT, SEL_FACE_AUTH,
                                 QUERIES["GET_CUR"], get_len(f.fileno(), MSXU_UNIT,
                                                             SEL_FACE_AUTH))
        print(f"state before: {before_state.hex(' ')}")
        print(decode_face_auth(before_state))

        print("\nmeasuring:")
        base = report("DISABLED (as found)", capture_means(dev))

        with open(dev, "rb", buffering=0) as f:
            cur = set_mode(f.fileno(), MODES["alt"])
        print(f"\nSET_CUR alt -> {cur.hex(' ')}")
        print(decode_face_auth(cur))

        # Does it survive the fd closing? Decides whether the daemon can set it once at
        # startup or must hold the handle for the life of the stream.
        with open(dev, "rb", buffering=0) as f:
            persist = query(f.fileno(), MSXU_UNIT, SEL_FACE_AUTH, QUERIES["GET_CUR"],
                            len(cur))
        print(f"after reopen: {persist.hex(' ')}  "
              f"{'PERSISTS' if persist == cur else 'RESET on close'}")

        print("\nmeasuring:")
        after = report("ALT_FRAME_ILLUM", capture_means(dev))

        if base and after:
            print(f"\nambient floor {base:.3f} -> {after:.3f}   "
                  f"({base / after:.2f}x lower)" if after > 0 else "")
        return

    sys.exit(f"usage: {sys.argv[0]} [/dev/videoN] [set disabled|alt|bgsub | experiment]")


if __name__ == "__main__":
    main()

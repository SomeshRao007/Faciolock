#!/usr/bin/env python3
"""Read-only probe of the IR camera's metadata node, /dev/video3.

Never issues SET_CUR, never writes a control. It streams and it reads.

Why this exists: wiki/11 B20 showed the ambient gate keys on a quantity the *attacker*
sets -- presenting a bright object near the lens collapses the measured floor 2,170x,
because the sensor AGC shortens exposure and the ambient pedestal follows. That diagnosis
is an inference from two columns of a table. If the camera reports its exposure time
per frame, it stops being an inference: ambient irradiance is pedestal / integration time,
which is a gate input the attacker no longer sits between.

The IR camera advertises MSXU_CONTROL_METADATA (unit 4, selector 0x09) and the kernel
lights up both metadata formats on /dev/video3:

    [0] 'UVCH'  V4L2_META_FMT_UVC            standard UVC payload headers
    [1] 'UVCM'  V4L2_META_FMT_UVC_MSXU_1_5   Microsoft MSXU metadata

Two things would be worth having, per "Microsoft Extensions to USB Video Class 1.5":

    CaptureStats        -- ExposureTime, which is what B20 needs
    FrameIllumination   -- per-frame ON/OFF ground truth, which would replace
                           frame::classify inferring phase from luma (B18)

**The MetadataId values and the CaptureStats field layout below are from the MSXU spec
and have not been re-read against it here.** So this dumps raw bytes first and decodes
second, and prints both: a wrong constant shows up as a decode that disagrees with the
hex, rather than as a confident wrong number. The decode is additionally checked against
something already known to be true -- the emitter alternates every frame (wiki/10 Probe 3,
four independent runs) -- so an illumination flag that tracks the measured frame means is
self-proving and one that does not is refuted.
"""

import ctypes
import fcntl
import mmap
import os
import struct
import sys

# ---------------------------------------------------------------------------
# V4L2, the parts of it this spike needs. Layouts from linux/videodev2.h.
# ---------------------------------------------------------------------------

VIDIOC_QUERYCAP = 0x80685600
VIDIOC_ENUM_FMT = 0xC0405602
VIDIOC_G_FMT = 0xC0D05604
VIDIOC_S_FMT = 0xC0D05605
VIDIOC_REQBUFS = 0xC0145608
VIDIOC_QUERYBUF = 0xC0585609
VIDIOC_QBUF = 0xC058560F
VIDIOC_DQBUF = 0xC0585611
VIDIOC_STREAMON = 0x40045612
VIDIOC_STREAMOFF = 0x40045613

BUF_TYPE_VIDEO_CAPTURE = 1
BUF_TYPE_META_CAPTURE = 13
MEMORY_MMAP = 1

FOURCC_GREY = 0x59455247  # 'GREY'
FOURCC_UVCH = 0x48435655  # 'UVCH'
FOURCC_UVCM = 0x4D435655  # 'UVCM'

WIDTH, HEIGHT = 640, 360

# Frames dropped from the head before the ON/OFF comparison. wiki/10 Probe 3 measured
# 22.7, 23.4, 14.3, 31.4 before the AGC settles into a clean alternation -- four frames.
# This spike streams raw, with none of the daemon's warm-up, so it pays that here.
SETTLE = 4


def fourcc_str(v):
    return "".join(chr((v >> s) & 0xFF) for s in (0, 8, 16, 24))


# struct v4l2_format is a u32 type plus a union, and the ioctl encodes its own size:
# _IOC_SIZE(VIDIOC_S_FMT) == 0xd0 == 208. Taking the number from the ioctl rather than
# from a count of union members is the only version of this that cannot drift.
FMT_SIZE = (VIDIOC_S_FMT >> 16) & 0x3FFF

# **The union does not start at offset 4.** One of its members is `struct v4l2_window`,
# which holds a `struct v4l2_clip __user *`, so the union is 8-byte aligned and four bytes
# of padding follow `type`. 4 + 4 + 200 == 208 == the size the ioctl encodes, which is the
# arithmetic that catches it. Writing at offset 4 puts `dataformat` in the padding: the
# driver reads zero, silently substitutes UVCH, and the read-back at the same wrong offset
# agrees with itself. That is a whole afternoon if the read-back is trusted.
FMT_UNION_AT = 8


def s_fmt_video(fd, width, height, pixfmt):
    raw = bytearray(FMT_SIZE)
    struct.pack_into("=I", raw, 0, BUF_TYPE_VIDEO_CAPTURE)
    # struct v4l2_pix_format: width, height, pixelformat, field, bytesperline, sizeimage
    struct.pack_into("=IIIIII", raw, FMT_UNION_AT, width, height, pixfmt, 1, 0, 0)
    fcntl.ioctl(fd, VIDIOC_S_FMT, raw)
    w, h, pf = struct.unpack_from("=III", raw, FMT_UNION_AT)
    return w, h, pf


def s_fmt_meta(fd, dataformat):
    raw = bytearray(FMT_SIZE)
    struct.pack_into("=I", raw, 0, BUF_TYPE_META_CAPTURE)
    # struct v4l2_meta_format { u32 dataformat; u32 buffersize; }
    struct.pack_into("=II", raw, FMT_UNION_AT, dataformat, 0)
    fcntl.ioctl(fd, VIDIOC_S_FMT, raw)
    # Read back rather than trust: drivers substitute a format they prefer and say
    # nothing about it.
    got, size = struct.unpack_from("=II", raw, FMT_UNION_AT)
    return got, size


def reqbufs(fd, buf_type, count):
    # struct v4l2_requestbuffers { u32 count, type, memory, capabilities; u8 flags;
    #                              u8 reserved[3]; } -- 20 bytes, and the ioctl says so.
    raw = bytearray(struct.pack("=IIIII", count, buf_type, MEMORY_MMAP, 0, 0))
    assert len(raw) == (VIDIOC_REQBUFS >> 16) & 0x3FFF
    fcntl.ioctl(fd, VIDIOC_REQBUFS, raw)
    return struct.unpack_from("=I", raw, 0)[0]


# struct v4l2_buffer on a 64-bit kernel, laid out field by field because the two
# 8-byte members (timeval, the `m` union) put padding in places a naive run of u32s
# gets wrong -- which is exactly the class of bug P2-a chose the `v4l` crate to avoid
# in the shipped code. A spike may hand-roll it; it may not guess at it.
#
#   0 index    4 type      8 bytesused  12 flags   16 field   (20 pad)
#  24 tv_sec  32 tv_usec  40 tc_type    44 tc_flags 48 tc_rest[8]
#  56 sequence 60 memory  64 m.offset  72 length   76 reserved2  80 request_fd
BUF_FMT = "=IIIII4xqqII8sIIQIIi4x"
BUF_SIZE = struct.calcsize(BUF_FMT)


def pack_buf(index, buf_type):
    return bytearray(
        struct.pack(BUF_FMT, index, buf_type, 0, 0, 0, 0, 0, 0, 0, b"\0" * 8,
                    0, MEMORY_MMAP, 0, 0, 0, 0)
    )


def unpack_buf(raw):
    f = struct.unpack(BUF_FMT, bytes(raw))
    return {"index": f[0], "bytesused": f[2], "sec": f[5], "usec": f[6],
            "sequence": f[10], "offset": f[12], "length": f[13]}


def querybuf(fd, index, buf_type):
    raw = pack_buf(index, buf_type)
    fcntl.ioctl(fd, VIDIOC_QUERYBUF, raw)
    return unpack_buf(raw)


def qbuf(fd, index, buf_type):
    fcntl.ioctl(fd, VIDIOC_QBUF, pack_buf(index, buf_type))


def dqbuf(fd, buf_type):
    raw = pack_buf(0, buf_type)
    fcntl.ioctl(fd, VIDIOC_DQBUF, raw)
    return unpack_buf(raw)


def streamon(fd, buf_type):
    fcntl.ioctl(fd, VIDIOC_STREAMON, struct.pack("=I", buf_type))


def streamoff(fd, buf_type):
    fcntl.ioctl(fd, VIDIOC_STREAMOFF, struct.pack("=I", buf_type))


def enum_meta_formats(fd):
    out = []
    for i in range(8):
        # struct v4l2_fmtdesc { u32 index; u32 type; u32 flags; u8 description[32];
        #                       u32 pixelformat; u32 mbus_code; u32 reserved[3]; }
        raw = bytearray(64)
        struct.pack_into("=II", raw, 0, i, BUF_TYPE_META_CAPTURE)
        try:
            fcntl.ioctl(fd, VIDIOC_ENUM_FMT, raw)
        except OSError:
            break
        desc = raw[12:44].split(b"\0")[0].decode("ascii", "replace")
        out.append((struct.unpack_from("=I", raw, 44)[0], desc))
    return out


# ---------------------------------------------------------------------------
# The metadata payload.
# ---------------------------------------------------------------------------

# struct uvc_meta_buf { u64 ns; u16 sof; u8 length; u8 flags; u8 buf[]; } __packed
# Verified against /usr/include/linux/uvcvideo.h on this machine, not from memory.
META_HDR = "=QHBB"
META_HDR_SIZE = struct.calcsize(META_HDR)

# From the MSXU spec's KSCAMERA_METADATA_* identifiers. **Verify the section before
# quoting these anywhere outside this spike.**
METADATA_IDS = {
    1: "PhotoConfirmation",
    2: "UsbVideoHeader",
    3: "CaptureStats",
    4: "CameraExtrinsics",
    5: "CameraIntrinsics",
    6: "FrameIllumination",
}
CAPTURESTATS_FLAG_EXPOSURETIME = 0x00000001
FRAMEILLUMINATION_FLAG_ON = 0x00000001


def split_meta_records(payload):
    """One dequeued buffer may hold several uvc_meta_buf records back to back.

    The kernel's own comment says so: each record is
    sizeof(ns) + sizeof(sof) + length bytes. Assuming one record per frame would
    silently drop everything after the first.
    """
    records, at = [], 0
    while at + META_HDR_SIZE <= len(payload):
        ns, sof, length, flags = struct.unpack_from(META_HDR, payload, at)
        # `length` counts the UVC payload header, of which `flags` is the second byte.
        if length < 2:
            break
        end = at + 10 + length
        if end > len(payload):
            break
        records.append({"ns": ns, "sof": sof, "length": length, "flags": flags,
                        "body": bytes(payload[at + META_HDR_SIZE:end]),
                        "raw": bytes(payload[at:end])})
        at = end
    return records, len(payload) - at


def decode_msxu_blobs(body):
    """MSXU metadata is a sequence of { MetadataId u32, Size u32, payload }.

    `Size` includes the 8-byte header. Anything that does not walk cleanly is reported
    as undecodable rather than guessed at.
    """
    blobs, at = [], 0
    while at + 8 <= len(body):
        mid, size = struct.unpack_from("=II", body, at)
        if size < 8 or at + size > len(body):
            return blobs, body[at:]
        blobs.append({"id": mid, "name": METADATA_IDS.get(mid, f"unknown({mid})"),
                      "payload": bytes(body[at + 8:at + size])})
        at += size
    return blobs, body[at:]


def decode_capture_stats(payload):
    """KSCAMERA_METADATA_CAPTURESTATS, first three fields only.

    Flags u32 | Reserved u32 | ExposureTime u64 (100 ns units). The rest of the struct
    is not needed here and is left undecoded rather than half-guessed.
    """
    if len(payload) < 16:
        return None
    flags, _reserved, exposure = struct.unpack_from("=IIQ", payload, 0)
    valid = bool(flags & CAPTURESTATS_FLAG_EXPOSURETIME)
    return {"flags": flags, "exposure_100ns": exposure, "exposure_valid": valid,
            "exposure_ms": exposure / 10_000.0 if valid else None}


def decode_frame_illumination(payload):
    if len(payload) < 4:
        return None
    flags = struct.unpack_from("=I", payload, 0)[0]
    return {"flags": flags, "on": bool(flags & FRAMEILLUMINATION_FLAG_ON)}


# ---------------------------------------------------------------------------

def frame_mean(buf):
    """Mean luma, as the phase ground truth. Subsampled: this is a sanity check on a
    decode, not a measurement anybody thresholds on."""
    return sum(buf[::37]) / len(buf[::37])


def run(video_path, meta_path, want_format, frames):
    vfd = os.open(video_path, os.O_RDWR)
    mfd = os.open(meta_path, os.O_RDWR)
    try:
        print(f"metadata formats on {meta_path}:")
        for pf, desc in enum_meta_formats(mfd):
            print(f"  {fourcc_str(pf)}  {desc}")

        w, h, pf = s_fmt_video(vfd, WIDTH, HEIGHT, FOURCC_GREY)
        print(f"\nvideo  {video_path}: {w}x{h} {fourcc_str(pf)}")

        got, size = s_fmt_meta(mfd, want_format)
        print(f"meta   {meta_path}: requested {fourcc_str(want_format)}, "
              f"driver accepted {fourcc_str(got)}, buffer {size} bytes")
        if got != want_format:
            print("  -> the driver substituted a format. Reporting what it gave.")

        n_meta = reqbufs(mfd, BUF_TYPE_META_CAPTURE, 8)
        n_vid = reqbufs(vfd, BUF_TYPE_VIDEO_CAPTURE, 8)
        print(f"buffers: {n_vid} video, {n_meta} meta")

        maps_v, maps_m = [], []
        for i in range(n_vid):
            b = querybuf(vfd, i, BUF_TYPE_VIDEO_CAPTURE)
            maps_v.append(mmap.mmap(vfd, b["length"], offset=b["offset"]))
            qbuf(vfd, i, BUF_TYPE_VIDEO_CAPTURE)
        for i in range(n_meta):
            b = querybuf(mfd, i, BUF_TYPE_META_CAPTURE)
            maps_m.append(mmap.mmap(mfd, b["length"], offset=b["offset"]))
            qbuf(mfd, i, BUF_TYPE_META_CAPTURE)

        # Metadata first: some drivers only emit for payloads that arrive after the
        # meta stream is running, and starting video first would lose the head.
        streamon(mfd, BUF_TYPE_META_CAPTURE)
        streamon(vfd, BUF_TYPE_VIDEO_CAPTURE)

        print(f"\nstreaming {frames} frames\n" + "-" * 72)
        stats_seen = illum_seen = 0
        # A standard UVC payload header with PTS and SCR is exactly 12 bytes: 2 + 4 + 6.
        # Anything vendor-specific has to make the header *longer* than that, so the
        # length histogram is the whole question and it is one line of bookkeeping.
        lengths, flagset, oversize, means, tags = {}, set(), [], [], []
        for n in range(frames):
            vb = dqbuf(vfd, BUF_TYPE_VIDEO_CAPTURE)
            mean = frame_mean(maps_v[vb["index"]][:vb["bytesused"]])
            qbuf(vfd, vb["index"], BUF_TYPE_VIDEO_CAPTURE)

            mb = dqbuf(mfd, BUF_TYPE_META_CAPTURE)
            payload = bytes(maps_m[mb["index"]][:mb["bytesused"]])
            qbuf(mfd, mb["index"], BUF_TYPE_META_CAPTURE)

            records, leftover = split_meta_records(payload)
            means.append(mean)
            # One tag per frame, appended unconditionally below, so `means` and `tags`
            # stay index-aligned even for a frame that carried no metadata at all.
            frame_tag = None
            print(f"frame {n:2d}  seq={vb['sequence']:<5d} mean={mean:7.2f}  "
                  f"meta {mb['bytesused']:>6d} bytes, {len(records):>4d} record(s)"
                  + (f", {leftover} undecoded" if leftover else ""))

            for r in records:
                lengths[r["length"]] = lengths.get(r["length"], 0) + 1
                flagset.add(r["flags"])
                # 12 == 2 + PTS(4) + SCR(6). Only a longer header can carry MSXU data,
                # so only a longer header is worth decoding or printing.
                if r["length"] <= 12:
                    continue
                oversize.append(r)
                print(f"  sof={r['sof']:<6d} len={r['length']:<3d} "
                      f"flags=0x{r['flags']:02x} body={r['body'].hex()}")
                blobs, tail = decode_msxu_blobs(r["body"][10:])
                for b in blobs:
                    line = f"    id={b['id']} {b['name']:<18} {len(b['payload'])} bytes"
                    if b["name"] == "CaptureStats":
                        cs = decode_capture_stats(b["payload"])
                        stats_seen += 1
                        if cs:
                            line += (f"  flags=0x{cs['flags']:08x} exposure="
                                     f"{cs['exposure_ms']} ms" if cs["exposure_valid"]
                                     else f"  flags=0x{cs['flags']:08x} exposure not valid")
                    elif b["name"] == "FrameIllumination":
                        fi = decode_frame_illumination(b["payload"])
                        illum_seen += 1
                        if fi:
                            frame_tag = fi["on"]
                            line += f"  {'ON' if fi['on'] else 'OFF'} (0x{fi['flags']:08x})"
                    print(line)
                if tail:
                    print(f"    {len(tail)} bytes did not decode: {tail.hex()[:48]}")

            tags.append(frame_tag)

        streamoff(vfd, BUF_TYPE_VIDEO_CAPTURE)
        streamoff(mfd, BUF_TYPE_META_CAPTURE)

        print("-" * 72)
        print(f"payload header lengths: "
              + ", ".join(f"{k} bytes x{v}" for k, v in sorted(lengths.items())))
        print(f"payload header flags:   " + ", ".join(f"0x{f:02x}" for f in sorted(flagset)))
        print(f"headers longer than 12: {len(oversize)}")
        print(f"CaptureStats blobs:     {stats_seen}")
        print(f"FrameIllumination blobs:{illum_seen}")

        if not oversize:
            print("\nVERDICT: UVCM streams, and every payload header is the standard")
            print("2 + PTS(4) + SCR(6) = 12 bytes. There is no room in them for MSXU")
            print("metadata, so this firmware appends none.")
            return

        # **The decode has to be checked against something already known.** The emitter
        # alternates every frame (wiki/10 Probe 3, four independent runs), so an
        # illumination tag that is real must line up with the frame means. A tag that
        # does not is a wrong constant, not a discovery.
        settled = [(m, t) for m, t in list(zip(means, tags))[SETTLE:] if t is not None]
        on = [m for m, t in settled if t]
        off = [m for m, t in settled if not t]
        print("\nframe means against the tag (first "
              f"{SETTLE} dropped, AGC):")
        for i, (m, t) in enumerate(zip(means, tags)):
            mark = "?" if t is None else ("ON " if t else "OFF")
            print(f"  {i:2d}  {m:7.2f}  {mark}" + ("   <- AGC settle" if i < SETTLE else ""))
        if on and off:
            print(f"\n  mean(ON)  = {sum(on)/len(on):7.2f}   n={len(on)}")
            print(f"  mean(OFF) = {sum(off)/len(off):7.2f}   n={len(off)}")
            print(f"  separation = {sum(on)/len(on) - sum(off)/len(off):+7.2f}")
            print("\nVERDICT: the firmware reports per-frame illumination and it agrees"
                  if sum(on) / len(on) > sum(off) / len(off) else
                  "\nVERDICT: the tag DISAGREES with the measured means -- decode is wrong")
            print("with the luma. No CaptureStats, so no exposure time." if stats_seen == 0
                  else "with the luma, and CaptureStats carries exposure time.")
    finally:
        os.close(vfd)
        os.close(mfd)


if __name__ == "__main__":
    video = sys.argv[1] if len(sys.argv) > 1 else "/dev/video2"
    meta = sys.argv[2] if len(sys.argv) > 2 else "/dev/video3"
    fmt = FOURCC_UVCH if "--uvch" in sys.argv else FOURCC_UVCM
    # 30 frames is ~2.5 s at the measured 12-15 fps (B19), which gives the ON/OFF
    # comparison roughly a dozen samples a side rather than three.
    count = 30
    run(video, meta, fmt, count)

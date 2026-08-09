#!/usr/bin/env python3
"""Set the MSXU face-authentication mode on the IR camera.

    set_mode.py                 show current mode, change nothing
    set_mode.py alt             ALTERNATIVE_FRAME_ILLUMINATION  (what Windows Hello uses)
    set_mode.py disabled        back to the factory default

This WRITES to the camera (SET_CUR, unit 4, selector 0x06). Reversible: `disabled` is
the device's own GET_DEF, and a USB replug restores it regardless.

Only the Microsoft XU (unit 4) is ever touched. The Sonix vendor XU (unit 3) is left
alone on purpose -- it advertises no controls on the IR path and sits next to an
undriven "Application Specific" interface that looks like a firmware-update endpoint.

After setting, re-reads through a fresh open() to report whether the mode survives the
file descriptor being closed -- that decides whether faciolockd can set it once at
startup or has to hold the device open.
"""

import sys

from probe import MSXU_UNIT, QUERIES, SEL_FACE_AUTH, decode_face_auth, get_len, query

MODES = {"disabled": 0x01, "alt": 0x02, "bgsub": 0x04}


def set_mode(fd, iface, flags, length):
    """bNumEntries=1, then the entry for one streaming interface.

    Layout read back off the device rather than assumed: GET_LEN says 9, GET_MAX says
    bNumEntries=1, so the entry is the remaining 8 bytes -- interface, then a 32-bit
    flags word, then reserved. Confirmed against GET_MAX/GET_DEF, which decode to the
    exact mode combinations the spec says are legal.
    """
    payload = bytearray(length)
    payload[0] = 1
    payload[1] = iface
    payload[2:6] = flags.to_bytes(4, "little")
    query(fd, MSXU_UNIT, SEL_FACE_AUTH, QUERIES["SET_CUR"], length, bytes(payload))


def current(dev):
    with open(dev, "rb", buffering=0) as f:
        length = get_len(f.fileno(), MSXU_UNIT, SEL_FACE_AUTH)
        return query(f.fileno(), MSXU_UNIT, SEL_FACE_AUTH, QUERIES["GET_CUR"], length)


def main():
    dev = "/dev/video2"
    want = sys.argv[1].lower() if len(sys.argv) > 1 else None

    print("current:")
    print(decode_face_auth(current(dev)))
    if want is None:
        return

    if want not in MODES:
        print(f"\nunknown mode '{want}' -- pick one of {', '.join(MODES)}", file=sys.stderr)
        sys.exit(2)

    with open(dev, "rb", buffering=0) as f:
        length = get_len(f.fileno(), MSXU_UNIT, SEL_FACE_AUTH)
        maxi = query(f.fileno(), MSXU_UNIT, SEL_FACE_AUTH, QUERIES["GET_MAX"], length)
        iface, supported = maxi[1], int.from_bytes(maxi[2:6], "little")
        if not supported & MODES[want]:
            print(f"\ndevice does not support '{want}' "
                  f"(GET_MAX flags 0x{supported:x})", file=sys.stderr)
            sys.exit(1)

        print(f"\nSET_CUR: iface 0x{iface:02x} -> {want} (0x{MODES[want]:02x})")
        set_mode(f.fileno(), iface, MODES[want], length)

        print("read back, same fd:")
        print(decode_face_auth(
            query(f.fileno(), MSXU_UNIT, SEL_FACE_AUTH, QUERIES["GET_CUR"], length)))

    print("read back, fresh open (does it survive close?):")
    print(decode_face_auth(current(dev)))


if __name__ == "__main__":
    main()

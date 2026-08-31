# 15 — Hardware Compatibility

Faciolock needs a **near-infrared camera** — the second, greyscale sensor that ships
beside the webcam on laptops sold with Windows Hello. It cannot work on an RGB webcam,
and this is not a limitation that a better model would fix: the whole liveness argument
rests on the IR emitter being the light source.

This page is the list of machines it has actually been run on. **One row is measured;
everything else on this page is a prediction.** Adding a row is the single most useful
contribution this project can receive — see [Reporting yours](#reporting-yours).

---

## The list

| Machine | Camera | IR node | Tier | Status |
|---|---|---|---|---|
| **ASUS Vivobook 16 X1607CA** (Core Ultra 5 225H, Kubuntu 26.04) | Sonix `3277:0018`, "ASUS FHD webcam: ASUS IR camera" | `/dev/video2`, `GREY` 640×360 | `ir-strobe` | **works — the reference machine.** Enrolment, `sudo`, SDDM and the KDE lock screen all authenticate by face |

That is the entire list. It is one machine, and every hardware claim this project makes
comes from it.

---

## The two tiers, and how to tell which you have

The hardware split that matters is **what controls the IR emitter**.

| Tier | What the hardware does | What Faciolock does |
|---|---|---|
| `ir-strobe` | The camera alternates emitter-lit and emitter-dark frames by itself, ~15 ON/OFF pairs per second, with no driver involvement | Reads pairs. The dark frame of each pair is the evidence that the bright frame was lit by *our* emitter and not by the room |
| `ir-constant` | The emitter is on continuously, or is off until something turns it on | Reads frames. **The dark-frame integrity check has nothing to compare against**, so one of four liveness checks is unavailable |

`faciolockctl diagnose` measures this rather than guessing — it captures a burst and
reports the alternation ratio. On the reference machine that reads **1.00 at 44.8:1**:
every frame alternates, and the bright frames are 44.8× the dark ones.

```bash
sudo faciolockctl diagnose --write
```

**`ir-constant` has never been tested on real hardware.** The code path exists and the
tier can be selected; nothing has confirmed it behaves as designed, because no
constant-emitter camera has been available. Treat it as untested rather than supported.

**Faciolock never writes to the camera device.** No emitter control, no `v4l2-ctl -c`,
no vendor ioctls. Howdy's approach of poking the emitter is deliberately not taken: it
is per-model, it is undocumented, and getting it wrong on somebody else's laptop is a
class of bug this project would rather not have.

---

## Does your laptop have one?

```bash
for d in /dev/video*; do
  echo "=== $d"
  v4l2-ctl -d "$d" --info --list-formats 2>&1 | grep -E "Card type|\[[0-9]\]"
done
```

You are looking for a device whose card type says **IR** and whose only pixel format is
**`GREY`**. Both signals firing on the same node is what an IR camera looks like.

```
=== /dev/video2
	Card type        : ASUS FHD webcam: ASUS IR camera
	[0]: 'GREY' (8-bit Greyscale)
```

If `v4l2-ctl` is missing, install `v4l-utils` first — and note that a probe redirecting
stderr to `/dev/null` hides a missing binary exactly as well as it hides a real failure.

Two things that trip people up:

- **A node with no formats at all is not a camera.** On this hardware `/dev/video1` and
  `/dev/video3` list nothing: they are the firmware's per-frame metadata streams, one
  per real sensor. `diagnose` rejects them explicitly.
- **The numbering is not stable across boots.** `/dev/video2` today may be `/dev/video4`
  tomorrow. This is why the systemd unit allows the whole `char-video4linux` device
  class rather than a path glob, and why `diagnose --write` records a device that
  `faciolockd` re-checks at startup.

---

## What to expect if you have one

Realistic first-wave hardware is business laptops with Windows Hello: **ThinkPad**,
**Dell Latitude / XPS**, **HP EliteBook**, and consumer ASUS/Acer models that shipped
with an IR camera. Nothing about the code is ASUS-specific.

Two limitations will meet you on any hardware, because neither is about the camera:

- **Bright daylight refuses.** Ambient infrared saturates the checks that separate an
  emitter-lit face from a room-lit one, so in a sunlit room face authentication refuses
  and you type your password. This is the largest open defect in the project
  ([B18](11-bug-log.md#b18)).
- **A printed photograph is not reliably rejected** ([B24](11-bug-log.md#b24)).

---

## Reporting yours

Open an issue titled `hardware: <vendor> <model>` and paste the output of:

```bash
cat /sys/class/dmi/id/sys_vendor /sys/class/dmi/id/product_name
lsusb | grep -i camera
for d in /dev/video*; do echo "=== $d"; v4l2-ctl -d "$d" --info --list-formats 2>&1 | grep -E "Card type|\[[0-9]\]"; done
sudo faciolockctl diagnose
```

`diagnose` prints the alternation ratio, which is the number that decides your tier and
the one thing this project cannot guess from a model name. A report that says "it does
not work" and carries that output is more useful than a report that says it works and
carries none.

**No face images, ever.** Nothing above captures one, and none is wanted.

![Yunzii B75 Pro Max header](header.png)

<div align="center">

![Rust](https://img.shields.io/badge/Rust-1.85%2B-orange?logo=rust&logoColor=white)
![Platform](https://img.shields.io/badge/platform-Linux-lightgrey?logo=linux&logoColor=white)
![Protocol](https://img.shields.io/badge/protocol-USB%20HID%20(hidraw)-blueviolet?logo=usb&logoColor=white)
![Checks](https://img.shields.io/badge/checks-bin%2Fci-brightgreen)
![License](https://img.shields.io/badge/license-MIT-green)

</div>

# ⌨️ Yunzii B75 Pro Max — native screen control for Linux

No browser needed. 🎉

The Yunzii B75 Pro Max keyboard has a small TFT screen (clock / picture /
GIF). The only configuration tool the vendor ships is a browser-based WebHID
app at [yunzii-game.com](https://yunzii-game.com/) (Chrome/Edge/Opera only,
no native Linux app) — it works, but it always needs a browser tab open.

This repo talks directly to the keyboard over `/dev/hidraw*`, no browser, no
WebHID, no tab to keep around.

```
USB ID 28e9:31c8  GDMicroelectronics YUNZII B75 PRO MAX Keyboard
```

---

## 🗺️ Status

**⏰ `set-time`, 🖼️ `switch-page`, 🧹 `clear-picture`, and 🎨 `set-picture`
work!** Their protocols are fully decoded (see `PROTOCOL.md`) with native
CLI commands, and all four are visually confirmed on real hardware.

`switch-page` still has no `gif` option, but the reason changed. Milestone 2
thought cmd15 was broken. It is not: the vendor's GIF save has **three
modes**, and every earlier test had used mode 0 ("set as boot animation"),
which stores frames somewhere that never plays. Saved with mode 1 ("save to
the device") the GIF displays and cmd15 switches to it correctly. So the
option stays out only until GIF upload exists — switching to a page this
tool cannot write to is not a useful command. GIF upload is decoded and is
the next milestone; it reuses `set-picture`'s encoder unchanged.

No `ratatui` screen yet — CLI-only, done well. Sliders and toggles aren't
implemented; each gets its own reverse-engineering pass first, same process
as `set-time` below. 🚧

---

## ⚡ Quick start

```bash
cargo build --release
sudo cp udev/99-yunzii-b75.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
# unplug and replug the keyboard, then:
./target/release/yunzii-b75-tui set-time
./target/release/yunzii-b75-tui switch-page home    # or: picture (gif not shipped, see Status)
./target/release/yunzii-b75-tui clear-picture
./target/release/yunzii-b75-tui set-picture logo.png
```

### 🎨 `set-picture`

Takes a **PNG or JPEG**. The panel is a fixed **160×96**, and the image is
stretched to fill it with nearest-neighbour sampling — the same as the
vendor's tool, which draws with image smoothing switched off. **Aspect ratio
is not preserved.** Crop or letterbox the file yourself first if that
matters.

- **Fully transparent** pixels become **black**. Partial transparency keeps
  its full colour — the alpha value is discarded, not blended, which is what
  the vendor does too. A logo with soft edges shows those edges at full
  colour against black; pre-flatten it yourself if you want them faded.
- **EXIF orientation** is applied, so phone photos are not uploaded sideways.
  Verified by a test with a real orientation-tagged JPEG, not assumed.
- The image is decoded **before** the keyboard is opened, so a missing or
  corrupt file says exactly that instead of failing with "device not found".
- Uploading **replaces** whatever picture was there, and **switches the panel
  to the picture page by itself** — you do not need `switch-page picture`
  afterwards. Verified on hardware from the home page: the clock stayed up
  during the upload and the image appeared when it finished.

An upload is 552 reports and takes a moment. Like `set-time`, a failure
partway through aborts rather than silently continuing, and it says so:
a half-finished upload leaves a partially-written frame on the panel, and
the fix is to re-run `set-picture` or run `clear-picture`.

`clear-picture` sends 32 reports (16 repeats), with the same partial-failure
caveat.

The udev rule grants access to **all** of the keyboard's `hidraw`
interfaces for this VID/PID (there's no finer-grained udev match available),
not just the one this tool actually uses.

Requires: [Rust](https://rustup.rs) 🦀 (for `bin/ci`'s `cargo` steps too),
the keyboard connected via USB-C (2.4G dongle / Bluetooth untested), and the
vendor's browser tab (if any) closed — WebHID and this tool can't hold the
device open at the same time.

---

## 🔧 Hardware

Built and tested against USB `28e9:31c8`, config channel usage page
`0xFF60` / usage `0x61` (standard QMK/VIA-style Raw HID, unnumbered
report). Device discovery matches the **exact** report-descriptor bytes
captured from this unit, not just VID/PID — a firmware revision with a
byte-identical config channel but different padding elsewhere would be
rejected rather than silently assumed compatible. Interface numbering
(which `hidraw*` node) can vary by machine/session — see `PROTOCOL.md` for
how to re-identify it on yours.

---

## 🔍 How the protocol was reverse-engineered

The vendor's config page talks to the keyboard over WebHID. Rather than
guessing at the byte format blind, this repo:

1. Hooks `HIDDevice.prototype.sendReport`/`sendFeatureReport` and the
   `inputreport` event on the live page (see `scripts/capture-hook.js`) to
   record every HID message exchanged with the device.
2. Cross-references those captures against the vendor's own client-side
   JavaScript (already loaded into the browser to render the page — not
   extracted from firmware or any protected asset), which contains the exact
   functions that build these commands.
3. Verifies the resulting byte-level model against real captured traffic with
   runnable scripts (`scripts/verify-checksums.js`, `scripts/check-coverage.js`,
   `scripts/check-raw-consistency.js`).
4. Resolves the remaining native-transport questions (does `write()` need a
   report-ID byte? how many ACKs?) empirically, against real hardware, once
   there's Rust code to test with.

See `PROTOCOL.md` for the decoded format, `fixtures/` for the decoded
per-capture evidence (checksums and structure independently re-verified
against the real device), and `fixtures/raw/` for minimally processed
capture logs — `scripts/check-raw-consistency.js` checks each of
`fixtures/cap1.json`, `fixtures/page-switch.json`, and
`fixtures/clear-picture.json` against its own raw log byte-for-byte, in
exact order.

---

## 📡 Protocol reference

| Opcode | Name | What it carries |
|--------|------|------------------|
| `0x40` | Info package | Constant header + inner CRC-16, per command |
| `0x41` | Data packet | The actual payload (e.g. `[hour, minute, second]`) |
| `0x42` | Finish | Constant, no payload — commits the group |

Every 64-byte report is checksummed with a plain byte sum (**not** a CRC):
`opcode + byte1 + byte2 + length + sum(payload)`, stored 16-bit
little-endian at bytes 4-5 — for **every** opcode.

> 📝 Milestone 3 corrected this. Bytes 4-5 used to be documented as an 8-bit
> checksum plus a reserved zero for `0x41`/`0x42`, and bytes 1-2 as reserved.
> Byte 4 was always right; byte 5 is the checksum's **high** byte, and bytes
> 1-2 carry the bulk **data offset**. Every earlier command had a sum under
> 256 and a zero offset, so the two models agreed — until picture upload,
> where byte 5 is non-zero in 551 of 552 reports. No shipped behaviour
> changed; the old fixtures still pass byte-for-byte.

> ⚠️ Native `write()` to `/dev/hidraw*` needs a **leading `0x00` byte**
> (65 bytes total) — this unnumbered-report interface still wants the
> synthetic report-ID byte on write, confirmed against real hardware.
> `read()` does **not** get that prefix back (64 bytes). See `PROTOCOL.md`
> for the full writeup.

Full field-by-field mapping: `fields.json` (machine-readable) and
`PROTOCOL.md` (human-readable prose, hand-maintained alongside it -- not
mechanically generated, so treat `fields.json` as the source of truth if
the two ever disagree).

**Debug flags** (not needed for normal use): `--debug-no-prefix` sends
without the leading `0x00` byte — confirmed not to work, kept only to
re-run that experiment if the device's behavior ever needs re-checking.
`YUNZII_DEBUG=1` prints every sent report and received ACK in hex.

---

## 📄 License

MIT — see `LICENSE`.

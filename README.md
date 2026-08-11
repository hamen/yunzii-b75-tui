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

**⏰ `set-time`, 🖼️ `switch-page`, and 🧹 `clear-picture` work!** Their
protocols are fully decoded (see `PROTOCOL.md`) with native CLI commands.
`switch-page home` and `switch-page picture` are visually confirmed on real
hardware. `switch-page` does **not** have a `gif` option: cmd15's bytes are
resolved and proven byte-identical to the vendor's own tool, but neither
this repo's command nor the vendor's own actually switches the TFT to the
GIF page (tested with a real GIF uploaded) — some other operation is
required and not yet known, so it's decoded-but-deferred rather than
shipped not doing what it says (see `PROTOCOL.md`). "Clear GIF" is
similarly decoded-but-deferred (2 trailing payload bytes not yet
understood). No `ratatui` screen yet — CLI-only, done well. Sliders,
toggles, and image/GIF upload aren't implemented yet; each gets its own
reverse-engineering pass first, same process as `set-time` below. 🚧

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
```

`clear-picture` sends 32 reports (16 repeats); like `set-time`, a failure
partway through aborts the whole transaction rather than silently
continuing, but could leave the picture only partially cleared — the same
risk class as `set-time`'s partial-clock-update risk, not a new one.

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
`opcode + length + sum(payload)`, 8-bit for `0x41`/`0x42`, 16-bit
little-endian for `0x40`.

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

# Yunzii B75 Pro Max — native screen control for Linux (no browser needed)

The Yunzii B75 Pro Max keyboard has a small TFT screen (clock / picture / GIF).
The only configuration tool the vendor ships is a browser-based WebHID app at
[yunzii-game.com](https://yunzii-game.com/) (Chrome/Edge/Opera only, no native
Linux app) — it works, but it always needs a browser tab open.

Goal: a native Rust TUI that talks directly to `/dev/hidraw*` for this device,
with zero browser/WebHID dependency, eventually covering the same controls as
the site's "Screen Settings" panel (picture upload, GIF upload,
brightness/chroma/saturation, grayscale/fuzzy/sharpening toggles, image
placement, page switching, clear picture/GIF, update device time).

```
USB ID 28e9:31c8  GDMicroelectronics YUNZII B75 PRO MAX Keyboard
```

## Status

**Milestone 1: `set-time` works.** The clock/date protocol is fully decoded
(see `PROTOCOL.md`) and there's a native CLI for it — no `ratatui` screen
yet, just this one command. Sliders, toggles, and image/GIF upload are not
implemented yet (each needs its own protocol discovery phase first, same
process as `set-time`).

## Quick start

```bash
cargo build --release
sudo cp udev/99-yunzii-b75.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
# unplug and replug the keyboard, then:
./target/release/yunzii-b75-tui set-time
```

Requires: [Rust](https://rustup.rs) (for `bin/ci`'s `cargo` steps too), the
keyboard connected via USB-C (2.4G dongle / Bluetooth untested), and the
vendor's browser tab (if any) closed — WebHID and this tool can't hold the
device open at the same time.

## Hardware

Any unit that enumerates as USB `28e9:31c8` should work, since the config
channel is a standard QMK/VIA-style Raw HID interface (usage page `0xFF60`,
usage `0x61`, report ID 0). Interface numbering can vary by machine/session —
see `PROTOCOL.md` for how to re-identify it on yours.

## How the protocol was reverse-engineered

The vendor's config page talks to the keyboard over WebHID. Rather than
guessing at the byte format blind, this repo's discovery phase:

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

See `PROTOCOL.md` for the decoded format, `fixtures/` for the decoded
per-capture evidence (checksums and structure independently re-verified
against the real device), and `fixtures/raw/` for a minimally processed
capture log that `fixtures/cap1.json` is checked against byte-for-byte.

## License

MIT — see `LICENSE`.

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

**Protocol discovery in progress — no working tool yet.** This repo currently
documents the reverse-engineered HID protocol (see `PROTOCOL.md`) rather than
shipping a binary. The native TUI ships in a follow-up phase once enough of
the protocol is decoded to implement it.

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
   a runnable script (`scripts/verify-checksums.js`).

See `PROTOCOL.md` for the decoded format and `fixtures/` for the raw evidence.

## License

MIT — see `LICENSE`.

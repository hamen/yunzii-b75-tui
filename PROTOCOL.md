# Yunzii B75 Pro Max — screen control protocol (Phase 0: discovery)

Machine-readable source of truth: `fields.json`. Decoded per-capture evidence:
`fixtures/` (built by `scripts/build-fixtures.js` from short, independently
re-verified real byte sequences — see that script's header for exactly how —
with zero-padding computed programmatically, never hand-typed, after an
earlier version of this file had a real transcription bug in the padding).
`fixtures/raw/cap1-hidlog.json` is a minimally processed capture log;
`scripts/check-raw-consistency.js` (part of `bin/ci`) asserts `fixtures/
cap1.json` matches it byte-for-byte, so the decode is checked against real
captured evidence, not only against itself. Capture tooling:
`scripts/capture-hook.js`, `scripts/verify-checksums.js`,
`scripts/vendor-source-excerpt.js`, `scripts/check-coverage.js`,
`scripts/check-raw-consistency.js` (all three checks run in `bin/ci`).

This phase decodes **one command family** ("Update device time," which turns
out to be two vendor sub-commands: set-clock and set-date) completely, and
documents the generic report-wrapper protocol it uses — which will apply to
every other screen command too.

## How this was decoded

Two independent, cross-checked methods:

1. **Live capture**: a JS hook (`scripts/capture-hook.js`) injected into the
   vendor's config page at `https://yunzii-game.com/#/screen`, recording every
   `sendReport`/`sendFeatureReport` call and every `inputreport` event. The
   "Update device time" button was clicked 3 times at different real
   wall-clock moments; the resulting byte differences were diffed and
   cross-checked against elapsed time via modular arithmetic.
2. **Vendor source reading**: the site's already-loaded client-side JS
   (`https://yunzii-game.com/assets/index-8Bj3uPPc.js` — fetched from the
   page's own network requests in the browser, not extracted from firmware or
   any protected asset) contains the exact function that builds this command.
   An excerpt is in `scripts/vendor-source-excerpt.js`.

Both methods agree on every byte. `scripts/verify-checksums.js` re-derives
every checksum in the fixtures from first principles and confirms them
(`node scripts/verify-checksums.js`).

## Interface identity

VID `0x28E9` / PID `0x31C8` ("GDMicroelectronics YUNZII B75 PRO MAX
Keyboard"). The device exposes 4 separate HID interfaces under this VID:PID;
the config channel is the one with usage page `0xFF60` / usage `0x61` (the
standard QMK/VIA "Raw HID" usage), report ID 0, 64-byte input AND output
reports. This is opened automatically by the vendor's page on connect.
Interface *numbering* (which `hidraw*` node this maps to) varies by
machine/session — re-enumerate at runtime by usage page/usage, not by a fixed
device index or node number.

**Confirmed from the Linux side too**, not just WebHID (see
`fields.json`'s `linuxInterfaceIdentity` for full detail): this machine
exposes the same 4 interfaces as 4 `hidraw` nodes. The config channel's real
sysfs report descriptor (`0660ff0961a1010962150026ff0095407508810209631500
26ff00954075089102c0`, SHA-256 `a30039d0…`) decodes to exactly usage page
`0xFF60` / usage `0x61` with 64-byte input and output reports and no Report
ID item (confirming the unnumbered-report / `reportId: 0` case) —
independently matching what WebHID reported, from an entirely different
vantage point. Its USB interface number is `1`. The product string
(`YUNZII B75 PRO MAX Keyboard`) matches; the device exposes no serial
number. `getfacl` confirms the udev rule's ACL correctly grants
read/write access on all 4 of the device's `hidraw` nodes on this machine,
not just some.

Connection mode tested: USB-C cable. 2.4G dongle / Bluetooth not yet tested
— documented as untested, not assumed to work.

## Outer report structure (generic — applies to every screen command)

The vendor's own opcode table (`scripts/vendor-source-excerpt.js`):

| Opcode | Name |
|---|---|
| `0x40` | `sendScreenControlInformationPackage` |
| `0x41` | `sendScreenControlDataPacket` |
| `0x42` | `finishScreenControlDataPacket` |
| `0x55` | `getDongleAndKeyboardStatus` (device ack) |
| `0xB0`-`0xB7` | firmware/DFU opcodes (unrelated to screen control) |

Every 64-byte report — the exact layout differs slightly by opcode (see
`fields.json`, the source of truth this table is generated from):

```
0x41 / 0x42:
offset  0  : opcode (0x41 / 0x42)
offset  1-2: 0x00 0x00 (reserved)
offset  3  : length (of the payload that follows)
offset  4  : checksum8
offset  5  : 0x00 (reserved)
offset  6  : status -- 0x00 outbound, 0x55 in the device's ACK
offset  7-63: payload (length bytes), then 0x00 padding

0x40 (no reserved byte between the checksum and status -- the checksum is
2 bytes wide here, occupying what would otherwise be the reserved slot):
offset  0  : opcode (0x40)
offset  1-2: 0x00 0x00 (reserved)
offset  3  : length (of the payload that follows)
offset  4-5: checksum16, little-endian [low byte, high byte]
offset  6  : status -- 0x00 outbound, 0x55 in the device's ACK
offset  7-63: payload (length bytes), then 0x00 padding
```

**There is no multi-report fragmentation.** An earlier hypothesis (before
this phase) assumed opcodes 0x40/0x41/0x42 were fragments of one oversized
logical message. That's wrong: each is an independently complete report type
with its own opcode, length, and checksum. No "reassembly" step exists.

### Checksum — a plain byte sum, not a CRC

```
checksum = (opcode + length + sum(payload_bytes))
0x41 / 0x42 reports: 1 byte  = checksum & 0xFF, at offset 4
0x40 reports:        2 bytes = [checksum & 0xFF, (checksum >> 8) & 0xFF], at offsets 4-5
```

Verified exactly against all 3 live captures plus the vendor's own hardcoded
constants — see `scripts/verify-checksums.js` (9/9 checks pass).

There is a **separate, unrelated** CRC-16/ARC function in the vendor source
(polynomial `0xA001` reflected, init `0xFFFF`) used only to precompute two
constant inner values at build time (`[9,0,3]→[195,225]`, `[10,0,4]→[1,80]`
— see below). It plays no role in the outer report checksum. Don't conflate
the two.

## "Update device time" = two vendor commands, sent 3x each

The vendor's click handler (deminified, see `scripts/vendor-source-excerpt.js`
for the real minified source):

```js
const updateDeviceTime = async () => {
  const hour = now.hour(), minute = now.minute(), second = now.second();
  const year2 = Number(now.format("YY"));
  const month = now.month() + 1;
  const date = now.date();
  const weekday = now.day() || 7; // Mon=1..Sun=7

  const D = [165, 90, 9, 0, 3, 195, 225];   // constant: cmd-9 header
  const T = [165, 90, 10, 0, 4, 1, 80];     // constant: cmd-10 header
  const P = [hour, minute, second];
  const M = [year2, weekday, month, date];

  for (let i = 0; i < 3; i++) {
    await sendScreenControlInformationPackage(D);
    await sendScreenControlDataPacket(P);
    await finishScreenControlDataPacket();
    await sendScreenControlInformationPackage(T);
    await sendScreenControlDataPacket(M);
    await finishScreenControlDataPacket();
  }
};
```

- **cmd 9 (set clock)**: info-package `[0xA5,0x5A,0x09,0x00,0x03,0xC3,0xE1]`
  (constant), then a data-packet payload `[hour, minute, second]`.
- **cmd 10 (set date)**: info-package `[0xA5,0x5A,0x0A,0x00,0x04,0x01,0x50]`
  (constant), then a data-packet payload `[year2digit, weekday, month, date]`.
- Both groups end with a `finish` (0x42) report — always identical bytes
  (`38 7a 00...`, all-zero payload) regardless of which command preceded it.
- The whole sequence repeats exactly 3 times per click — a literal loop in
  the vendor's code, not a retry-on-failure heuristic.
- Every outbound report gets exactly 2 identical `inputreport` ACKs from the
  device (byte offset 6 flips `0x00→0x55`) **as observed via WebHID in the
  browser**. This is Phase 0's original finding and is left as-is here for
  history — but see "Linux hidraw write/read byte layout" below: at the
  native hidraw layer, only 1 real ACK arrives per write. The "2 ACKs"
  count was specific to the WebHID/Chrome transport, not the wire protocol.

Local time is used throughout (`hour()`/`minute()`/etc. on a JS `Date`
object, which read local time unless explicitly converted) — no explicit
UTC conversion in the vendor code.

## Live capture cross-validation

| Capture | Elapsed since previous | hour | minute | second | Checksum predicted = observed |
|---|---|---|---|---|---|
| cap1 | — | 19 | 24 | 13 | `0x7c` = `0x7c` ✓ |
| cap2 | +241.05s | 19 | 28 | 14 | `0x81` = `0x81` ✓ |
| cap3 | +101.08s | 19 | 29 | 55 | `0xab` = `0xab` ✓ |

Minute/second deltas were cross-checked against real elapsed time via modular
arithmetic (e.g. second 13 + 241s = 254s = 4×60+14 → second 14, minute +4)
independently of the vendor-source reading — both methods agree exactly.
Date payload in cap3 (`26,1,8,10`) matches the actual capture date,
2026-08-10 (a Monday), exactly.

## Linux hidraw write/read byte layout — resolved (Milestone 1)

Phase 0 left this open; Milestone 1's native Rust implementation resolved it
empirically against real hardware (see `fields.json`'s
`linuxHidrawTransport` for full detail):

- **`write()`** to `/dev/hidraw5` needs a **leading `0x00` byte prepended**
  before the 64-byte report — 65 bytes total. This is the documented Linux
  kernel behavior for unnumbered-report HID devices (confirmed correct,
  not just theorized — writing 64 bytes with no prefix produces a
  malformed reply from the device, not silence).
- **`read()`** returns exactly 64 bytes, with **no** such prefix —
  asymmetric with `write()`.
- **Correction to Phase 0's ACK-count finding**: Phase 0's WebHID capture
  showed exactly 2 identical ACKs per outbound report. At the native
  hidraw layer, only **1** real ACK arrives per write — the "2 ACKs"
  pattern was a WebHID/Chrome-side artifact, not a wire-protocol fact.

Confirmed by sending the full 18-report "set time" sequence and visually
verifying the TFT screen showed the correct **hour, minute, and date** —
then reconfirmed the next day when the keyboard's own clock had correctly
ticked forward overnight to match real time. **Weekday was not visually
confirmed** — the TFT screen has no weekday field at all, so there's
nothing to check on-screen. Weekday correctness instead rests on the
encoding logic being unit-tested against every day of the week
(`src/time.rs`) plus the device accepting the byte via a valid ACK — weaker
evidence than a visual check, and intentionally not folded into the same
"confirmed" claim as hour/minute/date.

## What's resolved vs. not (see `fields.json`'s `unresolved` list for detail)

**Resolved** — every transmit-required field for "Update device time": HID op
type (output report), report ID (0), interface identity from BOTH the WebHID
side (VID+PID+usage-page+usage) AND the Linux side (sysfs report descriptor,
hash, USB interface number, ACL — see `fields.json`'s
`linuxInterfaceIdentity`), opcode table, outer report structure, both
checksum variants, the full clock+date payload layout, AND (as of Milestone
1) the native hidraw write/read byte layout and real ACK count, above.

**Unresolved** (named, not silently missing): the numeric meaning of the
`finish` command's constant length byte (`0x38`); opcodes for other screen
commands (switch page, clear picture/GIF) — identified by name from the
vendor source but not yet independently HID-captured; picture/GIF upload
payload format entirely (out of scope so far).

## What's next

Milestone 1 (this repo's native `set-time` CLI, see `README.md`) already
ships the Rust implementation for this one command. What's left: a
`ratatui` screen (once there's more than one action worth navigating
between), and Milestones 2/3 (sliders, toggles, image/GIF upload) — each
needs its own discovery pass first, same process as this document. The
generic opcode/checksum/report-structure model above should carry over
directly; only the opcode-specific payload layout will differ per command.

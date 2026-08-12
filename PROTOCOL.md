# Yunzii B75 Pro Max — screen control protocol (Phase 0: discovery)

Machine-readable source of truth: `fields.json`. Decoded per-capture evidence:
`fixtures/` (built by `scripts/build-fixtures.js` from short, independently
re-verified real byte sequences — see that script's header for exactly how —
with zero-padding computed programmatically, never hand-typed, after an
earlier version of this file had a real transcription bug in the padding).
`fixtures/raw/*.json` are minimally processed capture logs;
`scripts/check-raw-consistency.js` (part of `bin/ci`) asserts each of
`fixtures/cap1.json`, `fixtures/page-switch.json`,
`fixtures/clear-picture.json`, `fixtures/picture-upload.json` and
`fixtures/gif-upload.json` matches its own raw log byte-for-byte, in exact
order, so the decode is checked against real captured evidence, not only
against itself. The picture-upload fixture is the strongest of the five: it is
regenerated from its source image through the model, so that check is
model-versus-hardware rather than file-versus-copy. The gif-upload fixture is a
labelled hybrid -- see its section below. Capture tooling:
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
number.

Access to those nodes was originally granted on all 4 interfaces, and this
document used to record that as a success. It was the opposite: interface 0
carries the keystrokes, so a rule matching the whole device gives any process
running as the logged-in user a keylogger. The rule now matches interface 1
alone -- see `udev/99-yunzii-b75.rules`, which also explains the udev
same-parent constraint that makes the obvious spelling of that rule match
nothing at all.

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

One layout, all three opcodes:

```
offset  0  : opcode (0x40 / 0x41 / 0x42)
offset  1-2: data offset, u16 little-endian
offset  3  : length (of the payload that follows)
offset  4-5: checksum16, little-endian [low byte, high byte]
offset  6  : status -- 0x00 outbound, 0x55 in the device's ACK
offset  7-63: payload (length bytes), then 0x00 padding
```

Offsets 1-2 are zero for every command except bulk pixel data, where they
carry the data offset -- continuous across the frame for picture upload,
restarting every 1024 bytes for GIF. Earlier phases described these two bytes
as reserved and split the checksum by opcode; see the correction below.

**Report size is 64 bytes.** Worth stating because the vendor's JavaScript
builds a **63**-byte array and calls `sendReport(0, data)`: Chrome then
zero-pads it up to the output report size the device declares. That
declaration was read from Linux sysfs for this exact keyboard
(`/sys/class/hidraw/*/device/report_descriptor`) and says `Report Count =
64, Report Size = 8` with no Report ID item, for both directions. So the
64th byte is real, and it is always `0x00`. Any capture taken from the
browser side is one byte short of the wire report and must be padded, not
trusted as-is.

**There is no multi-report fragmentation.** An earlier hypothesis (before
this phase) assumed opcodes 0x40/0x41/0x42 were fragments of one oversized
logical message. That's wrong: each is an independently complete report type
with its own opcode, length, and checksum. No "reassembly" step exists.

### Checksum — a plain byte sum, not a CRC

```
sum      = opcode + byte1 + byte2 + length + 0 + 0 + 0 + sum(payload_bytes)
checksum = [sum & 0xFF, (sum >> 8) & 0xFF]   at offsets 4-5, for EVERY opcode
```

Bytes 4-6 count as zero while summing: the vendor builds the array with
`0x00` placeholders in those three slots, sums it, then writes the two
checksum bytes back into offsets 4 and 5.

#### Correction (Milestone 3) — this replaces what earlier phases documented

Phases 0-2 described `0x41`/`0x42` as carrying an **8-bit** checksum at
offset 4 with a **reserved zero** at offset 5, and offsets 1-2 as reserved.
Two of those three claims were wrong:

| Byte | Old claim | Actually |
|---|---|---|
| 4 | 8-bit checksum (0x41/0x42) | **Correct** — it is the low byte either way |
| 5 | reserved, always `0x00` | The checksum's **high** byte |
| 1-2 | reserved, always `0x00` | The bulk **data offset**, u16 little-endian |

Why it went unnoticed for two milestones: every command decoded before
picture upload had a total sum below 256 **and** a zero offset, so the two
models emit identical bytes. Picture upload is the first command where they
diverge, and it diverges hard — in the 552-report capture, byte 5 is
non-zero in **551** of them.

This is a documentation and model correction, not a behaviour change. Every
`set-time`, `switch-page`, and `clear-picture` fixture still passes
byte-for-byte under the unified formula, which is exactly the regression
guard the refactor needed.

Verified against the vendor's own hardcoded constants and against every
report of every capture — see `scripts/verify-checksums.js`, which now runs
the one formula over all 552 real picture-upload reports and reports how
many fit the old model (one: `finish`, whose sum is `0x7a`).

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

## Page switch and clear-picture — resolved (Milestone 2)

Four more commands, live HID-captured on 2026-08-11 (`fixtures/page-switch.json`,
`fixtures/clear-picture.json`, and their `fixtures/raw/` evidence):

| Button | Inner cmd | Sequence | Info-package payload |
|---|---|---|---|
| Switch to the homepage | 11 | infoPackage → finish (2 reports) | `[165,90,11,0,0,2,0]` |
| Switch to the picture page | 13 | infoPackage → finish (2 reports) | `[165,90,13,0,0,3,224]` |
| Switch to the GIF page | 15 | infoPackage → finish (2 reports) | `[165,90,15,0,0,195,65]` |
| Clear the picture | 14 | infoPackage → finish, **repeated 16x** (32 reports) | `[165,90,14,0,0,3,16]` |

None of these send a follow-up `0x41` data-packet — unlike `set-time`, the
info-package alone carries the whole command, and `finish` (the same
constant `38 7a 00...` bytes as `set-time`'s) closes it out. The 16x repeat
for "Clear the picture" matches the vendor source's literal
`for(a=0;a<16;a++)` loop and was independently confirmed by counting the
full raw HID log for one click, not just trusting the source.

**Real-hardware verification (Milestone 2, 2026-08-11)**: `switch-page
home` and `switch-page picture` were visually confirmed on the physical
TFT — the screen genuinely changed to the expected page each time,
including switching correctly right after a `clear-picture` run.
`switch-page home` and `switch-page picture`, each while already on that
page, were both a clean no-op (no error). `clear-picture` was visually
confirmed to remove the picture from the screen, and running it again on
an already-empty slot was a clean no-op.

> **Superseded by Milestone 4 — read this section as history, not as current
> state.** The conclusion below ("do not ship `switch-page gif`") was correct
> for what was known at the time and is wrong now. cmd15 was never at fault:
> the test GIF had only ever been saved in the vendor's **mode 0**, which
> stores frames that never play. `switch-page gif` and `set-gif` both ship as
> of Milestone 4. See [The GIF-page mystery, solved](#the-gif-page-mystery-solved)
> for the resolution. The investigation is kept because the reasoning — and
> how it reached a confident wrong answer — is worth more than the verdict.

**"Switch to the GIF page" (cmd15) — investigated in round 2, deferred in
round 3**: round-1 review (codex Blocker, cursor Should-fix) correctly
flagged that the round-1 hardware run had no GIF on the device, so a "no
visible change" result couldn't be distinguished from an actual bug. To
resolve this properly (not just argue about severity), a real disposable
test GIF was uploaded to the device via the vendor's own browser tool
(at the time, this repo had no upload command — `set-gif` did not exist
until Milestone 4) and saved successfully. Sending cmd15 via this repo's
own builder was then tried **twice** — still no visible change, screen
stayed on whatever page was already showing. To rule out a bug in this
repo's own cmd15 bytes as the cause, the SAME "Switch to the GIF page"
button was then clicked directly in the vendor's own official tool, with
`scripts/capture-hook.js` recording the actual bytes sent: **`40 00 00 07
59 02 00 a5 5a 0f 00 00 c3 41 00...`**, byte-for-byte identical to
`CMD15_INFO_PAYLOAD` — and the vendor's own tool **also** failed to switch
the display. This is decisive: this repo's cmd15 bytes are exactly what
the vendor's own reference implementation sends, and neither one visually
switches to the GIF page under these real conditions (GIF present, sent
via HID, keyboard otherwise idle). This is a device/firmware behavior this
repo's protocol layer cannot influence — not a wire-format bug — but *why*
the display doesn't switch (needs to be set as "startup animation" first?
needs a physical button press to cycle screens? something else?) is not
understood.

**Decision (round 3, codex Blocker)**: shipping `switch-page gif` as a CLI
command would ship something that sends a correct, ACK'd command but
doesn't do what its name says — the same half-understood-shipping trap
"Clear GIF" was already kept out of this milestone for. `Page::Gif` and
`CMD15_INFO_PAYLOAD` stay in `protocol.rs`, fully resolved and tested, but
`main.rs`'s `switch-page` CLI only accepts `home`/`picture`. `gif` is now
a named, evidenced `unresolved` entry in `fields.json`, the same pattern
as "Clear GIF," for a follow-up milestone to pick up once the missing
operation is found. *(Milestone 4 found it: the missing operation was
saving in mode 1. `switch-page gif` ships.)*

**Whether `clear-picture` leaves a separately-stored GIF untouched is
still open, but for a smaller reason now.** The blocker described here —
that the GIF page could not be reached at all, so a corrupted GIF and a
non-displaying page looked identical — is gone: the page displays. A
Milestone 4 hardware run did confirm the narrower direction, that a
**picture** upload leaves a stored GIF intact. The reverse, `clear-picture`
against a stored GIF, has not been run.

**"Clear GIF" is a different, deferred command — not shipped.** It looked
at first like it might share a handler with "Clear the picture" (the
vendor's own internal function name, `clearPictureOrGif_loop16x`, implies
that), but live capture shows it's structurally unrelated: two different
inner commands (18, then 19), each sent once — no 16x loop — each with a
9-byte info-package payload (2 bytes longer than every other command here).
Both fit the established checksum pattern, but 2 trailing bytes on each
(`[1,0]`) have unknown meaning, and no data-packet is ever sent despite the
payload's `N` byte looking like it should signal one. See `fields.json`'s
`unresolved` list for the full detail and what a follow-up capture would
need to resolve it — deferring an under-understood command rather than
shipping it on "the checksum matches" is the same discipline `set-time`'s
still-unexplained `finish` length byte gets.

## Picture upload — resolved (Milestone 3)

`yunzii-b75-tui set-picture <file>`. 552 reports for one full frame.

```
infoPackage(0x40, cmd16)        start upload
   ... host sleeps 300 ms ...
dataPacket(0x41, cmd12)         declare 30720 bytes of pixel data
dataPacket(0x41) x 549          the pixels, 56 bytes per report
finish(0x42)
```

**The 300 ms pause goes between the start report and declare-size**, not
before the bulk data. That is the order in the vendor's own source
(`await r(re); await sleep(300); await o(Ue); await o(xe); await i()`) and
it is where the gap appears in the captured timestamps: 300 ms between the
start ACK and the declare-size write, and none before the first pixel
packet. Two plan reviewers independently expected the opposite; the capture
settled it.

### Bulk packets

- **56 bytes each**, which is the vendor's own `reqLen - 7` — the report
  size minus the 7-byte header.
- **Offset in bytes 1-2**, u16 little-endian, walking 0, 56, 112, … 30688.
- **The length byte is the real remaining count.** The last packet declares
  `0x20` (32), *not* a zero-padded `0x38`: `548 × 56 + 32 = 30720` exactly.
  This is easy to get wrong and it does not fail quietly — the length byte
  is summed into the checksum, so padding it corrupts that packet's
  checksum too.

### Pixel format

- Panel is a fixed **160×96**, so a frame is always **30720 bytes**.
  Confirmed twice over: the byte count, and the vendor bundle's per-product
  screen table, which maps this product ID to `{width:160, height:96}`.
- **RGB565, big-endian per pixel**: `v = (R>>3)<<11 | (G>>2)<<5 | (B>>3)`,
  emitted `[v>>8, v&0xFF]`. Row-major, top row first, left to right.
- **Nearest-neighbour** resize, matching the vendor's
  `imageSmoothingEnabled = false`. An interpolating filter would produce
  different bytes for the same input file.
- Aspect ratio is **not** preserved; the image is stretched to fill.

#### Alpha: only alpha 0 goes black — this is not premultiplication

The vendor's picture encoder ignores the alpha channel outright:

```js
function te(J){ const ae=J.data; ...
  const xe=ae[he], ve=ae[he+1], re=ae[he+2];   // bytes 0-2 only
  ... }
```

Its input is a `getImageData()` from a **freshly created canvas with no black
fill**, and drawing source-over onto a transparent backdrop preserves the
un-premultiplied colour. So:

| Source pixel | On the panel |
|---|---|
| `rgba(255,0,0,255)` | full red |
| `rgba(255,0,0,128)` | **full red** — alpha discarded, not blended |
| `rgba(255,0,0,0)` | black — the canvas reads it back as `(0,0,0,0)` |

A cross-review round read this repo's plan prose and asked for
`out = src * alpha` instead (PR #4). That formula was a guess in the plan,
never the device's behaviour. The `if (data[i+3] === 0)` black pre-pass that
does exist in the vendor bundle belongs to its **GIF** path, which is a
different code path from the one this milestone implements.

Practical consequence: a logo with soft, semi-transparent edges will show
those edge pixels at full colour against black rather than faded into it.
Pre-flatten the file onto the background you want if that matters.

### How this was verified

`fixtures/picture-upload.json` is not a transcription of the capture. It is
**regenerated from `fixtures/test-quadrants.png` through the model above** by
`scripts/build-fixtures.js`, and then compared against
`fixtures/raw/cap-picture-upload-hidlog.json` — real traffic read out of the
vendor tool's own WebHID calls — by `scripts/check-raw-consistency.js`.

That makes the check model-versus-hardware rather than file-versus-copy, so
a mistake in channel order, per-pixel byte order, row order, chunk size,
offset encoding, the final length byte, or the checksum shows up as a byte
mismatch in `bin/ci`. Result: **0 mismatches across all 552 reports and all
30720 pixel bytes.**

The test image is a four-quadrant + gradient pattern precisely because solid
colours cannot distinguish any of those things.

### Hardware gate (2026-08-11, all confirmed visually on the real panel)

| Check | Input | Result |
|---|---|---|
| Byte-exact reproduction | `test-quadrants.png`, 160×96 | Red TL, green TR, blue BL, gradient BR — as sent by the vendor's own tool |
| Transparency → black | 200×120 RGBA, no background | Yellow circle and blue rectangle on a **black** field |
| Resize from a different size | 200×120 and 640×360 | Both fill the panel correctly |
| JPEG decoding | 640×360 JPEG | Gradient rendered, **white square top-left**, black bottom-right — so row and column order are right end to end |
| Re-upload replaces cleanly | three uploads in a row | Each fully replaced the last, no tearing or leftovers |
| Does upload switch pages? | panel left on the home page | **Yes** — the clock stayed up during the upload and the image appeared when it finished. No `switch-page picture` needed. |
| `switch-page` still works after | `switch-page home` between uploads | Clock and battery returned normally |

## GIF upload — resolved (Milestone 4)

`yunzii-b75-tui set-gif <file.gif> [--fps N] [--max-frames N]`. 1149 reports
for a 2-frame GIF.

```
infoPackage(0x40, cmd18)   open session   [.., mode, 0]
dataPacket (0x41, cmd19)   open session   [.., mode, 0]
   ── per frame ──
   dataPacket(0x41, cmd16)  frame header  [.., 2, mode, frameIndex]
   dataPacket(0x41, cmd17)  declare 30720 bytes
   30 x (1024-byte block -> 19 bulk packets)
dataPacket(0x41, cmd18)    close session  [.., mode, FRAME COUNT]
dataPacket(0x41, cmd19)    close session  [.., mode, FPS]
finish(0x42)
```

`2 + frames × (2 + 30 × 19) + 2 + 1`, which is 1149 for two frames — exactly
the captured count.

**Opcode quirk, preserved deliberately:** the *opening* cmd18 goes out as an
info package (`0x40`); everything else in the transaction, the *closing* cmd18
included, is a data packet (`0x41`). Milestone 2's "Clear GIF" capture sent
both as `0x40`, so those samples are a different sequence and not a
cross-check for these.

**The inner CRC does not cover the trailer.** `ga()` is computed over the fixed
`[cmd, 0, len]` triple only. The mode, frame index, frame count and rate bytes
sit after it and are covered by the outer 16-bit checksum like any other
payload byte. Folding them into `ga()` is an easy mistake.

### Offsets restart every 1024 bytes — the one real difference from pictures

Picture upload runs a single continuous offset, 0 … 30688, across the whole
frame. **GIF does not.** Each frame is pushed one 1024-byte block at a time,
and each block is chunked independently, so the offset field **restarts at 0
for every block** and never reaches 1024.

```
1024 = 18 × 56 + 16   → 19 packets per block, the last declaring 16 bytes
30720 / 1024 = 30 blocks per frame
```

Milestone 3's text said GIF "reuses the same 56-byte bulk packets". True of the
packet *format*, misleading about the chunker. The two builders are kept
separate in `protocol.rs` on purpose, and
`gif_offsets_are_block_relative_unlike_picture_uploads_continuous_run` fails if
anyone unifies them.

### Modes

| Mode | Vendor button | Max frames | Frame size | Shipped |
|---|---|---|---|---|
| 0 | Set it as the startup animation | 64 | 160×96 | no |
| **1** | **Save to the device** | **160** | 160×96 | **yes** |
| 2 | Save GIF to the device home page | 42 | 96×64 | no |

Mode 0 is the one that cost Milestone 2 a round of investigation: it stores
frames somewhere that never plays. Mode 2's button renders only for product ID
12463 (this keyboard is 12744) — a UI gate, not a wire one, but it is a
different frame size for a device we do not have.

### Frame rate

One rate for the whole animation, sent once as the last byte of the closing
cmd19. Slider range 1–60, default 30. **It is literal frames per second**, so a
2-frame GIF at 30 fps strobes; short animations want a low `--fps`.

### Host sleeps — every gap, including the ones that are zero

| Position | Sleep |
|---|---|
| open cmd18 → open cmd19 | none |
| after open cmd19 | 30 ms |
| after each frame's cmd16 | **3000 ms when `frameIndex % 16 == 0`** (0-based), else 30 ms |
| cmd17 → first block | none |
| after **every** 1024-byte block, last included | 30 ms |
| last block → next frame's cmd16 | none |
| after each of the two close reports | 30 ms |
| before `finish` | **500 ms** |

The 3000 ms is almost certainly a flash-write pause. None of these are tuned.

### Honest limit: pixel bytes are ours, not the vendor's

Milestone 3 could claim zero mismatches against the vendor's own bytes for the
whole picture upload. **That claim is not available here, and this section
exists so nobody assumes it is.**

The vendor's GIF frame pipeline is not its picture pipeline: per frame it runs
a three-stage canvas downscale (3× → 1.5× → 1×) with smoothing on, onto a
black-filled canvas, then an edge-aware filter. Browser resampling is
implementation-defined and not reproducible outside a browser.

So this repo converts frames its own way — the same nearest-neighbour path
`set-picture` uses, so the two commands agree with each other. What *is*
verified byte-for-byte against the capture is everything that can break the
device: framing, ordering, block boundaries, per-block offsets, length bytes
and every checksum. `fixtures/gif-upload.json` is a labelled **hybrid** — model
-generated control and chunking, captured pixel bytes — and says so in its own
provenance note.

One consequence worth knowing: GIF transparency is a single transparent
*index*, so frames arrive with alpha 0 or 255 and are composited onto opaque
black. That matches the vendor's black-filled canvas.

### Two resource limits, and why each number

A GIF is an untrusted input: a small file can declare an enormous canvas or an
enormous frame count. Two bounds keep `set-gif` from turning that into memory
exhaustion.

**64 MiB decode allocation** (`image::Limits::max_alloc`, set on the decoder
before any frame is read). This caps what the decoder may allocate for a
single frame buffer. The largest frame this tool has any use for is one panel,
160×96 RGBA, which is 61,440 bytes — four orders of magnitude below the limit.
The headroom is deliberate: source GIFs are routinely far larger than the panel
and get downscaled, so the limit must not reject ordinary input. It exists to
stop a decompression bomb, not to enforce a sensible size.

**4.9 MB of encoded frames held at once.** `GIF_MAX_FRAMES` is 160, the
device's own ceiling, and each encoded frame is `PICTURE_BYTES` = 30,720 bytes
of RGB565. 160 × 30,720 = 4,915,200 bytes is therefore the worst case the
frame vector can reach, and it is bounded by the device limit rather than by a
separate check. This is why `--max-frames` above 160 is refused at the command
line: the bound only holds because that value cannot be exceeded.

Note what these do *not* bound: decoding walks every frame of the source in
order, even the ones `--max-frames` skips, because a skipped frame still
mutates the canvas later frames build on. A GIF with a hundred thousand frames
is slow to read. It is not, however, a memory problem — only the selected
frames are kept.

### Frame construction is delegated, on purpose

Real GIFs are optimised: most frames are a small sub-rectangle that only means
anything once composed onto what came before, using the file's disposal method
and transparent index. This repo does not hand-roll that — `image`'s
`into_frames()` applies position, transparency and disposal and yields
full-canvas frames.

When subsampling with `--max-frames`, **every** frame is still walked in order;
only the selected ones are encoded. A skipped frame still mutates the canvas
that later frames build on.

Two fixtures keep this honest, and it takes two, because one of them cannot
fail for the reason its name suggested.

`fixtures/test-anim-disposal.gif` has a second frame that is a 16×12
sub-rectangle at (48,36), with disposal "do not dispose". The test asserts the
first frame's mark is still there in the second. That proves **placement and
composition**: an implementation encoding raw sub-frames would produce a 16×12
buffer, or stretch a 16×12 blue square across the whole panel. It does **not**
prove disposal is honoured — "do not dispose" means keep the canvas, which is
also exactly what a decoder ignoring disposal altogether would do. The test was
named as though it covered disposal, and reviewing Milestone 4 caught that.

`fixtures/test-anim-disposal-background.gif` is the one that can fail. Frame 0
is the full canvas with a red mark and disposal **restore to background**, so
the canvas must be cleared before frame 1 — a small green rectangle elsewhere —
is drawn. The test asserts the red mark is `0x0000` in frame 1. Ignore
disposal and it stays red, and the test fails.

Both, along with the delay fixtures behind the frame-rate tests, are built by
`scripts/make-test-gifs.js`, a dependency-free GIF89a writer. Committed binary
fixtures that nobody can regenerate are their own kind of unverifiable.

## What's resolved vs. not (see `fields.json`'s `unresolved` list for detail)

**Resolved** — every transmit-required field for "Update device time": HID op
type (output report), report ID (0), interface identity from BOTH the WebHID
side (VID+PID+usage-page+usage) AND the Linux side (sysfs report descriptor,
hash, USB interface number, ACL — see `fields.json`'s
`linuxInterfaceIdentity`), opcode table, outer report structure, both
checksum variants, the full clock+date payload layout, AND (as of Milestone
1) the native hidraw write/read byte layout and real ACK count, above.
**As of Milestone 2**: page-switch bytes for all of home/picture/gif (cmd
11/13/15) and clear-picture (cmd 14), above. cmd15's bytes were resolved here
and were correct all along; Milestone 2 withheld it as a CLI command for
reasons Milestone 4 disproved, and it ships now.

**As of Milestone 3**: the full picture-upload format (cmd16 start, cmd12
declare-size, the bulk packet layout, and the RGB565 pixel encoding), plus the
checksum correction above.

**As of Milestone 4**: GIF upload end to end -- the cmd18/cmd19 session pair
and what their trailing bytes mean, the per-frame cmd16/cmd17 headers, the
1024-byte block chunking with its restarting offsets, the mode table, and the
frame-rate byte. `switch-page gif` ships too; it was correct all along.

**Unresolved** (named, not silently missing): the numeric meaning of the
`finish` command's constant length byte (`0x38`); whether `clear-picture`
affects a stored GIF; GIF save modes 0 and 2, decoded but never exercised; the
2.4 GHz dongle and Bluetooth connection modes; any connect/init traffic before
the first command.

### The GIF-page mystery, solved

Milestone 2 recorded that "switch to the GIF page" (cmd15) did not visibly
switch the panel, even though its bytes were proven identical to the vendor's
own button, and left it open.

cmd15 was never the problem. The vendor's GIF save has **three modes** (see the
table above), and every test before Milestone 3 used **mode 0**, which stores
frames somewhere that never plays -- not on the GIF page, and not at power-up
either (tested by replugging: the panel comes back to the home screen).
Re-saving with **mode 1** makes the GIF play immediately. There was simply
nothing on the GIF page to show.

That also re-labelled cmd18/cmd19: they are GIF session **open** and **close**,
and their "unexplained trailing bytes" are the mode, the frame count and the
frame rate. Milestone 4 implements them.

## What's next

Milestones 1-4 ship `set-time`, `switch-page home`/`picture`/`gif`,
`clear-picture`, `set-picture` and `set-gif` -- see `README.md`.

What is left: the sliders and toggles (brightness, chroma, saturation,
grayscale, "fuzzy", sharpening), the vendor's "in the middle" vs "cover up
completely" placement setting, a `clear-gif` command if one exists, and the
`ratatui` screen this repo is named after -- which is now worth building, since
there are finally more actions than a flat CLI wants to carry.

Each still needs its own discovery pass, same process as this document. The
generic opcode / checksum / report-structure model carries over directly; only
the per-command payload layout differs.

// Builds fixtures/*.json from REAL captured evidence, not from the abstract
// checksum model (a prior version of this script regenerated everything from
// the model, which made check-coverage.js circular -- caught in PR #1
// cross-review round 2). This version:
//
//  1. Hardcodes only the SHORT, real, independently re-verified byte
//     sequences actually observed on the wire (never more than ~14 tokens
//     typed by hand at once -- long hand-typed zero-padding runs are exactly
//     where a real transcription bug slipped through earlier in this repo's
//     history: every 0x40/0x41 OUT report was one padding byte short).
//  2. Derives the zero-padding programmatically (`padTo64`), never by typing
//     more than a few zeros in a row.
//  3. For cap1, every report pair (OUT + its real device ACK) was
//     byte-count-validated and cross-checked against each other (ACK == OUT
//     with only byte[6] flipped 0x00->0x55) before being hardcoded here --
//     see the disposition comment on PR #1 for the verification steps.
//  4. The checksum bytes below are REAL observed values (confirmed via the
//     device's own ACK, which echoes them back). check-coverage.js
//     INDEPENDENTLY recomputes what the checksum should be from
//     opcode+length+payload and asserts it matches -- this is now a true
//     independent check of the decoded formula against real hardware
//     evidence, not a check of the model against itself.
//  5. The three info-package (0x40), finish (0x42), and cmd10-date-data
//     (0x41) reports were independently confirmed byte-identical across all
//     3 captures in this session (D/T are vendor-source constants; the date
//     payload never changed within the ~6-minute capture window) -- so cap2
//     and cap3 reuse cap1's fully-verified bytes for those, and only supply
//     their own short, independently-observed cmd9-dataPacket bytes (the one
//     report that actually varies with time).
//
// Run with `node scripts/build-fixtures.js` to regenerate.

const fs = require('fs');
const path = require('path');

function padTo64(bytes) {
  const out = bytes.slice();
  while (out.length < 64) out.push(0);
  if (out.length !== 64) throw new Error(`padTo64: got ${out.length}, expected <=64 input`);
  return out;
}

function toHexBytes(bytes) {
  return bytes.map((b) => b.toString(16).padStart(2, '0')).join(' ');
}

// --- Real, verbatim-verified byte sequences (see header comment) ---

// cmd9 info-package (0x40): opcode,reserved,reserved,length,checksumLo,checksumHi,status, then payload D=[165,90,9,0,3,195,225]
const CMD9_INFO_OUT = padTo64([0x40, 0, 0, 7, 0xf6, 0x02, 0x00, 165, 90, 9, 0, 3, 195, 225]);
const CMD9_INFO_ACK = padTo64([0x40, 0, 0, 7, 0xf6, 0x02, 0x55, 165, 90, 9, 0, 3, 195, 225]);

// finish (0x42): identical for both command groups, confirmed across every capture this session
const FINISH_OUT = padTo64([0x42, 0, 0, 0x38, 0x7a, 0x00, 0x00]);
const FINISH_ACK = padTo64([0x42, 0, 0, 0x38, 0x7a, 0x00, 0x55]);

// cmd10 info-package (0x40): payload T=[165,90,10,0,4,1,80]
const CMD10_INFO_OUT = padTo64([0x40, 0, 0, 7, 0xa5, 0x01, 0x00, 165, 90, 10, 0, 4, 1, 80]);
const CMD10_INFO_ACK = padTo64([0x40, 0, 0, 7, 0xa5, 0x01, 0x55, 165, 90, 10, 0, 4, 1, 80]);

// cmd10 data-packet (0x41): payload = [year2digit, weekday, month, date] = [26,1,8,10], unchanged across all 3 captures
const CMD10_DATA_OUT = padTo64([0x41, 0, 0, 4, 0x72, 0x00, 0x00, 26, 1, 8, 10]);
const CMD10_DATA_ACK = padTo64([0x41, 0, 0, 4, 0x72, 0x00, 0x55, 26, 1, 8, 10]);

// cmd9 data-packet (0x41): payload = [hour, minute, second] -- the one report that varies per capture.
// Checksum bytes below are the REAL value observed on the wire (device echoed them back in its ACK).
function cmd9Data(checksum, hour, minute, second, withAck) {
  const out = padTo64([0x41, 0, 0, 3, checksum, 0x00, 0x00, hour, minute, second]);
  const ack = withAck ? padTo64([0x41, 0, 0, 3, checksum, 0x00, 0x55, hour, minute, second]) : null;
  return { out, ack };
}

function buildTransaction({ transactionId, clickEpochMs, hour, minute, second, cmd9Checksum, includeAcks }) {
  const { out: cmd9DataOut, ack: cmd9DataAck } = cmd9Data(cmd9Checksum, hour, minute, second, includeAcks);

  const reports = [];
  function push(commandIndex, commandName, opcode, direction, bytes, decodedPayload) {
    const entry = {
      transaction_id: transactionId,
      command_index: commandIndex,
      command_name: commandName,
      fragment_index: 0,
      opcode_hex: '0x' + opcode.toString(16).padStart(2, '0'),
      direction,
      hid_method: direction === 'in' ? 'input-report' : 'output-report',
      report_id: 0,
      payload_hex: toHexBytes(bytes),
    };
    if (decodedPayload) entry.decoded_payload = decodedPayload;
    reports.push(entry);
  }

  push(0, 'cmd9-time-infoPackage', 0x40, 'out', CMD9_INFO_OUT);
  if (includeAcks) push(0, 'cmd9-time-infoPackage-ACK', 0x40, 'in', CMD9_INFO_ACK);
  push(0, 'cmd9-time-dataPacket', 0x41, 'out', cmd9DataOut, { hour, minute, second });
  if (includeAcks) push(0, 'cmd9-time-dataPacket-ACK', 0x41, 'in', cmd9DataAck, { hour, minute, second });
  push(0, 'cmd9-finish', 0x42, 'out', FINISH_OUT);
  if (includeAcks) push(0, 'cmd9-finish-ACK', 0x42, 'in', FINISH_ACK);

  push(1, 'cmd10-date-infoPackage', 0x40, 'out', CMD10_INFO_OUT);
  if (includeAcks) push(1, 'cmd10-date-infoPackage-ACK', 0x40, 'in', CMD10_INFO_ACK);
  push(1, 'cmd10-date-dataPacket', 0x41, 'out', CMD10_DATA_OUT, { year2digit: 26, weekday: 1, month: 8, date: 10 });
  if (includeAcks) push(1, 'cmd10-date-dataPacket-ACK', 0x41, 'in', CMD10_DATA_ACK, { year2digit: 26, weekday: 1, month: 8, date: 10 });
  push(1, 'cmd10-finish', 0x42, 'out', FINISH_OUT);
  if (includeAcks) push(1, 'cmd10-finish-ACK', 0x42, 'in', FINISH_ACK);

  return {
    transaction_id: transactionId,
    click_timestamp_epoch_ms: clickEpochMs,
    click_timestamp_iso: new Date(clickEpochMs).toISOString(),
    tz_offset_minutes: -120,
    tz_offset_convention: 'JS Date.getTimezoneOffset() sense: (UTC - local) in minutes, so -120 means local = UTC+2',
    connection_mode: 'usb-cable',
    interface_identity: {
      vendor_id_hex: '0x28E9',
      product_id_hex: '0x31C8',
      product_name: 'YUNZII B75 PRO MAX Keyboard',
      webhid_device_index: 2,
      usage_page_hex: '0xFF60',
      usage_hex: '0x61',
      serial: null,
      note: 'webhid_device_index is machine/session-specific noise, NOT a stable identifier -- re-enumerate by usage_page_hex+usage_hex at runtime. Linux-side sysfs report-descriptor bytes/hash and USB topology were NOT captured this phase -- not reachable from browser JS; deferred to the Milestone 1 implementation phase where sysfs is actually reachable.'
    },
    browser: 'Chrome (claude-in-chrome automation)',
    site_asset_version: 'index-8Bj3uPPc.js',
    time_source: 'real wall clock at click time (NOT an injected/overridden Date). See PROTOCOL.md / PR #1 disposition comment for why this phase relies on wall-clock diffing + vendor-source cross-validation instead of a synthetic Date-override rollover matrix.',
    reports,
  };
}

const cap1 = buildTransaction({ transactionId: 'cap1', clickEpochMs: 1786382653673, hour: 19, minute: 24, second: 13, cmd9Checksum: 0x7c, includeAcks: true });

const cap2 = buildTransaction({ transactionId: 'cap2', clickEpochMs: 1786382894721, hour: 19, minute: 28, second: 14, cmd9Checksum: 0x81, includeAcks: false });
cap2.note = 'Only the out-report bytes for the varying cmd9-dataPacket were independently re-confirmed for this capture; ACKs were not re-fetched before the log was cleared for cap3. All other reports (info-packages, finish, cmd10-data) reuse cap1\'s fully ACK-verified bytes, independently confirmed byte-identical across every capture in this session.';

const cap3 = buildTransaction({ transactionId: 'cap3', clickEpochMs: 1786382995802, hour: 19, minute: 29, second: 55, cmd9Checksum: 0xab, includeAcks: true });
cap3.total_raw_hid_events = 54;
cap3.repeat_structure = {
  repeat_count: 3,
  note: 'Confirmed both by full raw log analysis (18 out reports = 3 * (3+3)) and by the vendor source: a literal `for (i=0; i<3; i++)` loop wrapping both command groups. Every out report gets exactly 2 identical inputreport ACKs in the full raw log (36 in-entries): 18 out + 36 in = 54, matching total_raw_hid_events. This fixture stores one representative instance of each distinct report (repeats are byte-identical, confirmed), not all 54 raw events.'
};

for (const [name, obj] of [['cap1', cap1], ['cap2', cap2], ['cap3', cap3]]) {
  fs.writeFileSync(path.join(__dirname, '..', 'fixtures', `${name}.json`), JSON.stringify(obj, null, 2) + '\n');
  console.log(`wrote fixtures/${name}.json`);
}

// --- Milestone 2: page-switch (cmd11/13/15) and clear-picture (cmd14) ---
//
// Same discipline as above: short real payload arrays + observed checksum
// bytes, captured live this session via scripts/capture-hook.js while
// clicking each button on the real keyboard (see the Milestone 2 plan's
// "Raw evidence & anti-circularity" section for the full methodology note,
// including the 4th recurrence of the hand-typed-hex transcription bug this
// session -- caught in a throwaway manual cross-check, not in this data).
// ACK bytes are NOT independently re-observed per command here -- they are
// constructed from the OUT bytes with only byte[6] flipped 0x00->0x55,
// which Milestone 1 established and confirmed against real hardware as the
// device's general ACK shape (src/device.rs `is_valid_ack`). This is stated
// explicitly rather than presented as a fresh independent observation.

function withAckFlip(outBytes) {
  const ack = outBytes.slice();
  ack[6] = 0x55;
  return ack;
}

// [payload, observed checksum16le as [lo, hi]] -- checksum bytes are the
// REAL values read from the live capture, independently recomputed by
// check-coverage.js and verify-checksums.js against the outer-checksum
// formula as the actual independence check.
const PAGE_SWITCH_COMMANDS = [
  { cmd: 11, name: 'switch-homepage', payload: [165, 90, 11, 0, 0, 2, 0], checksum: [0x53, 0x01] },
  { cmd: 13, name: 'switch-picture-page', payload: [165, 90, 13, 0, 0, 3, 224], checksum: [0x36, 0x02] },
  { cmd: 15, name: 'switch-gif-page', payload: [165, 90, 15, 0, 0, 195, 65], checksum: [0x59, 0x02] },
];
const CLEAR_PICTURE = { cmd: 14, name: 'clear-picture', payload: [165, 90, 14, 0, 0, 3, 16], checksum: [0x67, 0x01] };

function infoPackageOut(payload, checksumLoHi) {
  return padTo64([0x40, 0, 0, payload.length, checksumLoHi[0], checksumLoHi[1], 0x00, ...payload]);
}

function buildPageSwitchFixture() {
  const reports = [];
  function push(commandIndex, commandName, opcode, direction, bytes) {
    reports.push({
      transaction_id: 'page-switch',
      command_index: commandIndex,
      command_name: commandName,
      fragment_index: 0,
      opcode_hex: '0x' + opcode.toString(16).padStart(2, '0'),
      direction,
      hid_method: direction === 'in' ? 'input-report' : 'output-report',
      report_id: 0,
      payload_hex: toHexBytes(bytes),
    });
  }
  PAGE_SWITCH_COMMANDS.forEach(({ cmd, name, payload, checksum }, i) => {
    const infoOut = infoPackageOut(payload, checksum);
    push(i, `cmd${cmd}-${name}-infoPackage`, 0x40, 'out', infoOut);
    push(i, `cmd${cmd}-${name}-infoPackage-ACK`, 0x40, 'in', withAckFlip(infoOut));
    push(i, `cmd${cmd}-${name}-finish`, 0x42, 'out', FINISH_OUT);
    push(i, `cmd${cmd}-${name}-finish-ACK`, 0x42, 'in', FINISH_ACK);
  });
  return {
    transaction_id: 'page-switch',
    connection_mode: 'usb-cable',
    interface_identity: cap1.interface_identity,
    browser: 'Chrome (claude-in-chrome automation)',
    note: 'All 3 "Equipment setup" page-switch buttons (homepage/picture/gif), captured live this session (2026-08-11) via scripts/capture-hook.js. Each is a 2-report sequence: info-package(0x40) + finish(0x42), no data-packet -- unlike set-time, none of these commands send a follow-up 0x41 report.',
    reports,
  };
}

// Round-1 cross-review Blocker (codex + antigravity, PR #3): storing only
// one representative info+finish pair for clear-picture let bin/ci pass
// without proving the 16x repeat count or exact report order against real
// evidence -- the "16x" claim was prose (repeat_structure notes) only, not
// something check-raw-consistency.js actually verified byte-for-byte. Fixed
// by storing ALL 16 repeats (all 32 out reports + their 32 ACKs = 64
// entries) here, not a sample -- every repeat is independently confirmed
// byte-identical to the others (the command carries no per-iteration
// state), but the point is that the evidence file and the consistency
// check now assert that directly, not just claim it in a comment.
function buildClearPictureFixture() {
  const { cmd, name, payload, checksum } = CLEAR_PICTURE;
  const infoOut = infoPackageOut(payload, checksum);
  const infoAck = withAckFlip(infoOut);
  const reports = [];
  function push(commandIndex, commandName, opcode, direction, bytes) {
    reports.push({
      transaction_id: 'clear-picture',
      command_index: commandIndex,
      command_name: commandName,
      fragment_index: 0,
      opcode_hex: '0x' + opcode.toString(16).padStart(2, '0'),
      direction,
      hid_method: direction === 'in' ? 'input-report' : 'output-report',
      report_id: 0,
      payload_hex: toHexBytes(bytes),
    });
  }
  for (let repeat = 0; repeat < 16; repeat++) {
    push(repeat, `cmd${cmd}-${name}-infoPackage`, 0x40, 'out', infoOut);
    push(repeat, `cmd${cmd}-${name}-infoPackage-ACK`, 0x40, 'in', infoAck);
    push(repeat, `cmd${cmd}-${name}-finish`, 0x42, 'out', FINISH_OUT);
    push(repeat, `cmd${cmd}-${name}-finish-ACK`, 0x42, 'in', FINISH_ACK);
  }
  return {
    transaction_id: 'clear-picture',
    connection_mode: 'usb-cable',
    interface_identity: cap1.interface_identity,
    browser: 'Chrome (claude-in-chrome automation)',
    note: 'The "Clear the picture" button, captured live this session (2026-08-11). ALL 16 repeats of the info+finish pair are stored here (64 reports: 16x[infoPackage,infoPackage-ACK,finish,finish-ACK]), not a single representative sample -- matching the vendor JS\'s `for(a=0;a<16;a++)` loop exactly. Every repeat is byte-identical to the others (the command carries no per-iteration state), which is now asserted by scripts/check-raw-consistency.js\'s index-based (order-sensitive) comparison against fixtures/raw/cap-clear-picture-hidlog.json, not just claimed in this note.',
    repeat_structure: {
      repeat_count: 16,
      note: 'Confirmed by full raw HID log event count for one "Clear the picture" click: 16 out info-packages + 16 out finishes (32 total out), each with a matching in-ACK (64 total events) -- matches the vendor source\'s literal for(a=0;a<16;a++) loop exactly, and is a much larger repeat count than set-time\'s 3x, so was checked by counting rather than assumed from the source alone.',
    },
    reports,
  };
}

const pageSwitch = buildPageSwitchFixture();
const clearPicture = buildClearPictureFixture();
for (const [name, obj] of [['page-switch', pageSwitch], ['clear-picture', clearPicture]]) {
  fs.writeFileSync(path.join(__dirname, '..', 'fixtures', `${name}.json`), JSON.stringify(obj, null, 2) + '\n');
  console.log(`wrote fixtures/${name}.json`);
}

// --- Milestone 3: picture upload (cmd16 start, cmd12 declare-size, bulk, finish) ---
//
// This fixture is built DIFFERENTLY from every one above, on purpose.
//
// The ones above hardcode short observed byte arrays. That works when a
// command is a handful of constant reports, but picture upload is 552
// reports and 30720 payload bytes: hardcoding them would mean pasting the
// capture in, and then check-raw-consistency.js would only be comparing a
// copy of the capture against the capture.
//
// So here the fixture is regenerated from the SOURCE IMAGE
// (fixtures/test-quadrants.png) through the documented protocol model:
// decode PNG -> RGB565 -> 56-byte chunks -> reports. The real hardware
// capture lives separately in fixtures/raw/cap-picture-upload-hidlog.json,
// written straight out of the vendor tool's own WebHID traffic and never
// read by this script. check-raw-consistency.js compares the two, which
// makes it a genuine test of the model against the hardware: any error in
// the resize rule, channel order, byte order, chunk size, offset encoding,
// length byte, or checksum shows up as a byte mismatch.

const { decodePng } = require('./png-decode');

const PANEL_W = 160;
const PANEL_H = 96;
const BULK_CHUNK = 56; // reqLen(63) - 7-byte header, per the vendor's own `i = n - 7`

// The unified outer checksum. Bytes 1-2 (the bulk offset) are part of the
// sum, and the result is 16-bit little-endian at bytes 4-5 for EVERY opcode
// -- see PROTOCOL.md's "checksum correction" section. Byte 5 was previously
// documented as a reserved zero, which held only because every command
// decoded before Milestone 3 had a total sum below 256.
function outerChecksum(opcode, b1, b2, lengthByte, payload) {
  const sum = opcode + b1 + b2 + lengthByte + payload.reduce((a, b) => a + b, 0);
  return [sum & 0xff, (sum >> 8) & 0xff];
}

function buildReport(opcode, b1, b2, lengthByte, payload) {
  const [lo, hi] = outerChecksum(opcode, b1, b2, lengthByte, payload);
  return padTo64([opcode, b1, b2, lengthByte, lo, hi, 0x00, ...payload]);
}

// Only FULLY transparent pixels become black; partial alpha keeps its full
// colour. The vendor's picture encoder reads bytes 0-2 and ignores alpha, off
// a getImageData() from a fresh (transparent, unfilled) canvas -- so nothing
// is premultiplied, and only alpha 0 reads back as (0,0,0,0). See
// protocol.rs's rgb565_encode docs; the `if (data[i+3] === 0)` pre-pass in
// the vendor bundle is in its GIF path, not this one.
function rgb565Encode(rgba) {
  const out = [];
  for (let i = 0; i < rgba.length; i += 4) {
    let r = rgba[i], g = rgba[i + 1], b = rgba[i + 2];
    if (rgba[i + 3] === 0) { r = 0; g = 0; b = 0; }
    const v = ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3);
    out.push((v >> 8) & 0xff, v & 0xff); // big-endian per pixel
  }
  return out;
}

function buildPictureUploadFixture() {
  const imagePath = path.join(__dirname, '..', 'fixtures', 'test-quadrants.png');
  const { width, height, rgba } = decodePng(imagePath);
  if (width !== PANEL_W || height !== PANEL_H) {
    throw new Error(`test-quadrants.png must be ${PANEL_W}x${PANEL_H} (it is the panel size, so no resize step is involved), got ${width}x${height}`);
  }

  const pixels = rgb565Encode(rgba);
  if (pixels.length !== PANEL_W * PANEL_H * 2) {
    throw new Error(`expected ${PANEL_W * PANEL_H * 2} encoded bytes, got ${pixels.length}`);
  }

  const reports = [];
  function push(commandIndex, commandName, opcode, bytes, extra) {
    const entry = {
      transaction_id: 'picture-upload',
      command_index: commandIndex,
      command_name: commandName,
      fragment_index: 0,
      opcode_hex: '0x' + opcode.toString(16).padStart(2, '0'),
      direction: 'out',
      hid_method: 'output-report',
      report_id: 0,
      payload_hex: toHexBytes(bytes),
    };
    if (extra) Object.assign(entry, extra);
    reports.push(entry);
  }

  // 1. start: info-package (0x40), inner cmd 16. Eight payload bytes, not
  //    the seven every earlier command used.
  push(0, 'cmd16-picture-start-infoPackage', 0x40, buildReport(0x40, 0, 0, 8, [165, 90, 16, 0, 1, 197, 177, 1]));

  // 2. declare size: data-packet (0x41), inner cmd 12. `120,0` is 30720 as
  //    [hi,lo]; `195,147` is the vendor's inner CRC ga([12,120,0]).
  push(1, 'cmd12-picture-declareSize-dataPacket', 0x41, buildReport(0x41, 0, 0, 7, [165, 90, 12, 120, 0, 195, 147]));

  // 3. bulk pixel data. The length byte is the real remaining-byte count,
  //    so the final chunk declares 0x20 (32), NOT a zero-padded 0x38.
  for (let offset = 0; offset < pixels.length; offset += BULK_CHUNK) {
    const chunk = pixels.slice(offset, offset + BULK_CHUNK);
    const bytes = buildReport(0x41, offset & 0xff, (offset >> 8) & 0xff, chunk.length, chunk);
    push(2, 'picture-bulk-' + String(offset / BULK_CHUNK).padStart(3, '0'), 0x41, bytes, {
      offset,
      data_length: chunk.length,
    });
  }

  // 4. finish (0x42), the same constant report every other command ends with.
  push(3, 'picture-finish', 0x42, FINISH_OUT);

  return {
    transaction_id: 'picture-upload',
    connection_mode: 'usb-cable',
    interface_identity: cap1.interface_identity,
    browser: 'Chrome (claude-in-chrome automation)',
    source_image: 'fixtures/test-quadrants.png',
    note: 'The "Save to the device" button on the Picture Settings tab, for a 160x96 four-quadrant + gradient test image, captured live 2026-08-11. 552 out reports: 1 start + 1 declare-size + 549 bulk + 1 finish. Unlike the other fixtures in this directory, these bytes are REGENERATED from fixtures/test-quadrants.png through the protocol model rather than hardcoded from the capture -- the capture itself is in fixtures/raw/cap-picture-upload-hidlog.json and scripts/check-raw-consistency.js compares the two, so that comparison actually tests the model against real hardware traffic. It passes with 0 mismatches across all 552 reports / 30720 pixel bytes.',
    structure: {
      total_reports: 552,
      bulk_reports: 549,
      bulk_chunk_bytes: BULK_CHUNK,
      pixel_bytes: PANEL_W * PANEL_H * 2,
      note: 'ACKs are not stored: they were not recorded in this capture, and inventing them by flipping byte 6 (as the earlier fixtures do) would add derived data to a file whose value is that every byte in it was observed.',
    },
    reports,
  };
}

const pictureUpload = buildPictureUploadFixture();
fs.writeFileSync(path.join(__dirname, '..', 'fixtures', 'picture-upload.json'), JSON.stringify(pictureUpload, null, 2) + '\n');
console.log(`wrote fixtures/picture-upload.json (${pictureUpload.reports.length} reports)`);

// --- Milestone 4: GIF upload (cmd18/19 session, cmd16 frame, cmd17, bulk) ---
//
// HYBRID fixture, and labelled as such wherever it is described.
//
// The control reports, the 1024-byte block chunking, the per-block offsets,
// the length bytes and every checksum are REGENERATED here by the model. The
// pixel payload bytes are COPIED from the capture, because the vendor runs
// each GIF frame through a three-stage browser-canvas downscale with smoothing
// plus an edge filter, and browser resampling is not reproducible outside a
// browser. Milestone 3's picture fixture is model-derived end to end; this one
// is not, and the two must not be described as equivalent.
//
// What it still catches, which is the class of bug that breaks the device: any
// error in framing, ordering, block boundaries, offset restart, length bytes,
// or checksums.

const GIF_BLOCK = 1024;
const GIF_MODE = 1; // "save to the device" -- the mode that actually plays

function gifSessionPayload(cmd, crc, mode, last) {
  return [165, 90, cmd, 0, 2, crc[0], crc[1], mode, last];
}

function buildGifUploadFixture() {
  const rawPath = path.join(__dirname, '..', 'fixtures', 'raw', 'cap-gif-upload-hidlog.json');
  const raw = JSON.parse(fs.readFileSync(rawPath, 'utf8'));
  const capture = raw.entries.map((e) => e.hex.trim().split(/\s+/).map((b) => parseInt(b, 16)));

  // Recover each frame's pixel stream from the capture's bulk packets. This is
  // the part we copy rather than model.
  const framePixels = [];
  let current = [];
  for (const r of capture) {
    const isControl = r[7] === 165 && r[8] === 90;
    if (r[0] === 0x41 && !isControl) {
      current.push(...r.slice(7, 7 + r[3]));
      if (current.length === PANEL_W * PANEL_H * 2) {
        framePixels.push(current);
        current = [];
      }
    }
  }
  if (current.length !== 0) throw new Error(`trailing ${current.length} pixel bytes: frame boundaries are wrong`);
  if (framePixels.length === 0) throw new Error('no frames recovered from the capture');

  const reports = [];
  function push(commandIndex, commandName, opcode, bytes, extra) {
    const entry = {
      transaction_id: 'gif-upload',
      command_index: commandIndex,
      command_name: commandName,
      fragment_index: 0,
      opcode_hex: '0x' + opcode.toString(16).padStart(2, '0'),
      direction: 'out',
      hid_method: 'output-report',
      report_id: 0,
      payload_hex: toHexBytes(bytes),
    };
    if (extra) Object.assign(entry, extra);
    reports.push(entry);
  }

  // Session open. Note the opcode asymmetry, which is the vendor's: cmd18 goes
  // out as an info package (0x40), cmd19 and everything after as data packets.
  push(0, 'cmd18-gif-session-open-infoPackage', 0x40,
    buildReport(0x40, 0, 0, 9, gifSessionPayload(18, [4, 80], GIF_MODE, 0)));
  push(0, 'cmd19-gif-session-open-dataPacket', 0x41,
    buildReport(0x41, 0, 0, 9, gifSessionPayload(19, [196, 1], GIF_MODE, 0)));

  framePixels.forEach((pixels, frameIndex) => {
    push(1 + frameIndex, 'cmd16-gif-frame-header', 0x41,
      buildReport(0x41, 0, 0, 10, [165, 90, 16, 0, 3, 4, 48, 2, GIF_MODE, frameIndex]),
      { frame_index: frameIndex });
    push(1 + frameIndex, 'cmd17-gif-declareSize', 0x41,
      buildReport(0x41, 0, 0, 7, [165, 90, 17, 120, 0, 197, 3]));

    // The offset RESTARTS at 0 for every 1024-byte block -- unlike picture
    // upload, whose offsets run 0..30688 across the whole frame.
    for (let blockStart = 0; blockStart < pixels.length; blockStart += GIF_BLOCK) {
      const block = pixels.slice(blockStart, blockStart + GIF_BLOCK);
      for (let off = 0; off < block.length; off += BULK_CHUNK) {
        const chunk = block.slice(off, off + BULK_CHUNK);
        push(1 + frameIndex, 'gif-bulk', 0x41,
          buildReport(0x41, off & 0xff, (off >> 8) & 0xff, chunk.length, chunk),
          { block_offset: off, data_length: chunk.length });
      }
    }
  });

  push(90, 'cmd18-gif-session-close-frameCount', 0x41,
    buildReport(0x41, 0, 0, 9, gifSessionPayload(18, [4, 80], GIF_MODE, framePixels.length)));
  push(91, 'cmd19-gif-session-close-fps', 0x41,
    buildReport(0x41, 0, 0, 9, gifSessionPayload(19, [196, 1], GIF_MODE, raw.fps)));
  push(92, 'gif-finish', 0x42, FINISH_OUT);

  return {
    transaction_id: 'gif-upload',
    connection_mode: 'usb-cable',
    interface_identity: cap1.interface_identity,
    browser: 'Chrome (claude-in-chrome automation)',
    source_gif: raw.source_gif,
    mode: GIF_MODE,
    fps: raw.fps,
    note: 'HYBRID fixture. Control reports, block chunking, per-block offsets, length bytes and all checksums are regenerated by the model in scripts/build-fixtures.js. Pixel payload bytes are COPIED from fixtures/raw/cap-gif-upload-hidlog.json, because the vendor resamples each GIF frame through a browser canvas and that is not reproducible outside a browser. This is deliberately weaker than fixtures/picture-upload.json, which is model-derived end to end -- do not describe them as equivalent. It still pins framing, ordering, block boundaries, offset restart, lengths and checksums against real hardware traffic.',
    structure: {
      total_reports: reports.length,
      frames: framePixels.length,
      block_bytes: GIF_BLOCK,
      packets_per_block: Math.ceil(GIF_BLOCK / BULK_CHUNK),
      blocks_per_frame: Math.ceil(PANEL_W * PANEL_H * 2 / GIF_BLOCK),
      note: 'Offsets restart at 0 for EVERY 1024-byte block (1024 = 18*56 + 16, so 19 packets per block, the last declaring 16). Picture upload instead runs one continuous 0..30688 offset across the whole frame. Do not unify the two chunkers.',
    },
    reports,
  };
}

const gifUpload = buildGifUploadFixture();
fs.writeFileSync(path.join(__dirname, '..', 'fixtures', 'gif-upload.json'), JSON.stringify(gifUpload, null, 2) + '\n');
console.log(`wrote fixtures/gif-upload.json (${gifUpload.reports.length} reports)`);

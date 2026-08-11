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

function buildClearPictureFixture() {
  const { cmd, name, payload, checksum } = CLEAR_PICTURE;
  const infoOut = infoPackageOut(payload, checksum);
  const reports = [];
  function push(commandName, opcode, direction, bytes) {
    reports.push({
      transaction_id: 'clear-picture',
      command_index: 0,
      command_name: commandName,
      fragment_index: 0,
      opcode_hex: '0x' + opcode.toString(16).padStart(2, '0'),
      direction,
      hid_method: direction === 'in' ? 'input-report' : 'output-report',
      report_id: 0,
      payload_hex: toHexBytes(bytes),
    });
  }
  push(`cmd${cmd}-${name}-infoPackage`, 0x40, 'out', infoOut);
  push(`cmd${cmd}-${name}-infoPackage-ACK`, 0x40, 'in', withAckFlip(infoOut));
  push(`cmd${cmd}-${name}-finish`, 0x42, 'out', FINISH_OUT);
  push(`cmd${cmd}-${name}-finish-ACK`, 0x42, 'in', FINISH_ACK);
  return {
    transaction_id: 'clear-picture',
    connection_mode: 'usb-cable',
    interface_identity: cap1.interface_identity,
    browser: 'Chrome (claude-in-chrome automation)',
    note: 'The "Clear the picture" button, captured live this session (2026-08-11). One representative info+finish pair is stored here; the full raw HID log (fixtures/raw/cap-clear-picture-hidlog.json) confirms the vendor JS\'s `for(a=0;a<16;a++)` loop is real -- exactly 16 repeats of this same info+finish pair (32 reports total), byte-identical every time (the command carries no per-iteration state). This mirrors cap3\'s repeat_structure pattern for set-time\'s 3x loop.',
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

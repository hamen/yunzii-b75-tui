// Generates fixtures/*.json from structured capture data, guaranteeing every
// report is exactly 64 bytes and every checksum is computed (never
// hand-transcribed) -- run with `node scripts/build-fixtures.js` from repo
// root whenever a new capture is added. This replaces hand-typed hex, which
// previously had a transcription bug (every 0x40/0x41 OUT report was one
// padding byte short -- caught in PR #1 cross-review).

const fs = require('fs');
const path = require('path');

function toHexBytes(bytes) {
  return bytes.map((b) => b.toString(16).padStart(2, '0')).join(' ');
}

// Builds one 64-byte report. status=0 for outbound, 0x55 for the device ACK.
function buildReport(opcode, length, payload, status) {
  const bytes = new Array(64).fill(0);
  bytes[0] = opcode;
  bytes[3] = length;
  const sum = opcode + length + payload.reduce((a, b) => a + b, 0);
  if (opcode === 0x40) {
    bytes[4] = sum & 0xff;
    bytes[5] = (sum >> 8) & 0xff;
    bytes[6] = status;
  } else {
    bytes[4] = sum & 0xff;
    bytes[5] = 0;
    bytes[6] = status;
  }
  for (let i = 0; i < payload.length; i++) bytes[7 + i] = payload[i];
  return bytes;
}

const D = [165, 90, 9, 0, 3, 195, 225];
const T = [165, 90, 10, 0, 4, 1, 80];

function buildTransaction({ transactionId, clickEpochMs, tzOffsetMinutes, hour, minute, second, year2, weekday, month, date, includeAcks, includeCmd10Finish, repeatCounts }) {
  const P = [hour, minute, second];
  const M = [year2, weekday, month, date];

  const reports = [];

  function push(commandIndex, commandName, opcode, length, payload, direction, observedRepeats, decodedPayload) {
    const status = direction === 'in' ? 0x55 : 0x00;
    const bytes = buildReport(opcode, length, payload, status);
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
    if (observedRepeats !== undefined) entry.observed_repeats = observedRepeats;
    if (decodedPayload !== undefined) entry.decoded_payload = decodedPayload;
    reports.push(entry);
  }

  push(0, 'cmd9-time-infoPackage', 0x40, D.length, D, 'out', repeatCounts.cmd9infoOut);
  if (includeAcks) push(0, 'cmd9-time-infoPackage-ACK', 0x40, D.length, D, 'in', repeatCounts.cmd9infoIn);
  push(0, 'cmd9-time-dataPacket', 0x41, P.length, P, 'out', repeatCounts.cmd9dataOut, { hour, minute, second });
  push(0, 'cmd9-finish', 0x42, 0x38, [], 'out', repeatCounts.cmd9finishOut);

  push(1, 'cmd10-date-infoPackage', 0x40, T.length, T, 'out', repeatCounts.cmd10infoOut);
  push(1, 'cmd10-date-dataPacket', 0x41, M.length, M, 'out', repeatCounts.cmd10dataOut, { year2digit: year2, weekday, month, date });
  if (includeCmd10Finish) push(1, 'cmd10-finish', 0x42, 0x38, [], 'out', repeatCounts.cmd10finishOut);

  return {
    transaction_id: transactionId,
    click_timestamp_epoch_ms: clickEpochMs,
    click_timestamp_iso: new Date(clickEpochMs).toISOString(),
    tz_offset_minutes: tzOffsetMinutes,
    connection_mode: 'usb-cable',
    interface_identity: {
      vendor_id_hex: '0x28E9',
      product_id_hex: '0x31C8',
      product_name: 'YUNZII B75 PRO MAX Keyboard',
      webhid_device_index: 2,
      usage_page_hex: '0xFF60',
      usage_hex: '0x61',
      serial: null,
      note: 'Linux-side sysfs report-descriptor bytes/hash and USB topology (bInterfaceNumber, bus/port path) were NOT captured this phase -- not reachable from browser JS. VID+PID+usage-page+usage is the same 4-tuple WebHID itself uses to disambiguate interfaces, which is sufficient to reliably re-open this exact interface; deeper sysfs correlation is deferred to the Rust implementation phase where it is actually actionable on Linux.'
    },
    browser: 'Chrome (claude-in-chrome automation)',
    site_asset_version: 'index-8Bj3uPPc.js',
    time_source: 'real wall clock at click time (NOT an injected/overridden Date) -- see PROTOCOL.md for why this phase relies on wall-clock diffing + vendor-source cross-validation instead of a synthetic Date-override capture matrix',
    reports,
  };
}

const cap1 = buildTransaction({
  transactionId: 'cap1', clickEpochMs: 1786382653673, tzOffsetMinutes: -120,
  hour: 19, minute: 24, second: 13, year2: 26, weekday: 1, month: 8, date: 10,
  includeAcks: true, includeCmd10Finish: true,
  repeatCounts: { cmd9infoOut: 3, cmd9infoIn: 6, cmd9dataOut: 3, cmd9finishOut: 3, cmd10infoOut: 3, cmd10dataOut: 3, cmd10finishOut: 3 },
});

const cap2 = buildTransaction({
  transactionId: 'cap2', clickEpochMs: 1786382894721, tzOffsetMinutes: -120,
  hour: 19, minute: 28, second: 14, year2: 26, weekday: 1, month: 8, date: 10,
  includeAcks: false, includeCmd10Finish: true,
  repeatCounts: { cmd9infoOut: undefined, cmd9dataOut: undefined, cmd9finishOut: undefined, cmd10infoOut: undefined, cmd10dataOut: undefined, cmd10finishOut: undefined },
});
cap2.note = 'Only unique out-report bytes were retrieved before the log was cleared for cap3 -- ACK/duplicate/repeat-count detail for this one is not stored (see cap1 and cap3 for that structure, independently confirmed there and in the vendor source).';

const cap3 = buildTransaction({
  transactionId: 'cap3', clickEpochMs: 1786382995802, tzOffsetMinutes: -120,
  hour: 19, minute: 29, second: 55, year2: 26, weekday: 1, month: 8, date: 10,
  includeAcks: true, includeCmd10Finish: true,
  repeatCounts: { cmd9infoOut: 3, cmd9infoIn: 6, cmd9dataOut: 3, cmd9finishOut: 3, cmd10infoOut: 3, cmd10dataOut: 3, cmd10finishOut: 3 },
});
cap3.total_raw_hid_events = 54;
cap3.repeat_structure = {
  repeat_count: 3,
  note: 'Confirmed both by full raw log analysis (18 out reports = 3 * (3+3)) and by the vendor source: a literal `for (i=0; i<3; i++)` loop wrapping both command groups. Every out report gets exactly 2 identical inputreport ACKs (36 in-entries total): 18 out + 36 in = 54, matching total_raw_hid_events. The cmd9-finish and cmd10-finish reports are byte-identical (same opcode 0x42, same constant length 0x38, same checksum) -- each is sent once per repeat of its own group (3 times), for 6 identical 0x42 sends total across both groups.'
};

for (const [name, obj] of [['cap1', cap1], ['cap2', cap2], ['cap3', cap3]]) {
  fs.writeFileSync(path.join(__dirname, '..', 'fixtures', `${name}.json`), JSON.stringify(obj, null, 2) + '\n');
  console.log(`wrote fixtures/${name}.json`);
}

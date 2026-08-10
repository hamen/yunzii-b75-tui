// bin/ci's coverage/decode check. Validates, for every fixture report:
//  1. fields.json's reportShapes cover byte offsets 0-63 with no gaps (once, globally)
//  2. required fixture keys are present
//  3. payload_hex is exactly 64 bytes
//  4. the wire checksum matches the formula fields.json declares for its opcode
//  5. constant fields (opcode, reserved bytes, info-package literals) match their declared values
//  6. decoded_payload (when present) matches the actual payload bytes at their documented offsets
// Run with `node scripts/check-coverage.js` from the repo root.

const fs = require('fs');
const path = require('path');

const fields = JSON.parse(fs.readFileSync(path.join(__dirname, '..', 'fields.json'), 'utf8'));
const fixturesDir = path.join(__dirname, '..', 'fixtures');
const fixtureFiles = fs.readdirSync(fixturesDir).filter((f) => f.endsWith('.json'));

const REQUIRED_KEYS = ['transaction_id', 'command_index', 'fragment_index', 'direction', 'hid_method', 'report_id', 'payload_hex'];

let failures = 0;
function fail(msg) { console.error(`FAIL: ${msg}`); failures++; }
function ok(msg) { console.log(`OK:   ${msg}`); }
function parseHex(hex) { return hex.trim().split(/\s+/).map((b) => parseInt(b, 16)); }

// --- 1. every reportShape's fields cover offsets 0-63 exactly once ---
for (const [opcodeHex, shape] of Object.entries(fields.reportShapes)) {
  const covered = new Array(shape.totalLength).fill(false);
  for (const f of shape.fields) {
    const [start, end] = f.offset;
    for (let i = start; i <= end; i++) {
      if (covered[i]) fail(`${opcodeHex}: offset ${i} covered by more than one field (last: ${f.name})`);
      covered[i] = true;
    }
  }
  const gaps = covered.map((v, i) => (v ? null : i)).filter((v) => v !== null);
  if (gaps.length) fail(`${opcodeHex}: uncovered byte offsets: ${gaps.join(',')}`);
  else ok(`${opcodeHex}: full 0-${shape.totalLength - 1} coverage, no gaps/overlaps`);
}

// --- 2-6. per-fixture-report checks ---
for (const file of fixtureFiles) {
  const data = JSON.parse(fs.readFileSync(path.join(fixturesDir, file), 'utf8'));
  for (const report of data.reports) {
    const label = `${file}/${report.command_name}`;

    // 2. required keys present
    const missing = REQUIRED_KEYS.filter((k) => !(k in report));
    if (missing.length) { fail(`${label}: missing required key(s): ${missing.join(', ')}`); continue; }

    const bytes = parseHex(report.payload_hex);

    // 3. exactly 64 bytes
    if (bytes.length !== 64) { fail(`${label}: payload_hex is ${bytes.length} bytes, expected 64`); continue; }
    ok(`${label}: 64 bytes`);

    const opcode = bytes[0];
    const length = bytes[3];
    const opcodeHex = '0x' + opcode.toString(16).padStart(2, '0');
    const shape = fields.reportShapes[opcodeHex];
    if (!shape) { fail(`${label}: unknown opcode ${opcodeHex}`); continue; }

    const payload = bytes.slice(7, 7 + length);
    const sum = opcode + length + payload.reduce((a, b) => a + b, 0);

    // 4. checksum
    if (opcodeHex === '0x40') {
      const expected = [sum & 0xff, (sum >> 8) & 0xff];
      const actual = [bytes[4], bytes[5]];
      if (JSON.stringify(expected) !== JSON.stringify(actual)) fail(`${label}: checksum16 mismatch, computed ${expected}, wire ${actual}`);
      else ok(`${label}: checksum16 matches (${actual})`);
    } else {
      const expected = sum & 0xff;
      const actual = bytes[4];
      if (expected !== actual) fail(`${label}: checksum8 mismatch, computed 0x${expected.toString(16)}, wire 0x${actual.toString(16)}`);
      else ok(`${label}: checksum8 matches (0x${actual.toString(16)})`);
    }

    // 5. reserved bytes actually zero, status byte is 0x00 or 0x55, padding beyond length is zero
    if (bytes[1] !== 0 || bytes[2] !== 0) fail(`${label}: reserved offsets 1-2 not zero`);
    // 0x41/0x42 have a single reserved byte at offset 5 (0x40's offset 5 is the checksum's high byte, not reserved).
    if (opcodeHex !== '0x40' && bytes[5] !== 0) fail(`${label}: reserved offset 5 not zero (0x${bytes[5].toString(16)})`);
    if (report.direction === 'out' && bytes[6] !== 0x00) fail(`${label}: outbound status byte should be 0x00, got 0x${bytes[6].toString(16)}`);
    if (report.direction === 'in' && bytes[6] !== 0x55) fail(`${label}: inbound ACK status byte should be 0x55, got 0x${bytes[6].toString(16)}`);
    for (let i = 7 + length; i < 64; i++) {
      if (bytes[i] !== 0) { fail(`${label}: padding byte at offset ${i} is not zero`); break; }
    }

    // 5a. finish (0x42) length byte is always the constant 0x38
    if (opcodeHex === '0x42' && length !== 0x38) fail(`${label}: finish report length should be constant 0x38, got 0x${length.toString(16)}`);

    // 5b. info-package (0x40) constant literals for known commands
    if (opcodeHex === '0x40') {
      const cmdByte = payload[2];
      const known = cmdByte === 9 ? fields.commands.cmd9_setClock.infoPackagePayload.value
                  : cmdByte === 10 ? fields.commands.cmd10_setDate.infoPackagePayload.value
                  : null;
      if (known && JSON.stringify(payload) !== JSON.stringify(known)) {
        fail(`${label}: info-package payload ${JSON.stringify(payload)} != declared constant ${JSON.stringify(known)}`);
      } else if (known) {
        ok(`${label}: info-package payload matches declared constant`);
      }
    }

    // 6. decoded_payload cross-check against the raw bytes
    if (report.decoded_payload) {
      const dp = report.decoded_payload;
      let expectedPayload = null;
      if ('hour' in dp) expectedPayload = [dp.hour, dp.minute, dp.second];
      else if ('year2digit' in dp) expectedPayload = [dp.year2digit, dp.weekday, dp.month, dp.date];
      if (expectedPayload) {
        if (JSON.stringify(payload) !== JSON.stringify(expectedPayload)) {
          fail(`${label}: decoded_payload ${JSON.stringify(expectedPayload)} != raw payload bytes ${JSON.stringify(payload)}`);
        } else {
          ok(`${label}: decoded_payload matches raw bytes (${JSON.stringify(expectedPayload)})`);
        }
      }
    }
  }
}

if (failures > 0) {
  console.error(`\n${failures} failure(s).`);
  process.exit(1);
}
console.log('\nAll coverage, schema, checksum, and decode checks passed.');

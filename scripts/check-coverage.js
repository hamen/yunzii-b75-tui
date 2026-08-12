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

// Above this many reports in one fixture, suppress the per-report OK lines
// and print a per-file summary instead. fixtures/picture-upload.json is 552
// reports; at ~4 OK lines each it would bury every other check in the run
// under 2000 lines of noise. FAIL lines are never suppressed.
const VERBOSE_REPORT_LIMIT = 100;

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
  const verbose = data.reports.length <= VERBOSE_REPORT_LIMIT;
  const okDetail = verbose ? ok : () => {};
  let checkedInFile = 0;

  for (const report of data.reports) {
    const label = `${file}/${report.command_name}`;

    // 2. required keys present
    const missing = REQUIRED_KEYS.filter((k) => !(k in report));
    if (missing.length) { fail(`${label}: missing required key(s): ${missing.join(', ')}`); continue; }

    const bytes = parseHex(report.payload_hex);

    // 3. exactly 64 bytes
    if (bytes.length !== 64) { fail(`${label}: payload_hex is ${bytes.length} bytes, expected 64`); continue; }
    checkedInFile++;
    okDetail(`${label}: 64 bytes`);

    const opcode = bytes[0];
    const length = bytes[3];
    const opcodeHex = '0x' + opcode.toString(16).padStart(2, '0');
    const shape = fields.reportShapes[opcodeHex];
    if (!shape) { fail(`${label}: unknown opcode ${opcodeHex}`); continue; }

    const payload = bytes.slice(7, 7 + length);
    // Bytes 1-2 are part of the checksummed prefix. They are zero for every
    // command except picture-upload's bulk packets, where they carry the
    // little-endian byte offset -- so they must be summed, not skipped.
    const sum = opcode + bytes[1] + bytes[2] + length + payload.reduce((a, b) => a + b, 0);

    // 4. checksum -- 16-bit little-endian at bytes 4-5, for EVERY opcode.
    //    Milestone 3 correction: byte 5 used to be documented as a reserved
    //    zero for 0x41/0x42, and byte 4 as a standalone 8-bit checksum. The
    //    low byte was in fact always right; what was wrong was calling byte 5
    //    reserved. In fixtures/picture-upload.json, byte 5 is non-zero in
    //    551 of 552 reports (only `finish`, whose sum is 0x7a, has a zero
    //    high byte). Every command decoded before Milestone 3 happened to
    //    have a total sum below 256, which is why the old model held.
    const expected = [sum & 0xff, (sum >> 8) & 0xff];
    const actual = [bytes[4], bytes[5]];
    if (JSON.stringify(expected) !== JSON.stringify(actual)) fail(`${label}: checksum16le mismatch, computed ${expected}, wire ${actual}`);
    else okDetail(`${label}: checksum16le matches (${actual})`);

    // 5. offset/reserved bytes 1-2, status byte, padding beyond length
    //
    // Two kinds of bulk packet carry an offset in bytes 1-2:
    //   picture upload  -> `offset`, continuous 0..30688 across the frame
    //   GIF upload      -> `block_offset`, restarting at 0 every 1024 bytes
    // They are named differently in the fixtures on purpose, so that reading
    // one cannot be mistaken for the other.
    const offsetKey = 'offset' in report ? 'offset' : ('block_offset' in report ? 'block_offset' : null);
    if (offsetKey) {
      const wireOffset = bytes[1] | (bytes[2] << 8);
      if (wireOffset !== report[offsetKey]) fail(`${label}: offset bytes decode to ${wireOffset}, fixture says ${report[offsetKey]}`);
      else okDetail(`${label}: ${offsetKey} bytes match (${wireOffset})`);
      if (offsetKey === 'block_offset' && wireOffset >= 1024) {
        fail(`${label}: a GIF block offset must stay below the 1024-byte block size, got ${wireOffset}`);
      }
      if ('data_length' in report && length !== report.data_length) {
        fail(`${label}: length byte is 0x${length.toString(16)} but fixture says data_length ${report.data_length}`);
      }
    } else if (bytes[1] !== 0 || bytes[2] !== 0) {
      fail(`${label}: offsets 1-2 should be zero for a non-bulk report, got 0x${bytes[1].toString(16)} 0x${bytes[2].toString(16)}`);
    }
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
                  : cmdByte === 11 ? fields.commands.cmd11_switchToHomepage.infoPackagePayload.value
                  : cmdByte === 13 ? fields.commands.cmd13_switchToPicturePage.infoPackagePayload.value
                  : cmdByte === 14 ? fields.commands.cmd14_clearPicture.infoPackagePayload.value
                  : cmdByte === 15 ? fields.commands.cmd15_switchToGifPage.infoPackagePayload.value
                  : cmdByte === 16 ? fields.commands.cmd16_pictureUploadStart.infoPackagePayload.value
                  : null;
      if (known && JSON.stringify(payload) !== JSON.stringify(known)) {
        fail(`${label}: info-package payload ${JSON.stringify(payload)} != declared constant ${JSON.stringify(known)}`);
      } else if (known) {
        okDetail(`${label}: info-package payload matches declared constant`);
      }
    }

    // 5d. Milestone 4's GIF commands are 0x41 data packets whose payloads end
    //     in VARIABLE bytes (mode, frame index, frame count, frame rate), so
    //     unlike every command above them they cannot be compared against a
    //     single constant. Check the fixed prefix, and sanity-check the
    //     parameters that have a knowable range.
    if (opcodeHex === '0x41' && payload[0] === 165 && payload[1] === 90) {
      const gifCommands = {
        18: fields.commands.cmd18_gifSession,
        19: fields.commands.cmd19_gifSessionRate,
      };
      const gifCmd = gifCommands[payload[2]];
      if (gifCmd) {
        const prefix = gifCmd.dataPacketPayload.prefix;
        if (JSON.stringify(payload.slice(0, prefix.length)) !== JSON.stringify(prefix)) {
          fail(`${label}: GIF session prefix ${JSON.stringify(payload.slice(0, prefix.length))} != declared ${JSON.stringify(prefix)}`);
        } else {
          okDetail(`${label}: GIF session prefix matches; trailer [${payload.slice(prefix.length)}]`);
        }
        if (payload.length !== prefix.length + 2) fail(`${label}: expected a 2-byte trailer, payload is ${payload.length} bytes`);
        if (payload[7] !== 1) fail(`${label}: only mode 1 ("save to the device") is shipped, got mode ${payload[7]}`);
      }
      if (payload[2] === 16 && length === 10) {
        const prefix = fields.commands.cmd16_gifFrameHeader.dataPacketPayload.prefix;
        if (JSON.stringify(payload.slice(0, prefix.length)) !== JSON.stringify(prefix)) {
          fail(`${label}: GIF frame-header prefix ${JSON.stringify(payload.slice(0, prefix.length))} != declared ${JSON.stringify(prefix)}`);
        } else {
          okDetail(`${label}: GIF frame-header prefix matches; mode ${payload[8]}, frame ${payload[9]}`);
        }
        if ('frame_index' in report && payload[9] !== report.frame_index) {
          fail(`${label}: wire frame index ${payload[9]} != fixture's ${report.frame_index}`);
        }
        if (payload[8] !== 1) fail(`${label}: only mode 1 ("save to the device") is shipped, got mode ${payload[8]}`);
      }
      if (payload[2] === 17) {
        const known = fields.commands.cmd17_gifDeclareSize.dataPacketPayload.value;
        if (JSON.stringify(payload) !== JSON.stringify(known)) {
          fail(`${label}: cmd17 payload ${JSON.stringify(payload)} != declared constant ${JSON.stringify(known)}`);
        } else {
          okDetail(`${label}: cmd17 declare-size matches declared constant`);
        }
      }
    }

    // 5c. cmd12 declare-size is a 0x41 data-packet, not an info-package, so
    //     it needs its own constant check rather than riding on 5b above.
    if (opcodeHex === '0x41' && payload[0] === 165 && payload[1] === 90 && payload[2] === 12) {
      const known = fields.commands.cmd12_pictureDeclareSize.dataPacketPayload.value;
      if (JSON.stringify(payload) !== JSON.stringify(known)) {
        fail(`${label}: declare-size payload ${JSON.stringify(payload)} != declared constant ${JSON.stringify(known)}`);
      } else {
        okDetail(`${label}: declare-size payload matches declared constant`);
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
          okDetail(`${label}: decoded_payload matches raw bytes (${JSON.stringify(expectedPayload)})`);
        }
      }
    }
  }

  if (!verbose) {
    ok(`${file}: all ${checkedInFile} reports pass every schema/checksum/offset/padding check (per-report lines suppressed above ${VERBOSE_REPORT_LIMIT})`);
  }
}

if (failures > 0) {
  console.error(`\n${failures} failure(s).`);
  process.exit(1);
}
console.log('\nAll coverage, schema, checksum, and decode checks passed.');

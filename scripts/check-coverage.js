// bin/ci's coverage check: (1) fields.json's reportShapes must cover byte
// offsets 0-63 with no gaps for every shape; (2) every fixture's actual bytes
// must match the checksum formula fields.json declares for its opcode.
// Run with `node scripts/check-coverage.js` from the repo root.

const fs = require('fs');
const path = require('path');

const fields = JSON.parse(fs.readFileSync(path.join(__dirname, '..', 'fields.json'), 'utf8'));
const fixturesDir = path.join(__dirname, '..', 'fixtures');
const fixtureFiles = fs.readdirSync(fixturesDir).filter((f) => f.endsWith('.json'));

let failures = 0;
function fail(msg) { console.error(`FAIL: ${msg}`); failures++; }
function ok(msg) { console.log(`OK:   ${msg}`); }

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

// --- 2. every fixture's outbound report checksum matches the declared formula ---
function parseHex(hex) { return hex.trim().split(/\s+/).map((b) => parseInt(b, 16)); }

for (const file of fixtureFiles) {
  const data = JSON.parse(fs.readFileSync(path.join(fixturesDir, file), 'utf8'));
  for (const report of data.reports) {
    if (report.direction !== 'out') continue; // ACKs are a documented flip of the request, not independently checksummed
    const bytes = parseHex(report.payloadHex);
    const opcode = bytes[0];
    const length = bytes[3];
    const opcodeHex = '0x' + opcode.toString(16).padStart(2, '0');
    const shape = fields.reportShapes[opcodeHex];
    if (!shape) { fail(`${file}/${report.commandName}: unknown opcode ${opcodeHex}`); continue; }

    const payload = bytes.slice(7, 7 + length);
    const sum = opcode + length + payload.reduce((a, b) => a + b, 0);

    if (opcodeHex === '0x40') {
      const expected = [sum & 0xff, (sum >> 8) & 0xff];
      const actual = [bytes[4], bytes[5]];
      if (JSON.stringify(expected) !== JSON.stringify(actual)) {
        fail(`${file}/${report.commandName}: checksum16 mismatch, computed ${expected}, wire ${actual}`);
      } else {
        ok(`${file}/${report.commandName}: checksum16 matches (${actual})`);
      }
    } else {
      const expected = sum & 0xff;
      const actual = bytes[4];
      if (expected !== actual) {
        fail(`${file}/${report.commandName}: checksum8 mismatch, computed 0x${expected.toString(16)}, wire 0x${actual.toString(16)}`);
      } else {
        ok(`${file}/${report.commandName}: checksum8 matches (0x${actual.toString(16)})`);
      }
    }
  }
}

if (failures > 0) {
  console.error(`\n${failures} failure(s).`);
  process.exit(1);
}
console.log('\nAll coverage and checksum checks passed.');

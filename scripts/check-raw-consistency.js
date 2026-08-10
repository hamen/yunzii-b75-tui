// Anti-circularity check (added after PR #1 round-2 cross-review): confirms
// that fixtures/cap1.json's reports actually match the raw, minimally
// processed capture evidence in fixtures/raw/cap1-hidlog.json, byte for
// byte. Without this, check-coverage.js only checks the polished fixtures
// against fields.json -- both derived by the same person from the same
// decode -- which can never catch a real mismatch between the model and
// what the hardware actually did. This script is the one check that
// compares against the raw evidence directly.
const fs = require('fs');
const path = require('path');

const raw = JSON.parse(fs.readFileSync(path.join(__dirname, '..', 'fixtures', 'raw', 'cap1-hidlog.json'), 'utf8'));
const cap1 = JSON.parse(fs.readFileSync(path.join(__dirname, '..', 'fixtures', 'cap1.json'), 'utf8'));

let failures = 0;
function fail(msg) { console.error(`FAIL: ${msg}`); failures++; }
function ok(msg) { console.log(`OK:   ${msg}`); }

// Also validate every raw entry is exactly 64 bytes -- the raw file is the
// ground truth, so if IT ever regresses to 63 bytes, nothing downstream can
// be trusted.
for (const e of raw.entries) {
  const n = e.hex.trim().split(/\s+/).length;
  if (n !== 64) fail(`raw/cap1-hidlog.json: ${e.dir} ${e.opcode} cmd${e.cmd} is ${n} bytes, expected 64`);
}

for (const rawEntry of raw.entries) {
  const opcode = parseInt(rawEntry.opcode, 16);
  const match = cap1.reports.find((r) => {
    const cmdMatches = rawEntry.cmd === 9 ? r.command_name.startsWith('cmd9') : r.command_name.startsWith('cmd10');
    return cmdMatches && r.direction === rawEntry.dir && parseInt(r.opcode_hex, 16) === opcode;
  });
  if (!match) { fail(`no fixtures/cap1.json report matches raw entry ${rawEntry.dir}/${rawEntry.opcode}/cmd${rawEntry.cmd}`); continue; }
  if (match.payload_hex !== rawEntry.hex) {
    fail(`fixtures/cap1.json "${match.command_name}" (${match.direction}) does not match raw evidence:\n  raw:     ${rawEntry.hex}\n  fixture: ${match.payload_hex}`);
  } else {
    ok(`fixtures/cap1.json "${match.command_name}" (${match.direction}) matches raw capture exactly`);
  }
}

if (failures > 0) {
  console.error(`\n${failures} failure(s) -- fixtures/cap1.json has drifted from the raw evidence it's supposed to represent.`);
  process.exit(1);
}
console.log('\nfixtures/cap1.json is fully consistent with the raw capture evidence.');

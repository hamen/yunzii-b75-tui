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

const fixturesDir = path.join(__dirname, '..', 'fixtures');
function load(rel) { return JSON.parse(fs.readFileSync(path.join(fixturesDir, rel), 'utf8')); }

let failures = 0;
function fail(msg) { console.error(`FAIL: ${msg}`); failures++; }
function ok(msg) { console.log(`OK:   ${msg}`); }

// Each pair: a raw evidence file (fixtures/raw/*.json) and the polished
// fixture it must match byte-for-byte, plus a matcher that maps a raw
// entry's cmd number to the command_name prefix used in the polished
// fixture's reports.
const PAIRS = [
  {
    rawFile: 'raw/cap1-hidlog.json',
    fixtureFile: 'cap1.json',
    cmdPrefix: (cmd) => (cmd === 9 ? 'cmd9' : 'cmd10'),
  },
  {
    rawFile: 'raw/cap-page-switch-hidlog.json',
    fixtureFile: 'page-switch.json',
    cmdPrefix: (cmd) => `cmd${cmd}`,
  },
  {
    rawFile: 'raw/cap-clear-picture-hidlog.json',
    fixtureFile: 'clear-picture.json',
    cmdPrefix: (cmd) => `cmd${cmd}`,
  },
];

for (const { rawFile, fixtureFile, cmdPrefix } of PAIRS) {
  const raw = load(rawFile);
  const fixture = load(fixtureFile);

  // Every raw entry must be exactly 64 bytes -- the raw file is the ground
  // truth, so if IT ever regresses to 63 bytes, nothing downstream can be
  // trusted.
  for (const e of raw.entries) {
    const n = e.hex.trim().split(/\s+/).length;
    if (n !== 64) fail(`${rawFile}: ${e.dir} ${e.opcode} cmd${e.cmd} is ${n} bytes, expected 64`);
  }

  for (const rawEntry of raw.entries) {
    const opcode = parseInt(rawEntry.opcode, 16);
    const prefix = cmdPrefix(rawEntry.cmd);
    const matches = fixture.reports.filter(
      (r) => r.command_name.startsWith(prefix) && r.direction === rawEntry.dir && parseInt(r.opcode_hex, 16) === opcode
    );
    if (matches.length === 0) {
      fail(`no ${fixtureFile} report matches raw entry ${rawEntry.dir}/${rawEntry.opcode}/cmd${rawEntry.cmd} (from ${rawFile})`);
      continue;
    }
    // A command can legitimately have more than one matching report of the
    // same opcode/direction (e.g. clear-picture's raw evidence stores one
    // representative pair while the built sequence repeats it 16x) -- every
    // match must agree with the raw bytes exactly.
    for (const match of matches) {
      if (match.payload_hex !== rawEntry.hex) {
        fail(`${fixtureFile} "${match.command_name}" (${match.direction}) does not match raw evidence in ${rawFile}:\n  raw:     ${rawEntry.hex}\n  fixture: ${match.payload_hex}`);
      } else {
        ok(`${fixtureFile} "${match.command_name}" (${match.direction}) matches raw capture exactly`);
      }
    }
  }
}

if (failures > 0) {
  console.error(`\n${failures} failure(s) -- a polished fixture has drifted from the raw evidence it's supposed to represent.`);
  process.exit(1);
}
console.log('\nAll polished fixtures are fully consistent with their raw capture evidence.');

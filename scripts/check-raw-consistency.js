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
// fixture it must match byte-for-byte AND in exact order. Both files are
// built (by scripts/build-fixtures.js and the raw-evidence generator
// respectively) by iterating the same command list in the same order, so a
// strict index-by-index comparison is the correct check here -- it is also
// what catches a report silently landing out of sequence, which a
// filter/multi-match comparison (an earlier version of this script) could
// not: round-1 cross-review (codex Blocker, PR #3) found that version only
// asserted "some report with this command/direction/opcode matches
// somewhere," which passed even for clear-picture's now-fixed evidence gap
// (only 1 of 16 repeats was stored) without ever checking count or order.
//
// The picture-upload pair (Milestone 3) is the strongest of the four: its
// fixture is not a transcription of the capture at all, it is REGENERATED
// from fixtures/test-quadrants.png through the protocol model by
// scripts/build-fixtures.js. So this comparison is model-vs-hardware, and a
// mistake anywhere in the decode -- channel order, per-pixel byte order,
// chunk size, offset encoding, the final chunk's length byte, the checksum
// width -- fails here rather than shipping.
const PAIRS = [
  { rawFile: 'raw/cap1-hidlog.json', fixtureFile: 'cap1.json' },
  { rawFile: 'raw/cap-page-switch-hidlog.json', fixtureFile: 'page-switch.json' },
  { rawFile: 'raw/cap-clear-picture-hidlog.json', fixtureFile: 'clear-picture.json' },
  { rawFile: 'raw/cap-picture-upload-hidlog.json', fixtureFile: 'picture-upload.json' },
];

// Above this many reports, print one summary line per pair instead of one
// line per report: picture-upload alone is 552 reports, and burying a real
// failure in 550 lines of "OK" is how a failure gets skimmed past.
const VERBOSE_REPORT_LIMIT = 100;

for (const { rawFile, fixtureFile } of PAIRS) {
  const raw = load(rawFile);
  const fixture = load(fixtureFile);

  // Every raw entry must be exactly 64 bytes -- the raw file is the ground
  // truth, so if IT ever regresses to 63 bytes, nothing downstream can be
  // trusted.
  for (const e of raw.entries) {
    const n = e.hex.trim().split(/\s+/).length;
    const which = e.name !== undefined ? e.name : `cmd${e.cmd}`;
    if (n !== 64) fail(`${rawFile}: ${e.dir} ${e.opcode} ${which} is ${n} bytes, expected 64`);
  }

  if (raw.entries.length !== fixture.reports.length) {
    fail(
      `${rawFile} has ${raw.entries.length} entries but ${fixtureFile} has ${fixture.reports.length} reports -- counts must match exactly (no sampling/summarizing on either side)`
    );
    continue;
  }

  const verbose = raw.entries.length <= VERBOSE_REPORT_LIMIT;
  let matched = 0;

  for (let i = 0; i < raw.entries.length; i++) {
    const rawEntry = raw.entries[i];
    const report = fixture.reports[i];
    const rawOpcode = parseInt(rawEntry.opcode, 16);
    const reportOpcode = parseInt(report.opcode_hex, 16);
    const label = `${fixtureFile}[${i}] "${report.command_name}" (${report.direction}) vs ${rawFile}[${i}]`;

    if (report.direction !== rawEntry.dir || reportOpcode !== rawOpcode) {
      fail(
        `${label}: direction/opcode mismatch at the SAME index -- raw is ${rawEntry.dir}/${rawEntry.opcode}, fixture is ${report.direction}/${report.opcode_hex}. Reports are out of order.`
      );
      continue;
    }
    if (report.payload_hex !== rawEntry.hex) {
      // Always reported in full, however large the pair: a mismatch is the
      // entire point of this script.
      fail(`${label}: bytes don't match:\n  raw:     ${rawEntry.hex}\n  fixture: ${report.payload_hex}`);
    } else {
      matched++;
      if (verbose) ok(`${label}: matches raw capture exactly, same position`);
    }
  }

  if (!verbose) {
    ok(`${fixtureFile}: all ${matched}/${raw.entries.length} reports match ${rawFile} exactly, in order`);
  }
}

if (failures > 0) {
  console.error(`\n${failures} failure(s) -- a polished fixture has drifted from the raw evidence it's supposed to represent.`);
  process.exit(1);
}
console.log('\nAll polished fixtures are fully consistent with their raw capture evidence.');

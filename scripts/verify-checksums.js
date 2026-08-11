// Verifies the outer-report checksum formula and the vendor's inner CRC-16
// against the vendor's own hardcoded constants (see
// scripts/vendor-source-excerpt.js). This is a from-first-principles check
// independent of fixtures/*.json; scripts/check-coverage.js separately
// verifies fixtures against fields.json and against this same formula.
// Run with `node scripts/verify-checksums.js`. All assertions passed on
// 2026-08-10.

function ga(t) {
  let n = 65535;
  for (let r = 0; r < t.length; r++) {
    n ^= t[r];
    for (let i = 0; i < 8; i++) n & 1 ? (n >>= 1, n ^= 40961) : n >>= 1;
  }
  return [n >> 8 & 255, n & 255];
}

// lengthByte is an explicit parameter, NOT payload.length: the "finish"
// command sends length=0x38 with zero real payload bytes, so length is
// independent wire data, not derived from the payload array.
//
// b1/b2 are report bytes 1-2. They are zero for every command except
// picture-upload's bulk packets, where they carry the little-endian data
// offset -- and they are part of the checksummed prefix either way. This is
// ONE formula for all three opcodes (Milestone 3 correction; see below).
function outerChecksum16(opcode, lengthByte, payload, b1 = 0, b2 = 0) {
  const sum = opcode + b1 + b2 + lengthByte + payload.reduce((a, b) => a + b, 0);
  return [sum & 255, (sum >> 8) & 255]; // [lo, hi], as observed on the wire
}

// Kept only to demonstrate WHY the old model looked right for so long: it
// returns byte 4, which is the same value under both models. What the old
// model got wrong was byte 5, which it called a reserved zero.
function outerChecksum8(opcode, lengthByte, payload) {
  return (opcode + lengthByte + payload.reduce((a, b) => a + b, 0)) & 255;
}

function assertEqual(label, actual, expected) {
  const a = JSON.stringify(actual), e = JSON.stringify(expected);
  if (a !== e) throw new Error(`FAIL ${label}: got ${a}, expected ${e}`);
  console.log(`OK   ${label}: ${a}`);
}

const D = [165, 90, 9, 0, 3, 195, 225];
const T = [165, 90, 10, 0, 4, 1, 80];

assertEqual('vendor inner CRC ga([9,0,3])', ga([9, 0, 3]), [195, 225]);
assertEqual('vendor inner CRC ga([10,0,4])', ga([10, 0, 4]), [1, 80]);

assertEqual('outer checksum for D (opcode 0x40, len 7)', outerChecksum16(0x40, D.length, D), [0xf6, 0x02]);
assertEqual('outer checksum for T (opcode 0x40, len 7)', outerChecksum16(0x40, T.length, T), [0xa5, 0x01]);

// Milestone 2 commands (page-switch, clear-picture) -- live-captured
// 2026-08-11, checksum bytes are the REAL values observed on the wire.
const CMD11 = [165, 90, 11, 0, 0, 2, 0];
const CMD13 = [165, 90, 13, 0, 0, 3, 224];
const CMD15 = [165, 90, 15, 0, 0, 195, 65];
const CMD14 = [165, 90, 14, 0, 0, 3, 16];

assertEqual('vendor inner CRC ga([11,0,0])', ga([11, 0, 0]), [2, 0]);
assertEqual('vendor inner CRC ga([13,0,0])', ga([13, 0, 0]), [3, 224]);
assertEqual('vendor inner CRC ga([15,0,0])', ga([15, 0, 0]), [195, 65]);
assertEqual('vendor inner CRC ga([14,0,0])', ga([14, 0, 0]), [3, 16]);

assertEqual('outer checksum for cmd11 (opcode 0x40, len 7)', outerChecksum16(0x40, CMD11.length, CMD11), [0x53, 0x01]);
assertEqual('outer checksum for cmd13 (opcode 0x40, len 7)', outerChecksum16(0x40, CMD13.length, CMD13), [0x36, 0x02]);
assertEqual('outer checksum for cmd15 (opcode 0x40, len 7)', outerChecksum16(0x40, CMD15.length, CMD15), [0x59, 0x02]);
assertEqual('outer checksum for cmd14 (opcode 0x40, len 7)', outerChecksum16(0x40, CMD14.length, CMD14), [0x67, 0x01]);

// "Clear GIF" (cmd18/cmd19) -- deferred, NOT shipped as a CLI command this
// milestone (see fields.json unresolved[]), but its checksums are verified
// here anyway since the bytes were captured and the formula applies cleanly
// even to the still-unresolved parts of the payload.
const CMD18 = [165, 90, 18, 0, 1, 5, 16, 1, 0];
const CMD19 = [165, 90, 19, 0, 2, 196, 1, 1, 0];
assertEqual('vendor inner CRC ga([18,0,1])', ga([18, 0, 1]), [5, 16]);
assertEqual('vendor inner CRC ga([19,0,2])', ga([19, 0, 2]), [196, 1]);
assertEqual('outer checksum for cmd18 (opcode 0x40, len 9)', outerChecksum16(0x40, CMD18.length, CMD18), [0x71, 0x01]);
assertEqual('outer checksum for cmd19 (opcode 0x40, len 9)', outerChecksum16(0x40, CMD19.length, CMD19), [0x23, 0x02]);

assertEqual('P cap1 [hour=19,min=24,sec=13]', outerChecksum8(0x41, 3, [19, 24, 13]), 0x7c);
assertEqual('P cap2 [hour=19,min=28,sec=14]', outerChecksum8(0x41, 3, [19, 28, 14]), 0x81);
assertEqual('P cap3 [hour=19,min=29,sec=55]', outerChecksum8(0x41, 3, [19, 29, 55]), 0xab);

assertEqual('M [year=26,weekday=1,month=8,date=10]', outerChecksum8(0x41, 4, [26, 1, 8, 10]), 0x72);

// "finish": length byte is a fixed constant (0x38) unrelated to any real
// payload -- zero actual payload bytes are summed.
assertEqual('finish (opcode 0x42, len 0x38, no payload)', outerChecksum8(0x42, 0x38, []), 0x7a);

// Constructive test (added after round-3 cross-review, grok SF6): build a
// FULL 64-byte report for a timestamp that was never captured, matching the
// plan's actual exit rule -- "can a reader construct new bytes from the
// docs," not just replay known samples.
// Cross-review round 1 (codex Should-fix, PR #4): this helper still branched
// on opcode and wrote an 8-bit checksum with a zero byte 5 for 0x41/0x42 --
// the exact model the rest of this file now documents as wrong. It produced
// correct bytes only while the sum stayed under 256. Unified.
function buildReport(opcode, lengthByte, payload, b1 = 0, b2 = 0) {
  const bytes = new Array(64).fill(0);
  bytes[0] = opcode;
  bytes[1] = b1;
  bytes[2] = b2;
  bytes[3] = lengthByte;
  const [lo, hi] = outerChecksum16(opcode, lengthByte, payload, b1, b2);
  bytes[4] = lo;
  bytes[5] = hi;
  for (let i = 0; i < payload.length; i++) bytes[7 + i] = payload[i];
  return bytes;
}

// An hour/minute/second combination never seen in any capture (23:59:58).
const neverCapturedP = [23, 59, 58];
const builtCmd9Data = buildReport(0x41, 3, neverCapturedP);
assertEqual('constructed report length is 64 bytes', builtCmd9Data.length, 64);
assertEqual('constructed report checksum for hour=23,min=59,sec=58', [builtCmd9Data[4], builtCmd9Data[5]], outerChecksum16(0x41, 3, neverCapturedP));
assertEqual('constructed report payload round-trips', builtCmd9Data.slice(7, 10), neverCapturedP);

// --- Milestone 3: picture upload (2026-08-11) ---

const CMD16 = [165, 90, 16, 0, 1, 197, 177, 1]; // 8 bytes, not 7
const CMD12 = [165, 90, 12, 120, 0, 195, 147];

assertEqual('vendor inner CRC ga([16,0,1])', ga([16, 0, 1]), [197, 177]);
assertEqual('vendor inner CRC ga([12,120,0])', ga([12, 120, 0]), [195, 147]);

assertEqual('outer checksum for cmd16 start (opcode 0x40, len 8)', outerChecksum16(0x40, CMD16.length, CMD16), [0xcf, 0x02]);
assertEqual('outer checksum for cmd12 declare-size (opcode 0x41, len 7)', outerChecksum16(0x41, CMD12.length, CMD12), [0x21, 0x03]);

// The declare-size report is the compact counter-example that kills the old
// "byte 5 is a reserved zero" model: its sum is 0x0321, so the wire carries
// 0x03 at byte 5. The low byte, 0x21, is identical under both models -- which
// is exactly why the mistake survived two milestones.
assertEqual('cmd12 low byte is the same under the old 8-bit model', outerChecksum8(0x41, CMD12.length, CMD12), 0x21);
assertEqual('cmd12 high byte is NOT zero (old model said reserved 0x00)', outerChecksum16(0x41, CMD12.length, CMD12)[1], 0x03);

// Bulk packets: the offset in bytes 1-2 is part of the sum. Both examples are
// real reports from fixtures/raw/cap-picture-upload-hidlog.json.
//
// First bulk: offset 0, length 0x38, payload = 28 pixels of pure red (0xf800),
// i.e. 28 x [0xf8, 0x00].
const redPixels = [];
for (let i = 0; i < 28; i++) redPixels.push(0xf8, 0x00);
assertEqual('bulk offset 0 (opcode 0x41, len 0x38, 28x red)', outerChecksum16(0x41, 0x38, redPixels, 0x00, 0x00), [0x99, 0x1b]);

// Second bulk: identical payload, offset 56 -- so the ONLY difference in the
// sum is the offset byte, which shifts the checksum by exactly 56 (0x1b99 +
// 0x38 = 0x1bd1). This is the check that would fail if bytes 1-2 were left
// out of the sum.
assertEqual('bulk offset 56 (same payload, offset is in the sum)', outerChecksum16(0x41, 0x38, redPixels, 0x38, 0x00), [0xd1, 0x1b]);

// The same thing through buildReport(), which used to take an 8-bit branch
// for 0x41 and would have written 0x00 into byte 5 here.
const builtBulk = buildReport(0x41, 0x38, redPixels, 0x00, 0x00);
assertEqual('constructed bulk report byte 5 is the checksum high byte', [builtBulk[4], builtBulk[5]], [0x99, 0x1b]);

// Finally, run the one formula over EVERY report in the real picture-upload
// capture. This reads fixtures/raw/, which is observed hardware traffic, not
// a fixture derived from this model -- so it is still a check of the formula
// against reality rather than against itself, and it is the check that makes
// the correction above quantitative instead of asserted.
//
// The bytes are read from the file rather than transcribed. Hand-typing
// 64-token hex strings has produced a silent 63-byte error four separate
// times in this repo's history, including once in a throwaway double-check
// that existed only to be careful.
const fs = require('fs');
const pathMod = require('path');
const rawPath = pathMod.join(__dirname, '..', 'fixtures', 'raw', 'cap-picture-upload-hidlog.json');
const rawEntries = JSON.parse(fs.readFileSync(rawPath, 'utf8')).entries;
const rawReports = rawEntries.map((e) => e.hex.trim().split(/\s+/).map((b) => parseInt(b, 16)));

let unifiedOk = 0;
let oldModelOk = 0;
let lowByteOk = 0;
for (const r of rawReports) {
  const len = r[3];
  const payload = r.slice(7, 7 + len);
  const [lo, hi] = outerChecksum16(r[0], len, payload, r[1], r[2]);
  if (lo === r[4] && hi === r[5]) unifiedOk++;
  if (lo === r[4]) lowByteOk++;
  if (lo === r[4] && r[5] === 0) oldModelOk++;
}
assertEqual('every captured report matches the unified 16-bit LE model', unifiedOk, rawReports.length);
assertEqual('byte 4 alone was always right (why the old model survived)', lowByteOk, rawReports.length);
assertEqual('reports fitting the OLD 8-bit + reserved-zero model: only `finish`', oldModelOk, 1);

// Structure of the same capture, checked from the bytes rather than asserted.
const bulk = rawReports.filter((r) => r[0] === 0x41 && !(r[7] === 165 && r[8] === 90));
const offsets = bulk.map((r) => r[1] | (r[2] << 8));
const totalBytes = bulk.reduce((a, r) => a + r[3], 0);
assertEqual('bulk packet count', bulk.length, 549);
assertEqual('bulk offsets are 0,56,...,30688 with no gaps', offsets.every((o, i) => o === i * 56), true);
assertEqual('bulk payload bytes total exactly 160*96*2', totalBytes, 160 * 96 * 2);
assertEqual('lengths: 548 full 56-byte chunks', bulk.filter((r) => r[3] === 0x38).length, 548);
assertEqual('lengths: the last chunk declares 32, NOT a padded 56', bulk[bulk.length - 1][3], 0x20);

console.log('\nAll checks passed.');

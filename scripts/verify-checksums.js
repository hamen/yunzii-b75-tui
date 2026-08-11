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
function outerChecksum16(opcode, lengthByte, payload) {
  const sum = opcode + lengthByte + payload.reduce((a, b) => a + b, 0);
  return [sum & 255, (sum >> 8) & 255]; // [lo, hi], as observed on the wire
}

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
function buildReport(opcode, lengthByte, payload) {
  const bytes = new Array(64).fill(0);
  bytes[0] = opcode;
  bytes[3] = lengthByte;
  if (opcode === 0x40) {
    const [lo, hi] = outerChecksum16(opcode, lengthByte, payload);
    bytes[4] = lo; bytes[5] = hi;
  } else {
    bytes[4] = outerChecksum8(opcode, lengthByte, payload);
  }
  for (let i = 0; i < payload.length; i++) bytes[7 + i] = payload[i];
  return bytes;
}

// An hour/minute/second combination never seen in any capture (23:59:58).
const neverCapturedP = [23, 59, 58];
const builtCmd9Data = buildReport(0x41, 3, neverCapturedP);
assertEqual('constructed report length is 64 bytes', builtCmd9Data.length, 64);
assertEqual('constructed report checksum for hour=23,min=59,sec=58', builtCmd9Data[4], outerChecksum8(0x41, 3, neverCapturedP));
assertEqual('constructed report payload round-trips', builtCmd9Data.slice(7, 10), neverCapturedP);

console.log('\nAll checks passed.');

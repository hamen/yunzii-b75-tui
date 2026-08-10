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

assertEqual('P cap1 [hour=19,min=24,sec=13]', outerChecksum8(0x41, 3, [19, 24, 13]), 0x7c);
assertEqual('P cap2 [hour=19,min=28,sec=14]', outerChecksum8(0x41, 3, [19, 28, 14]), 0x81);
assertEqual('P cap3 [hour=19,min=29,sec=55]', outerChecksum8(0x41, 3, [19, 29, 55]), 0xab);

assertEqual('M [year=26,weekday=1,month=8,date=10]', outerChecksum8(0x41, 4, [26, 1, 8, 10]), 0x72);

// "finish": length byte is a fixed constant (0x38) unrelated to any real
// payload -- zero actual payload bytes are summed.
assertEqual('finish (opcode 0x42, len 0x38, no payload)', outerChecksum8(0x42, 0x38, []), 0x7a);

console.log('\nAll checks passed.');

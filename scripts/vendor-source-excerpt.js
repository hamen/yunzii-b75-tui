// Excerpt from https://yunzii-game.com/assets/index-8Bj3uPPc.js (2026-08-10).
// Fetched client-side from the page already loaded in the browser (not
// extracted from firmware or any protected asset). Minified variable names
// kept as-is; comments added for clarity. Two excerpts: the CRC-16 helper
// and the "Update device time" click handler.

// --- offset ~1828349: opcode table (deminified property names, values as found) ---
// magneticAxis*: ... ,
// sendScreenControlInformationPackage: "0x40",
// sendScreenControlDataPacket: "0x41",
// finishScreenControlDataPacket: "0x42",
// getDongleAndKeyboardStatus: "0x55",
// getFirmwareVersion: "0xB0",
// toBootLoader: "0xB1",
// getBootLoaderStatus: "0xB2",
// confirmFirmwareInfo: "0xB3",
// startUpgrade: "0xB4",
// transferUpgradeData: "0xB5",
// upgradeComplete: "0xB6",
// endUpgrade: "0xB7",
// ...

// --- CRC-16/ARC (poly 0xA001 reflected, init 0xFFFF) ---
// Used ONLY to precompute the constant inner command bytes (baked into the
// bundle as literals for cmd-9/cmd-10 headers) -- NOT used for the outer
// 64-byte HID report checksum (see FINDINGS.md).
function ga(t) {
  let n = 65535;
  for (let r = 0; r < t.length; r++) {
    n ^= t[r];
    for (let i = 0; i < 8; i++)
      n & 1 ? (n >>= 1, n ^= 40961) : n >>= 1;
  }
  return [n >> 8 & 255, n & 255];
}

// --- offset ~1828349-1829300: the button handlers for Equipment setup ---
// (Lu(a => a.X) resolves to the store action for opcode X; deminified names
// substituted for the opcode table above.)

const switchToHomepage = async () => {
  const crc = ga([11, 0, 0]);
  const pkg = [165, 90, 11, 0, 0, crc[0], crc[1]];
  await sendScreenControlInformationPackage(pkg);
  await finishScreenControlDataPacket();
};

const switchToPicturePage = async () => {
  const crc = ga([13, 0, 0]);
  const pkg = [165, 90, 13, 0, 0, crc[0], crc[1]];
  await sendScreenControlInformationPackage(pkg);
  await finishScreenControlDataPacket();
};

const switchToGifPage = async () => {
  const crc = ga([15, 0, 0]);
  const pkg = [165, 90, 15, 0, 0, crc[0], crc[1]];
  await sendScreenControlInformationPackage(pkg);
  await finishScreenControlDataPacket();
};

const clearPictureOrGif_loop16x = async () => {
  // for (a=0; a<16; a++) { crc = ga([14,0,0]); pkg = [165,90,14,0,0,crc0,crc1]; ... }
  // (truncated in the fetched excerpt -- opcode 14, sent 16 times)
};

// THE ACTUAL "Update device time" HANDLER (dayjs instance called `Uf()`):
const updateDeviceTime = async () => {
  const hour = Uf().hour();
  const minute = Uf().minute();
  const second = Uf().second();
  const year2 = Number(Uf().format("YY"));
  const month = Uf().month() + 1;
  const date = Uf().date();
  const weekday = Uf().day() || 7;

  const D = [165, 90, 9, 0, 3, 195, 225];   // ga([9,0,3]) === [195,225], precomputed
  const T = [165, 90, 10, 0, 4, 1, 80];     // ga([10,0,4]) === [1,80], precomputed
  const P = [hour, minute, second];
  const M = [year2, weekday, month, date];

  for (let i = 0; i < 3; i++) {
    await sendScreenControlInformationPackage(D);
    await sendScreenControlDataPacket(P);
    await finishScreenControlDataPacket();
    await sendScreenControlInformationPackage(T);
    await sendScreenControlDataPacket(M);
    await finishScreenControlDataPacket();
  }
};

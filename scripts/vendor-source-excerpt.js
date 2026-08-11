// Excerpt from https://yunzii-game.com/assets/index-8Bj3uPPc.js (2026-08-10).
// Fetched client-side from the page already loaded in the browser (not
// extracted from firmware or any protected asset).
//
// CORRECTION (found in PR #1 round-2 cross-review): an earlier version of
// this file claimed to keep "minified variable names as-is" while actually
// showing deminified names (switchToHomepage, updateDeviceTime, etc.) --
// self-contradictory, and not literally the source text. This version
// includes the actual verbatim minified text FIRST, exactly as fetched, and
// the deminified version SECOND, clearly labeled as a readability aid, not
// as evidence in itself.

// =====================================================================
// VERBATIM (offset ~1828349-1829300 of the fetched bundle, minified as-is)
// =====================================================================

// CRC-16/ARC helper, verbatim:
//   function ga(t){let n=65535;for(let r=0;r<t.length;r++){n^=t[r];for(let i=0;i<8;i++)n&1?(n>>=1,n^=40961):n>>=1}return[n>>8&255,n&255]}

// Equipment-setup button handlers, verbatim:
//   const hDe=()=>{const t=Lu(a=>a.sendScreenControlInformationPackage),n=Lu(a=>a.finishScreenControlDataPacket),r=Lu(a=>a.sendScreenControlDataPacket),i=async()=>{const a=ga([11,0,0]),g=[165,90,11,0,0,a[0],a[1]];await t(g),await n()},o=async()=>{const a=ga([13,0,0]),g=[165,90,13,0,0,a[0],a[1]];await t(g),await n()},c=async()=>{const a=ga([15,0,0]),g=[165,90,15,0,0,a[0],a[1]];await t(g),await n()},d=async()=>{for(let a=0;a<16;a++){const g=ga([14,0,0]),p=[165,90,14,0,0,g[0],g[1]];await t(p),await n()}}, ...

// THE "Update device time" handler, verbatim:
//   f=async()=>{const a=Uf().hour(),g=Uf().minute(),p=Uf().second(),m=Number(Uf().format("YY")),w=Uf().month()+1,x=Uf().date(),S=Uf().day()||7,D=[165,90,9,0,3,195,225],T=[165,90,10,0,4,1,80],P=[a,g,p],M=[m,S,w,x];for(let O=0;O<3;O++)await t(D),await r(P),await n(),await t(T),await r(M),await n()}

// Opcode table (property names in the fetched bundle are NOT minified here --
// this object's keys are used as-is by the site's own code, verbatim):
//   sendScreenControlInformationPackage:"0x40",sendScreenControlDataPacket:"0x41",finishScreenControlDataPacket:"0x42",getDongleAndKeyboardStatus:"0x55",getFirmwareVersion:"0xB0",toBootLoader:"0xB1",getBootLoaderStatus:"0xB2",confirmFirmwareInfo:"0xB3",startUpgrade:"0xB4",transferUpgradeData:"0xB5",upgradeComplete:"0xB6",endUpgrade:"0xB7"

// =====================================================================
// DEMINIFIED (readability aid only -- not verbatim evidence; see above
// for the actual source text this is derived from)
// =====================================================================

function ga(t) {
  let n = 65535;
  for (let r = 0; r < t.length; r++) {
    n ^= t[r];
    for (let i = 0; i < 8; i++)
      n & 1 ? (n >>= 1, n ^= 40961) : n >>= 1;
  }
  return [n >> 8 & 255, n & 255];
}

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

// CORRECTIVE NOTE (Milestone 2, live capture, 2026-08-11): despite this
// function's name (from the original fetched/deminified excerpt, kept below
// verbatim), the 16x loop applies ONLY to "Clear the picture" (cmd14).
// "Clear GIF" is a DIFFERENT, non-looped sequence (cmd18 then cmd19, each
// sent once) -- see fields.json's unresolved[] entry for cmd18/19 and
// commands.cmd14_clearPicture. This function was never renamed in the
// vendor's own minified bundle either; the name is simply misleading, not
// evidence the two buttons share a handler.
const clearPictureOrGif_loop16x = async () => {
  // for (a=0; a<16; a++) { crc = ga([14,0,0]); pkg = [165,90,14,0,0,crc0,crc1]; ... }
  // (truncated in the fetched excerpt -- opcode 14, sent 16 times)
  // Confirmed by live capture: this is "Clear the picture" ONLY.
};

// "Clear GIF" -- confirmed by live capture (Milestone 2, 2026-08-11) to be
// unrelated to clearPictureOrGif_loop16x above: two different inner
// commands, cmd18 then cmd19, each sent once (no loop). Reconstructed here
// from captured bytes, not fetched from the bundle directly.
const clearGif_notLooped = async () => {
  const crc18 = ga([18, 0, 1]);
  const pkg18 = [165, 90, 18, 0, 1, crc18[0], crc18[1], 1, 0]; // trailing [1,0] meaning unresolved
  await sendScreenControlInformationPackage(pkg18);
  await finishScreenControlDataPacket();

  const crc19 = ga([19, 0, 2]);
  const pkg19 = [165, 90, 19, 0, 2, crc19[0], crc19[1], 1, 0]; // trailing [1,0] meaning unresolved
  await sendScreenControlInformationPackage(pkg19);
  await finishScreenControlDataPacket();
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

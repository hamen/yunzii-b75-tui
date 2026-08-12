#!/usr/bin/env node
'use strict';
//
// Builds every animated-GIF fixture this repo tests against.
//
//   node scripts/make-test-gifs.js
//
// These fixtures exist to pin behaviour that is easy to get wrong and hard to
// notice: sub-rectangle placement, disposal methods, transparency, and the
// frame-delay cases that drive rate selection. They must be reproducible, so
// this file is a complete GIF89a writer with no dependencies -- the repo has
// no package.json and shelling out to ImageMagick would make the fixtures
// only as reproducible as whatever happens to be installed.
//
// The LZW coder below is the real thing (variable code width, clear codes on
// table overflow), not the "emit a clear code every pixel" shortcut. The
// shortcut produces files that most decoders accept and some reject, which is
// exactly the kind of fixture that fails for a reason unrelated to the test.

const fs = require('fs');
const path = require('path');

// --- GIF89a writer -------------------------------------------------------

/** LZW-compresses `indices` (one byte per pixel) at the given code width. */
function lzwEncode(indices, minCodeSize) {
  const clearCode = 1 << minCodeSize;
  const endCode = clearCode + 1;

  const out = [];
  let cur = 0; // bit accumulator, LSB first
  let curBits = 0;
  const emit = (code, width) => {
    cur |= code << curBits;
    curBits += width;
    while (curBits >= 8) {
      out.push(cur & 0xff);
      cur >>= 8;
      curBits -= 8;
    }
  };

  let dict = new Map();
  let nextCode, codeWidth;
  const resetDict = () => {
    dict = new Map();
    for (let i = 0; i < clearCode; i++) dict.set(String(i), i);
    nextCode = endCode + 1;
    codeWidth = minCodeSize + 1;
  };

  resetDict();
  emit(clearCode, codeWidth);

  let prefix = String(indices[0]);
  for (let i = 1; i < indices.length; i++) {
    const k = indices[i];
    const candidate = prefix + ',' + k;
    if (dict.has(candidate)) {
      prefix = candidate;
      continue;
    }
    emit(dict.get(prefix), codeWidth);
    dict.set(candidate, nextCode++);
    // The width grows *after* the code that filled the table is emitted.
    if (nextCode > (1 << codeWidth)) {
      if (codeWidth < 12) {
        codeWidth++;
      } else {
        emit(clearCode, codeWidth);
        resetDict();
      }
    }
    prefix = String(k);
  }
  emit(dict.get(prefix), codeWidth);
  emit(endCode, codeWidth);
  if (curBits > 0) out.push(cur & 0xff);

  return out;
}

/** Wraps raw LZW output in GIF's 255-byte sub-block chain. */
function subBlocks(bytes) {
  const out = [];
  for (let i = 0; i < bytes.length; i += 255) {
    const chunk = bytes.slice(i, i + 255);
    out.push(chunk.length, ...chunk);
  }
  out.push(0); // block terminator
  return out;
}

function u16le(v) {
  return [v & 0xff, (v >> 8) & 0xff];
}

/**
 * @param {object} spec
 * @param {number} spec.width           logical screen width
 * @param {number} spec.height          logical screen height
 * @param {number[][]} spec.palette     up to 256 [r,g,b] entries
 * @param {number} [spec.background]    background colour index
 * @param {object[]} spec.frames        see below
 *
 * Each frame: { left, top, width, height, indices, delayCs, disposal,
 * transparentIndex }. `disposal` is the raw GIF value: 0 unspecified, 1 do not
 * dispose, 2 restore to background, 3 restore to previous.
 */
function writeGif(outPath, spec) {
  const { width, height, palette, frames } = spec;
  const background = spec.background ?? 0;

  // The global colour table must be a power of two, at least 2 entries.
  let tableBits = 1;
  while (1 << tableBits < palette.length) tableBits++;
  const tableSize = 1 << tableBits;

  const bytes = [];
  const push = (...b) => bytes.push(...b.flat());

  push([0x47, 0x49, 0x46, 0x38, 0x39, 0x61]); // "GIF89a"
  push(u16le(width), u16le(height));
  push(0x80 | (tableBits - 1)); // global table present, size
  push(background, 0x00); // background index, pixel aspect ratio

  for (let i = 0; i < tableSize; i++) {
    const [r, g, b] = palette[i] ?? [0, 0, 0];
    push(r, g, b);
  }

  // NETSCAPE2.0 loop-forever, so the fixtures behave like real animations.
  push(0x21, 0xff, 0x0b);
  push([...'NETSCAPE2.0'].map((c) => c.charCodeAt(0)));
  push(0x03, 0x01, u16le(0), 0x00);

  for (const f of frames) {
    const transparent = f.transparentIndex ?? null;
    const disposal = f.disposal ?? 0;

    // Graphic Control Extension: disposal, delay, transparency.
    push(0x21, 0xf9, 0x04);
    push(((disposal & 0x07) << 2) | (transparent !== null ? 0x01 : 0x00));
    push(u16le(f.delayCs ?? 0));
    push(transparent ?? 0, 0x00);

    // Image Descriptor: no local colour table, not interlaced.
    push(0x2c);
    push(u16le(f.left ?? 0), u16le(f.top ?? 0));
    push(u16le(f.width), u16le(f.height));
    push(0x00);

    const minCodeSize = Math.max(2, tableBits);
    push(minCodeSize);
    push(subBlocks(lzwEncode(f.indices, minCodeSize)));
  }

  push(0x3b); // trailer

  fs.writeFileSync(outPath, Buffer.from(bytes));
  return bytes.length;
}

// --- helpers -------------------------------------------------------------

/** A solid rectangle of palette index `idx`, as a `w`x`h` index buffer. */
function solid(w, h, idx) {
  return new Array(w * h).fill(idx);
}

/** Paints a filled rectangle into a full-canvas index buffer. */
function rect(buf, canvasW, x, y, w, h, idx) {
  for (let row = y; row < y + h; row++) {
    for (let col = x; col < x + w; col++) {
      buf[row * canvasW + col] = idx;
    }
  }
  return buf;
}

// --- the fixtures --------------------------------------------------------

const FIX = path.join(__dirname, '..', 'fixtures');

// Palette shared by the composition fixtures. Index 0 is the mid-grey the
// tests assert against as "background", and it must stay 128,128,128: that is
// RGB565 0x8410, a value the Rust test names literally.
const PALETTE = [
  [128, 128, 128], // 0 grey
  [255, 0, 0], // 1 red
  [0, 0, 255], // 2 blue
  [0, 255, 0], // 3 green
];

const built = [];

// 1. Two full-canvas frames, uniform 100 ms delays -> a clean 10 fps.
//    Drives the "native rate is used when it is in range" test.
{
  const W = 160;
  const H = 96;
  const f0 = solid(W, H, 1);
  const f1 = solid(W, H, 2);
  const name = 'test-anim-2frames.gif';
  const n = writeGif(path.join(FIX, name), {
    width: W,
    height: H,
    palette: PALETTE,
    frames: [
      { left: 0, top: 0, width: W, height: H, indices: f0, delayCs: 10, disposal: 1 },
      { left: 0, top: 0, width: W, height: H, indices: f1, delayCs: 10, disposal: 1 },
    ],
  });
  built.push([name, n, '2 frames, 100 ms each -> 10 fps']);
}

// 2. Sub-rectangle placement with "do not dispose".
//    Frame 1 is a small rectangle that only means anything once composed onto
//    frame 0, so an implementation that encoded raw sub-frames fails here.
//    This does NOT prove disposal is honoured -- see fixture 3.
{
  const W = 64;
  const H = 48;
  const f0 = rect(solid(W, H, 0), W, 4, 4, 16, 12, 1); // grey + red top-left
  const name = 'test-anim-disposal.gif';
  const n = writeGif(path.join(FIX, name), {
    width: W,
    height: H,
    palette: PALETTE,
    frames: [
      { left: 0, top: 0, width: W, height: H, indices: f0, delayCs: 10, disposal: 1 },
      {
        left: 48,
        top: 36,
        width: 16,
        height: 12,
        indices: solid(16, 12, 2), // blue
        delayCs: 10,
        disposal: 1,
      },
    ],
  });
  built.push([name, n, 'sub-rect placement, "do not dispose"']);
}

// 3. Disposal actually being applied.
//    Frame 0 is full-canvas grey with a red mark, and its disposal is
//    "restore to background" (2). Before frame 1 is drawn the canvas must be
//    cleared, so the red mark MUST be gone -- a decoder that ignores disposal
//    leaves it there and the test fails. Frame 1 itself is a small green
//    rectangle somewhere else entirely.
{
  const W = 64;
  const H = 48;
  const f0 = rect(solid(W, H, 0), W, 4, 4, 16, 12, 1); // grey + red top-left
  const name = 'test-anim-disposal-background.gif';
  const n = writeGif(path.join(FIX, name), {
    width: W,
    height: H,
    palette: PALETTE,
    background: 0,
    frames: [
      { left: 0, top: 0, width: W, height: H, indices: f0, delayCs: 10, disposal: 2 },
      {
        left: 48,
        top: 36,
        width: 16,
        height: 12,
        indices: solid(16, 12, 3), // green
        delayCs: 10,
        disposal: 1,
      },
    ],
  });
  built.push([name, n, '"restore to background" clears frame 0']);
}

// 4. Delays that vary between frames -> no native rate, warn and fall back.
{
  const W = 32;
  const H = 32;
  const name = 'test-anim-variable-delay.gif';
  const n = writeGif(path.join(FIX, name), {
    width: W,
    height: H,
    palette: PALETTE,
    frames: [
      { left: 0, top: 0, width: W, height: H, indices: solid(W, H, 1), delayCs: 10, disposal: 1 },
      { left: 0, top: 0, width: W, height: H, indices: solid(W, H, 2), delayCs: 40, disposal: 1 },
      { left: 0, top: 0, width: W, height: H, indices: solid(W, H, 3), delayCs: 10, disposal: 1 },
    ],
  });
  built.push([name, n, 'delays 100/400/100 ms -> variable']);
}

// 5. Uniform delays that ask for a rate the device cannot store.
//    2 cs is 20 ms, which is 50 fps and legal; 1 cs is 10 ms, which is 100 fps
//    and is not. This fixture is the out-of-range case.
{
  const W = 32;
  const H = 32;
  const name = 'test-anim-too-fast.gif';
  const n = writeGif(path.join(FIX, name), {
    width: W,
    height: H,
    palette: PALETTE,
    frames: [
      { left: 0, top: 0, width: W, height: H, indices: solid(W, H, 1), delayCs: 1, disposal: 1 },
      { left: 0, top: 0, width: W, height: H, indices: solid(W, H, 2), delayCs: 1, disposal: 1 },
    ],
  });
  built.push([name, n, 'uniform 10 ms -> 100 fps, out of range']);
}

// 6. Uniform delays slower than the device's floor.
//    150 cs is 1500 ms, which is 0.67 fps -- below the 1 fps minimum. This is
//    the case that used to round UP to 1 and pass as an exact match.
{
  const W = 32;
  const H = 32;
  const name = 'test-anim-too-slow.gif';
  const n = writeGif(path.join(FIX, name), {
    width: W,
    height: H,
    palette: PALETTE,
    frames: [
      { left: 0, top: 0, width: W, height: H, indices: solid(W, H, 1), delayCs: 150, disposal: 1 },
      { left: 0, top: 0, width: W, height: H, indices: solid(W, H, 2), delayCs: 150, disposal: 1 },
    ],
  });
  built.push([name, n, 'uniform 1500 ms -> 0.67 fps, below the floor']);
}

// 7. No frame delay at all -- "as fast as possible".
//    Every delay is 0, so they are uniform but do not express a rate. Folding
//    this into the variable case made the warning claim the delays differed
//    when all of them were identical.
{
  const W = 32;
  const H = 32;
  const name = 'test-anim-zero-delay.gif';
  const n = writeGif(path.join(FIX, name), {
    width: W,
    height: H,
    palette: PALETTE,
    frames: [
      { left: 0, top: 0, width: W, height: H, indices: solid(W, H, 1), delayCs: 0, disposal: 1 },
      { left: 0, top: 0, width: W, height: H, indices: solid(W, H, 2), delayCs: 0, disposal: 1 },
    ],
  });
  built.push([name, n, 'no delay -> as fast as possible']);
}

for (const [name, size, why] of built) {
  console.log(`  wrote fixtures/${name}  (${size} bytes) -- ${why}`);
}
console.log(`${built.length} GIF fixtures written.`);

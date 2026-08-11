// Minimal PNG decoder, used only by scripts/build-fixtures.js to read
// fixtures/test-quadrants.png back into RGBA pixels.
//
// Why this exists instead of an npm dependency: this repo has no
// package.json and no node_modules -- bin/ci runs the scripts with a bare
// `node`, and adding a dependency tree just to read one 362-byte test image
// would be a much bigger change than the decoder itself. The scope is
// deliberately tiny: 8-bit, non-interlaced, colour type 2 (RGB) or 6 (RGBA),
// which is exactly what the committed test image is. Anything else throws
// rather than silently guessing.
//
// The point of decoding the image here at all is anti-circularity: the
// fixture is regenerated from the SOURCE IMAGE through the documented
// protocol model, so scripts/check-raw-consistency.js compares the model
// against the real hardware capture in fixtures/raw/, rather than comparing
// a file to a copy of itself.

const fs = require('fs');
const zlib = require('zlib');

const PNG_SIGNATURE = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

function paethPredictor(a, b, c) {
  const p = a + b - c;
  const pa = Math.abs(p - a);
  const pb = Math.abs(p - b);
  const pc = Math.abs(p - c);
  if (pa <= pb && pa <= pc) return a;
  if (pb <= pc) return b;
  return c;
}

/**
 * Decode a PNG file into { width, height, rgba } where rgba is a
 * Uint8Array of width*height*4 bytes.
 */
function decodePng(filePath) {
  const buf = fs.readFileSync(filePath);
  if (!buf.subarray(0, 8).equals(PNG_SIGNATURE)) {
    throw new Error(`${filePath}: not a PNG (bad signature)`);
  }

  let width = 0;
  let height = 0;
  let bitDepth = 0;
  let colourType = 0;
  let interlace = 0;
  const idat = [];

  let offset = 8;
  while (offset < buf.length) {
    const length = buf.readUInt32BE(offset);
    const type = buf.toString('ascii', offset + 4, offset + 8);
    const data = buf.subarray(offset + 8, offset + 8 + length);
    if (type === 'IHDR') {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      bitDepth = data[8];
      colourType = data[9];
      interlace = data[12];
    } else if (type === 'IDAT') {
      idat.push(data);
    } else if (type === 'IEND') {
      break;
    }
    offset += 12 + length; // length + type + data + CRC
  }

  if (bitDepth !== 8) throw new Error(`${filePath}: bit depth ${bitDepth} unsupported (need 8)`);
  if (interlace !== 0) throw new Error(`${filePath}: interlaced PNGs unsupported`);
  if (colourType !== 2 && colourType !== 6) {
    throw new Error(`${filePath}: colour type ${colourType} unsupported (need 2=RGB or 6=RGBA)`);
  }

  const channels = colourType === 6 ? 4 : 3;
  const raw = zlib.inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  if (raw.length !== (stride + 1) * height) {
    throw new Error(`${filePath}: inflated ${raw.length} bytes, expected ${(stride + 1) * height}`);
  }

  // Undo the per-scanline filter. `prev` is the already-unfiltered previous
  // scanline, which filter types 2-4 refer back to.
  const out = new Uint8Array(width * height * 4);
  let prev = new Uint8Array(stride);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = new Uint8Array(raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1)));
    for (let i = 0; i < stride; i++) {
      const a = i >= channels ? line[i - channels] : 0; // byte to the left
      const b = prev[i]; // byte above
      const c = i >= channels ? prev[i - channels] : 0; // byte above-left
      switch (filter) {
        case 0: break;
        case 1: line[i] = (line[i] + a) & 0xff; break;
        case 2: line[i] = (line[i] + b) & 0xff; break;
        case 3: line[i] = (line[i] + ((a + b) >> 1)) & 0xff; break;
        case 4: line[i] = (line[i] + paethPredictor(a, b, c)) & 0xff; break;
        default: throw new Error(`${filePath}: unknown filter type ${filter} on row ${y}`);
      }
    }
    for (let x = 0; x < width; x++) {
      const src = x * channels;
      const dst = (y * width + x) * 4;
      out[dst] = line[src];
      out[dst + 1] = line[src + 1];
      out[dst + 2] = line[src + 2];
      out[dst + 3] = channels === 4 ? line[src + 3] : 0xff;
    }
    prev = line;
  }

  return { width, height, rgba: out };
}

module.exports = { decodePng };

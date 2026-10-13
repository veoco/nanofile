import zlib from "node:zlib";

/**
 * Deterministic image fixtures for the e2e suite.
 *
 * BMP on purpose: the format is a 54-byte header followed by raw BGR rows, so a
 * fixture needs neither zlib nor a CRC table and the suite stays free of image
 * tooling. The server decodes it through the same pipeline as a photo — a
 * confined child process with a memory and CPU bound (`sandbox::jobs::images`,
 * started by `sandbox::worker`) — so it exercises the real thumbnail path.
 *
 * PNG where the *browser* has to size the fixture: the preview's content
 * endpoint serves a BMP as `application/octet-stream` (see `files::mime_guess`),
 * which an `<img>` will not render, so the full-page preview tests need bytes
 * with a `image/png` content type. Deflating raw scanlines is all the encoder
 * `pngFixture` needs — no image tooling, no dependency.
 */
export function bmpFixture(width: number, height: number): Buffer {
  const clamp = (v: number) => (v < 0 ? 0 : v > 255 ? 255 : v);
  // Rows are padded to a 4-byte boundary, bottom-up, three bytes per pixel.
  const rowSize = Math.ceil((width * 3) / 4) * 4;
  const pixels = Buffer.alloc(rowSize * height);
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      // A dithered gradient: enough entropy that a PNG of this fixture is large
      // and a JPEG of it is not, which is what the thumbnail assertions in
      // `views.spec.ts` rely on.
      const dither = ((x * 7 + y * 13) % 17) - 8;
      const r = clamp(Math.round((x / width) * 200) + dither);
      const g = clamp(Math.round((y / height) * 200) + dither);
      const b = clamp(128 + dither);
      const o = (height - 1 - y) * rowSize + x * 3;
      pixels[o] = b;
      pixels[o + 1] = g;
      pixels[o + 2] = r;
    }
  }

  const header = Buffer.alloc(54);
  header.write("BM", 0, "ascii");
  header.writeUInt32LE(54 + pixels.length, 2); // file size
  header.writeUInt32LE(54, 10); // pixel data offset
  header.writeUInt32LE(40, 14); // BITMAPINFOHEADER
  header.writeInt32LE(width, 18);
  header.writeInt32LE(height, 22); // positive = bottom-up
  header.writeUInt16LE(1, 26); // planes
  header.writeUInt16LE(24, 28); // bits per pixel
  header.writeUInt32LE(0, 30); // BI_RGB (uncompressed)
  header.writeUInt32LE(pixels.length, 34);
  header.writeInt32LE(2835, 38); // ~72 dpi
  header.writeInt32LE(2835, 42);
  return Buffer.concat([header, pixels]);
}

/** PNG's CRC-32 (IEEE 802.3), table built once on first use. */
const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(buf: Buffer): number {
  let c = 0xffffffff;
  for (const byte of buf) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

/** A truecolour PNG of `width` x `height`, with a per-pixel gradient. */
export function pngFixture(width: number, height: number): Buffer {
  // Raw scanlines: a filter byte per row, then three bytes per pixel.
  const raw = Buffer.alloc((width * 3 + 1) * height);
  let o = 0;
  for (let y = 0; y < height; y++) {
    raw[o++] = 0; // filter: None
    for (let x = 0; x < width; x++) {
      raw[o++] = Math.round((x / width) * 255);
      raw[o++] = Math.round((y / height) * 255);
      raw[o++] = 128;
    }
  }

  const chunk = (type: string, data: Buffer): Buffer => {
    const out = Buffer.alloc(data.length + 12);
    out.writeUInt32BE(data.length, 0);
    out.write(type, 4, "ascii");
    data.copy(out, 8);
    out.writeUInt32BE(crc32(out.subarray(4, 8 + data.length)), 8 + data.length);
    return out;
  };

  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 2; // colour type: truecolour RGB
  ihdr[10] = 0; // compression: deflate
  ihdr[11] = 0; // filtering: adaptive
  ihdr[12] = 0; // interlace: none

  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", zlib.deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

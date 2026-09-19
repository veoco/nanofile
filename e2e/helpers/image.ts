/**
 * Deterministic image fixtures for the e2e suite.
 *
 * BMP on purpose: the format is a 54-byte header followed by raw BGR rows, so a
 * fixture needs neither zlib nor a CRC table and the suite stays free of image
 * tooling. The server decodes it through the same in-process pipeline as a
 * photo, so it exercises the real thumbnail path.
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

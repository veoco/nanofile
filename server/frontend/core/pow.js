// Proof-of-work helpers shared by the browser entry and the Node tests.
//
// The challenge is `SHA-256(seed || nonce_u64_be)` with at least `bits` leading
// zero bits. The solver is a synchronous SHA-256 over a single 64-byte block:
// Web Crypto's `subtle.digest` is async per hash, which makes a browser loop over
// hundreds of thousands of nonces far slower than this compact implementation.

const K = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]);

const INITIAL = new Uint32Array([
  0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
]);

function rotr(value, bits) {
  return (value >>> bits) | (value << (32 - bits));
}

function compress(state, words, block) {
  for (let i = 0; i < 16; i += 1) {
    words[i] = block.getUint32(i * 4);
  }
  for (let i = 16; i < 64; i += 1) {
    const s0 = rotr(words[i - 15], 7) ^ rotr(words[i - 15], 18) ^ (words[i - 15] >>> 3);
    const s1 = rotr(words[i - 2], 17) ^ rotr(words[i - 2], 19) ^ (words[i - 2] >>> 10);
    words[i] = (words[i - 16] + s0 + words[i - 7] + s1) | 0;
  }

  let a = state[0];
  let b = state[1];
  let c = state[2];
  let d = state[3];
  let e = state[4];
  let f = state[5];
  let g = state[6];
  let h = state[7];

  for (let i = 0; i < 64; i += 1) {
    const s1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
    const ch = (e & f) ^ (~e & g);
    const t1 = (h + s1 + ch + K[i] + words[i]) | 0;
    const s0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
    const maj = (a & b) ^ (a & c) ^ (b & c);
    const t2 = (s0 + maj) | 0;

    h = g;
    g = f;
    f = e;
    e = (d + t1) | 0;
    d = c;
    c = b;
    b = a;
    a = (t1 + t2) | 0;
  }

  state[0] = (state[0] + a) | 0;
  state[1] = (state[1] + b) | 0;
  state[2] = (state[2] + c) | 0;
  state[3] = (state[3] + d) | 0;
  state[4] = (state[4] + e) | 0;
  state[5] = (state[5] + f) | 0;
  state[6] = (state[6] + g) | 0;
  state[7] = (state[7] + h) | 0;
}

/** Decode a hex string into bytes; throws on an odd length or a non-hex digit. */
export function hexToBytes(hex) {
  if (typeof hex !== "string" || hex.length % 2 !== 0 || !/^[0-9a-fA-F]*$/.test(hex)) {
    throw new Error("invalid hex");
  }
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i += 1) {
    out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return out;
}

function pad(bytes) {
  const paddedLength = (((bytes.length + 8) >> 6) + 1) * 64;
  const padded = new Uint8Array(paddedLength);
  padded.set(bytes);
  padded[bytes.length] = 0x80;
  const view = new DataView(padded.buffer);
  const bitLength = bytes.length * 8;
  view.setUint32(paddedLength - 8, Math.floor(bitLength / 0x100000000));
  view.setUint32(paddedLength - 4, bitLength >>> 0);
  return padded;
}

/** Hash arbitrary bytes. Kept generic for the Node vectors; the solver below
 * uses the one-block fast path. */
export function sha256Bytes(bytes) {
  const padded = pad(bytes);
  const state = new Uint32Array(INITIAL);
  const words = new Uint32Array(64);
  for (let offset = 0; offset < padded.length; offset += 64) {
    compress(state, words, new DataView(padded.buffer, offset, 64));
  }
  const out = new Uint8Array(32);
  const view = new DataView(out.buffer);
  for (let i = 0; i < 8; i += 1) {
    view.setUint32(i * 4, state[i]);
  }
  return out;
}

/** Leading zero bits in a byte array, counting bits rather than bytes. */
export function leadingZeroBits(bytes) {
  let bits = 0;
  for (let i = 0; i < bytes.length; i += 1) {
    const value = bytes[i];
    if (value === 0) {
      bits += 8;
      continue;
    }
    bits += Math.clz32(value) - 24;
    break;
  }
  return bits;
}

function leadingZeroBitsInState(state) {
  let bits = 0;
  for (let i = 0; i < 8; i += 1) {
    if (state[i] === 0) {
      bits += 32;
      continue;
    }
    bits += Math.clz32(state[i]);
    break;
  }
  return bits;
}

function fixedBlock(seed) {
  // 24 bytes of message + 0x80 + 192-bit length fit in one 64-byte block.
  const padded = new Uint8Array(64);
  padded.set(seed, 0);
  padded[24] = 0x80;
  const view = new DataView(padded.buffer);
  view.setUint32(56, 0);
  view.setUint32(60, 192);
  return { padded, view };
}

function hashFixed24(seed, nonce) {
  const { padded, view } = fixedBlock(seed);
  view.setUint32(16, 0);
  view.setUint32(20, nonce >>> 0);
  const state = new Uint32Array(INITIAL);
  compress(state, new Uint32Array(64), new DataView(padded.buffer));
  return state;
}

/** Synchronous search, used by the tests and as the chunk implementation. */
export function findNonce(seed, bits) {
  const { padded, view } = fixedBlock(seed);
  const block = new DataView(padded.buffer);
  const words = new Uint32Array(64);
  const state = new Uint32Array(8);
  for (let nonce = 0; nonce <= 0xffffffff; nonce += 1) {
    view.setUint32(16, 0);
    view.setUint32(20, nonce >>> 0);
    state.set(INITIAL);
    compress(state, words, block);
    if (leadingZeroBitsInState(state) >= bits) {
      return nonce;
    }
  }
  return null;
}

/**
 * Asynchronous solver: searches in time-bounded chunks so the page can paint and
 * the status line can update. Resolves with the nonce, or `null` on exhaustion.
 */
export async function solvePow(seed, bits, onProgress) {
  if (!(seed instanceof Uint8Array) || seed.length !== 16) {
    throw new Error("seed must be 16 bytes");
  }
  const { padded, view } = fixedBlock(seed);
  const block = new DataView(padded.buffer);
  const words = new Uint32Array(64);
  const state = new Uint32Array(8);
  const clock = typeof performance !== "undefined" ? () => performance.now() : () => Date.now();
  const chunkMs = 10;
  let nonce = 0;
  let hashes = 0;

  while (nonce <= 0xffffffff) {
    const started = clock();
    do {
      view.setUint32(16, 0);
      view.setUint32(20, nonce >>> 0);
      state.set(INITIAL);
      compress(state, words, block);
      if (leadingZeroBitsInState(state) >= bits) {
        return nonce;
      }
      nonce += 1;
      hashes += 1;
    } while (nonce <= 0xffffffff && clock() - started < chunkMs);

    if (onProgress) onProgress(hashes);
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
  return null;
}

/** Test helper: verify a nonce exactly as the Rust side does. */
export function verifyNonce(seed, nonce, bits) {
  const state = hashFixed24(seed, nonce);
  return leadingZeroBitsInState(state) >= bits;
}

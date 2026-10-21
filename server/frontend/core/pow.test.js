import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  findNonce,
  hexToBytes,
  leadingZeroBits,
  sha256Bytes,
  solvePow,
  verifyNonce,
} from "./pow.js";

function utf8(value) {
  return new TextEncoder().encode(value);
}

function hex(bytes) {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

function seed() {
  return new Uint8Array([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
}

test("sha256Bytes matches the standard vectors", () => {
  assert.equal(
    hex(sha256Bytes(utf8(""))),
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
  );
  assert.equal(
    hex(sha256Bytes(utf8("abc"))),
    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
  );
  const long = "a".repeat(1000);
  assert.equal(hex(sha256Bytes(utf8(long))), createHash("sha256").update(long).digest("hex"));
});

test("leadingZeroBits counts bits, not bytes", () => {
  assert.equal(leadingZeroBits(new Uint8Array([0, 0x0f])), 12);
  assert.equal(leadingZeroBits(new Uint8Array([0x7f])), 1);
  assert.equal(leadingZeroBits(new Uint8Array([0x80])), 0);
  assert.equal(leadingZeroBits(new Uint8Array([0, 0])), 16);
});

test("hexToBytes rejects malformed input", () => {
  assert.deepEqual(Array.from(hexToBytes("00ff10")), [0, 255, 16]);
  assert.throws(() => hexToBytes("abc"), /invalid hex/);
  assert.throws(() => hexToBytes("zz"), /invalid hex/);
  assert.throws(() => hexToBytes(null), /invalid hex/);
});

test("findNonce returns a nonce that verifies", () => {
  const s = seed();
  for (const bits of [8, 12]) {
    const nonce = findNonce(s, bits);
    assert.notEqual(nonce, null);
    assert.ok(verifyNonce(s, nonce, bits), `bits=${bits} nonce=${nonce}`);
  }
});

test("solvePow agrees with findNonce", async () => {
  const s = seed();
  const nonce = await solvePow(s, 8);
  assert.notEqual(nonce, null);
  assert.ok(verifyNonce(s, nonce, 8));
});

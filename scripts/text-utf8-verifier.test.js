"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const { verifyTextBuffer } = require("./text-utf8-verify.js");

test("accepts strict UTF-8 text", () => {
  assert.doesNotThrow(() =>
    verifyTextBuffer(Buffer.from("Roze 上下文契约\n", "utf8"), "valid.md"),
  );
});

test("rejects malformed UTF-8 bytes", () => {
  assert.throws(
    () => verifyTextBuffer(Buffer.from([0x52, 0x6f, 0x7a, 0x65, 0xef, 0xbc, 0x3f]), "bad.md"),
    /invalid UTF-8/,
  );
});

test("rejects an encoded replacement character", () => {
  assert.throws(
    () => verifyTextBuffer(Buffer.from("broken \uFFFD text", "utf8"), "replacement.md"),
    /U\+FFFD/,
  );
});

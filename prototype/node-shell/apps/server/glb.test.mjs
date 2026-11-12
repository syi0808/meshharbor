import test from "node:test";
import assert from "node:assert/strict";
import { detectAsset, inspectGlb, safeFilename } from "./glb.mjs";
import { createGridGlb } from "./fixtures.test-helper.mjs";

test("inspects a self-contained triangle GLB and reports simplification support", () => {
  const file = createGridGlb(4);
  const info = inspectGlb(file);
  assert.equal(info.format, "GLB");
  assert.equal(info.version, "2.0");
  assert.equal(info.triangles, 32);
  assert.equal(info.vertices, 25);
  assert.equal(info.materials, 1);
  assert.deepEqual(info.support, { supported: true, reason: null });
  assert.equal(detectAsset(file, "turnip.glb").kind, "mesh");
});

test("accepts PNG/JPEG only when their bytes and suffix agree", () => {
  const png = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10, 0]);
  const jpg = Buffer.from([255, 216, 255, 0]);
  assert.equal(detectAsset(png, "input.png").mimeType, "image/png");
  assert.equal(detectAsset(jpg, "input.jpeg").mimeType, "image/jpeg");
  assert.throws(() => detectAsset(png, "input.jpg"), { code: "EXTENSION_MISMATCH" });
});

test("rejects malformed GLB lengths and unsafe or unsupported uploads", () => {
  const file = createGridGlb(1);
  const invalid = Buffer.from(file);
  invalid.writeUInt32LE(file.length - 4, 8);
  assert.throws(() => inspectGlb(invalid), { code: "INVALID_GLB" });
  assert.throws(() => detectAsset(Buffer.from("#!/bin/sh"), "run.sh"), { code: "UNSUPPORTED_FORMAT" });
  assert.equal(safeFilename("../../bad.glb"), "bad.glb");
  assert.equal(detectAsset(createGridGlb(1), "../../bad.glb").name, "bad.glb");
});

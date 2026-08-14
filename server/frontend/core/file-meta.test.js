import { test } from "node:test";
import assert from "node:assert/strict";
import { humanType, isQuickPreviewImage, getExifFields } from "./file-meta.js";

test("humanType maps dir/file/ext, falling back to key when untranslated", () => {
  globalThis.window = { __T: {} };
  assert.equal(humanType("dir", ""), "ft.folder");
  assert.equal(humanType("file", ""), "ft.file");
  assert.equal(humanType("file", "PDF"), "ft.pdf_document");
  assert.equal(humanType("file", "XYZ"), "XYZ File");
});

test("isQuickPreviewImage detects image extensions case-insensitively", () => {
  assert.equal(isQuickPreviewImage("photo.png"), true);
  assert.equal(isQuickPreviewImage("photo.PNG"), true);
  assert.equal(isQuickPreviewImage("a.b.jpeg"), true);
  assert.equal(isQuickPreviewImage("notes.txt"), false);
  assert.equal(isQuickPreviewImage("noext"), false);
});

test("getExifFields formats known fields in order and skips missing", () => {
  globalThis.window = { __T: {} };
  var fields = getExifFields({
    Model: "X100",
    FNumber: "2.8",
    ISOSpeed: "400",
    Flash: "1",
    Orientation: "6",
  });
  assert.deepEqual(fields[0], { label: "exif.model", value: "X100" });
  assert.deepEqual(fields[1], { label: "exif.aperture", value: "2.8" });
  assert.deepEqual(fields[2], { label: "exif.iso", value: "400" });
  assert.deepEqual(fields[3], { label: "exif.flash", value: "common.yes" });
  assert.deepEqual(fields[4], { label: "exif.orientation", value: "exif.orientation_90_cw" });
});

test("getExifFields Flash=0 means no", () => {
  globalThis.window = { __T: {} };
  var fields = getExifFields({ Flash: "0" });
  assert.deepEqual(fields[0], { label: "exif.flash", value: "common.no" });
});

test("getExifFields skips null/undefined values", () => {
  globalThis.window = { __T: {} };
  var fields = getExifFields({ Make: null, Model: undefined, ISOSpeed: "200" });
  assert.equal(fields.length, 1);
  assert.deepEqual(fields[0], { label: "exif.iso", value: "200" });
});

test("getExifFields formats px and quoted fields", () => {
  globalThis.window = { __T: {} };
  var fields = getExifFields({
    ExposureTime: "1/125",
    FocalLength: "50",
    GPSLatitude: "31.23",
    PixelXDimension: "4000",
    PixelYDimension: "3000",
  });
  assert.deepEqual(fields[0], { label: "exif.exposure", value: "1/125" });
  assert.deepEqual(fields[1], { label: "exif.focal_length", value: "50" });
  assert.deepEqual(fields[2], { label: "exif.gps_latitude", value: "31.23" });
  assert.deepEqual(fields[3], { label: "exif.width", value: "4000 px" });
  assert.deepEqual(fields[4], { label: "exif.height", value: "3000 px" });
});

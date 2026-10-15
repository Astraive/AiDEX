import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { overflowEdges } from "./useOverflowEdges";

describe("overflowEdges", () => {
  test("content that fits hides nothing", () => {
    assert.deepEqual(overflowEdges(0, 300, 300), { start: false, end: false });
  });

  test("at the start only the end is cut off", () => {
    assert.deepEqual(overflowEdges(0, 300, 600), { start: false, end: true });
  });

  test("in the middle both edges are cut off", () => {
    assert.deepEqual(overflowEdges(150, 300, 600), { start: true, end: true });
  });

  test("at the end only the start is cut off", () => {
    assert.deepEqual(overflowEdges(300, 300, 600), { start: true, end: false });
  });

  test("a pixel of slack absorbs fractional sizes", () => {
    assert.deepEqual(overflowEdges(0.5, 300, 300.8), { start: false, end: false });
    assert.deepEqual(overflowEdges(299.5, 300, 600), { start: true, end: false });
  });

  test("right-to-left offsets count by magnitude", () => {
    assert.deepEqual(overflowEdges(-150, 300, 600), { start: true, end: true });
    assert.deepEqual(overflowEdges(-300, 300, 600), { start: true, end: false });
  });
});

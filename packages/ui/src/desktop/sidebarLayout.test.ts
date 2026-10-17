import assert from "node:assert/strict";
import { test } from "node:test";
import {
  clampSidebarWidth,
  dockMinViewport,
  resizeStep,
  SHELL_CHROME,
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_MIN_WIDTH,
} from "./sidebarLayout";

test("the shell chrome is the rail plus two pads and two gaps", () => {
  assert.equal(SHELL_CHROME, 52 + 32);
});

test("a default sidebar docks from 980px", () => {
  assert.equal(dockMinViewport(SIDEBAR_DEFAULT_WIDTH), 980);
  assert.equal(dockMinViewport(208), 932);
  assert.equal(dockMinViewport(480), 1204);
});

test("the docked width keeps main at 640px and stays within 208-480", () => {
  assert.equal(clampSidebarWidth(500, 1440), 480);
  assert.equal(clampSidebarWidth(300, 1000), 276);
  assert.equal(clampSidebarWidth(300, 900), 208);
  assert.equal(clampSidebarWidth(100, 1440), 208);
  assert.equal(clampSidebarWidth(256, 1440), 256);
  assert.equal(clampSidebarWidth(256.6, 1440), 257);
  assert.equal(clampSidebarWidth(Number.NaN, 1440), SIDEBAR_DEFAULT_WIDTH);
});

test("separator keys step by 8, or 32 with Shift", () => {
  assert.equal(resizeStep("ArrowRight", true, 256), 288);
  assert.equal(resizeStep("ArrowRight", false, 256), 264);
  assert.equal(resizeStep("ArrowLeft", false, 256), 248);
  assert.equal(resizeStep("ArrowLeft", true, 256), 224);
  assert.equal(resizeStep("ArrowUp", false, 256), 264);
  assert.equal(resizeStep("ArrowDown", false, 256), 248);
});

test("Home and End go to the ends of the range", () => {
  assert.equal(resizeStep("Home", false, 256), SIDEBAR_MIN_WIDTH);
  assert.equal(resizeStep("End", false, 256), SIDEBAR_MAX_WIDTH);
  assert.equal(resizeStep("End", false, 256, 300), 300);
});

test("steps never leave the range or pass the current maximum", () => {
  assert.equal(resizeStep("ArrowLeft", true, 216), SIDEBAR_MIN_WIDTH);
  assert.equal(resizeStep("ArrowRight", true, 470), SIDEBAR_MAX_WIDTH);
  assert.equal(resizeStep("ArrowRight", false, 296, 300), 300);
  assert.equal(resizeStep("End", false, 256, 100), SIDEBAR_MIN_WIDTH);
});

test("other keys do nothing", () => {
  assert.equal(resizeStep("Enter", false, 256), null);
  assert.equal(resizeStep("a", true, 256), null);
});

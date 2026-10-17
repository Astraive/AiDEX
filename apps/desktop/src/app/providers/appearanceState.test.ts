import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { APPEARANCE_DEFAULTS } from "@aidex/client/dto/preferences";
import {
  isDefaultAppearance,
  legacyAppearance,
  nextScaleStep,
  normalizeAppearance,
  toAppearanceTokens,
  zoomKeyAction,
  zoomScale,
  type ZoomKeyEvent,
} from "./appearanceState";

const key = (overrides: Partial<ZoomKeyEvent>): ZoomKeyEvent => ({
  key: "",
  code: "",
  ctrlKey: false,
  metaKey: false,
  shiftKey: false,
  altKey: false,
  ...overrides,
});

describe("normalizeAppearance", () => {
  it("keeps a valid record as it is", () => {
    const saved = {
      ui_scale: 110,
      density: "spacious",
      chat_width: "wide",
      chat_font_size: 16,
      code_font_size: 12,
      sidebar_width: 300,
      sidebar_auto_collapse: false,
      message_spacing: "relaxed",
    } as const;
    assert.deepEqual(normalizeAppearance(saved, "comfortable"), saved);
  });

  it("clamps numbers to their ranges", () => {
    const appearance = normalizeAppearance(
      { ...APPEARANCE_DEFAULTS, ui_scale: 200, chat_font_size: 9, code_font_size: 40, sidebar_width: 9999 },
      "comfortable",
    );
    assert.equal(appearance.ui_scale, 125);
    assert.equal(appearance.chat_font_size, 12);
    assert.equal(appearance.code_font_size, 18);
    assert.equal(appearance.sidebar_width, 480);
    assert.equal(normalizeAppearance({ ui_scale: 10, sidebar_width: 1 }).ui_scale, 75);
    assert.equal(normalizeAppearance({ sidebar_width: 1 }).sidebar_width, 208);
    assert.equal(normalizeAppearance({ chat_font_size: 13.6 }).chat_font_size, 14);
  });

  it("defaults unknown enum values and wrong types", () => {
    const appearance = normalizeAppearance(
      {
        ui_scale: "big",
        density: "dense",
        chat_width: "xwide",
        chat_font_size: null,
        code_font_size: Number.NaN,
        sidebar_width: "300",
        sidebar_auto_collapse: "yes",
        message_spacing: 3,
        future_field: true,
      },
      "comfortable",
    );
    assert.deepEqual(appearance, APPEARANCE_DEFAULTS);
  });

  it("falls back to the legacy density when the method is missing", () => {
    assert.deepEqual(normalizeAppearance(undefined, "compact"), { ...APPEARANCE_DEFAULTS, density: "compact" });
    assert.deepEqual(normalizeAppearance(null, "comfortable"), APPEARANCE_DEFAULTS);
    assert.deepEqual(normalizeAppearance(null), APPEARANCE_DEFAULTS);
    assert.deepEqual(normalizeAppearance("garbage", "compact"), { ...APPEARANCE_DEFAULTS, density: "compact" });
  });

  it("uses the legacy density for an unreadable density only", () => {
    assert.equal(normalizeAppearance({ density: "dense" }, "compact").density, "compact");
    assert.equal(normalizeAppearance({ density: "spacious" }, "compact").density, "spacious");
  });
});

describe("legacyAppearance", () => {
  it("never yields spacious and ignores unknown legacy values", () => {
    assert.equal(legacyAppearance("compact").density, "compact");
    assert.equal(legacyAppearance("spacious" as never).density, "comfortable");
    assert.equal(legacyAppearance(undefined).density, "comfortable");
  });
});

describe("nextScaleStep", () => {
  it("steps through 75 / 80 / 90 / 100 / 110 / 125", () => {
    assert.equal(nextScaleStep(100, "in"), 110);
    assert.equal(nextScaleStep(110, "in"), 125);
    assert.equal(nextScaleStep(125, "out"), 110);
    assert.equal(nextScaleStep(100, "out"), 90);
    assert.equal(nextScaleStep(80, "out"), 75);
  });

  it("stays at the ends", () => {
    assert.equal(nextScaleStep(125, "in"), 125);
    assert.equal(nextScaleStep(75, "out"), 75);
  });

  it("moves an off-step value to the neighbouring step", () => {
    assert.equal(nextScaleStep(105, "in"), 110);
    assert.equal(nextScaleStep(105, "out"), 100);
  });
});

describe("zoomScale", () => {
  it("goes 100 → 110 → 125 → 110 → 100, and reset returns to 100", () => {
    let scale = 100;
    const seen = [scale];
    for (const action of ["in", "in", "out", "out"] as const) {
      scale = zoomScale(scale, action);
      seen.push(scale);
    }
    assert.deepEqual(seen, [100, 110, 125, 110, 100]);
    assert.equal(zoomScale(125, "reset"), 100);
  });
});

describe("toAppearanceTokens", () => {
  it("maps the record to the @aidex/ui tokens", () => {
    assert.deepEqual(toAppearanceTokens({ ...APPEARANCE_DEFAULTS, sidebar_width: 320, chat_width: "full" }), {
      density: "comfortable",
      chatWidth: "full",
      messageSpacing: "comfortable",
      chatFontSize: 14,
      codeFontSize: 13,
      sidebarWidth: 320,
    });
  });
});

describe("isDefaultAppearance", () => {
  it("is true only when every field is the default", () => {
    assert.equal(isDefaultAppearance(APPEARANCE_DEFAULTS), true);
    assert.equal(isDefaultAppearance({ ...APPEARANCE_DEFAULTS, sidebar_width: 256 }), false);
    assert.equal(isDefaultAppearance({ ...APPEARANCE_DEFAULTS, density: "compact" }), false);
  });
});

describe("zoomKeyAction", () => {
  it("matches = / + / - / _ / 0 with Ctrl", () => {
    assert.equal(zoomKeyAction(key({ key: "=", code: "Equal", ctrlKey: true }), false), "in");
    assert.equal(zoomKeyAction(key({ key: "+", code: "Equal", ctrlKey: true, shiftKey: true }), false), "in");
    assert.equal(zoomKeyAction(key({ key: "-", code: "Minus", ctrlKey: true }), false), "out");
    assert.equal(zoomKeyAction(key({ key: "_", code: "Minus", ctrlKey: true, shiftKey: true }), false), "out");
    assert.equal(zoomKeyAction(key({ key: "0", code: "Digit0", ctrlKey: true }), false), "reset");
  });

  it("zooms in with + on a German layout", () => {
    assert.equal(zoomKeyAction(key({ key: "+", code: "BracketRight", ctrlKey: true }), false), "in");
  });

  it("matches the numpad", () => {
    assert.equal(zoomKeyAction(key({ key: "+", code: "NumpadAdd", ctrlKey: true }), false), "in");
    assert.equal(zoomKeyAction(key({ key: "-", code: "NumpadSubtract", ctrlKey: true }), false), "out");
    assert.equal(zoomKeyAction(key({ key: "0", code: "Numpad0", ctrlKey: true }), false), "reset");
    // Num Lock off: Ctrl+Insert is copy, not a zoom reset.
    assert.equal(zoomKeyAction(key({ key: "Insert", code: "Numpad0", ctrlKey: true }), false), null);
  });

  it("uses ⌘ on Apple platforms", () => {
    assert.equal(zoomKeyAction(key({ key: "=", code: "Equal", metaKey: true }), true), "in");
    assert.equal(zoomKeyAction(key({ key: "=", code: "Equal", ctrlKey: true }), true), null);
    assert.equal(zoomKeyAction(key({ key: "=", code: "Equal", metaKey: true }), false), null);
  });

  it("ignores Alt, missing Mod, other keys and IME composition", () => {
    assert.equal(zoomKeyAction(key({ key: "=", code: "Equal", ctrlKey: true, altKey: true }), false), null);
    assert.equal(zoomKeyAction(key({ key: "0", code: "Digit0", ctrlKey: true, altKey: true }), false), null);
    assert.equal(zoomKeyAction(key({ key: "=", code: "Equal" }), false), null);
    assert.equal(zoomKeyAction(key({ key: "=", code: "Equal", ctrlKey: true, metaKey: true }), false), null);
    assert.equal(zoomKeyAction(key({ key: "b", code: "KeyB", ctrlKey: true }), false), null);
    assert.equal(zoomKeyAction(key({ key: "=", code: "Equal", ctrlKey: true, isComposing: true }), false), null);
  });
});

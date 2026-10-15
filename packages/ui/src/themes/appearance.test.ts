import { describe, test } from "node:test";
import assert from "node:assert/strict";
import {
  DEFAULT_APPEARANCE_TOKENS,
  appearanceAttributes,
  appearanceVariables,
  applyAppearance,
  type AppearanceRoot,
  type AppearanceTokens,
} from "./appearance";

type StubRoot = AppearanceRoot & { calls: string[]; properties: Map<string, string> };

/** A stand-in for `<html>`: bun has no DOM, and applyAppearance needs only dataset and two style methods. */
function stubRoot(dataset: Record<string, string> = {}): StubRoot {
  const calls: string[] = [];
  const properties = new Map<string, string>();
  return {
    calls,
    properties,
    dataset: { ...dataset },
    style: {
      setProperty(name: string, value: string | null) {
        calls.push(`set ${name}`);
        properties.set(name, value ?? "");
      },
      removeProperty(name: string) {
        calls.push(`remove ${name}`);
        const previous = properties.get(name) ?? "";
        properties.delete(name);
        return previous;
      },
    },
  };
}

function tokens(patch: Partial<AppearanceTokens> = {}): AppearanceTokens {
  return { ...DEFAULT_APPEARANCE_TOKENS, ...patch };
}

describe("appearanceVariables", () => {
  test("code size sets the code text and a rounded 1.54 line height", () => {
    const cases: [number, string, string][] = [
      [13, "13px", "20px"],
      [10, "10px", "15px"],
      [18, "18px", "28px"],
    ];
    for (const [size, text, line] of cases) {
      const vars = appearanceVariables(tokens({ codeFontSize: size }));
      assert.equal(vars["--aidex-text-code"], text, `code ${size}`);
      assert.equal(vars["--aidex-code-line-height"], line, `code ${size}`);
    }
  });

  test("chat size is written in px", () => {
    assert.equal(appearanceVariables(tokens())["--aidex-text-chat"], "14px");
    assert.equal(appearanceVariables(tokens({ chatFontSize: 20 }))["--aidex-text-chat"], "20px");
  });

  test("out-of-range and non-finite sizes are clamped or defaulted", () => {
    const vars = appearanceVariables(tokens({ chatFontSize: 9, codeFontSize: Number.NaN, sidebarWidth: 9999 }));
    assert.equal(vars["--aidex-text-chat"], "12px");
    assert.equal(vars["--aidex-text-code"], "13px");
    assert.equal(vars["--aidex-code-line-height"], "20px");
    assert.equal(vars["--aidex-sidebar-width"], "480px");
  });

  test("a null sidebar width removes the property", () => {
    assert.equal(appearanceVariables(tokens({ sidebarWidth: null }))["--aidex-sidebar-width"], null);
    assert.equal(appearanceVariables(tokens({ sidebarWidth: 320 }))["--aidex-sidebar-width"], "320px");
  });
});

describe("appearanceAttributes", () => {
  test("density is always present and the other attributes are absent at their default", () => {
    assert.deepEqual(appearanceAttributes(tokens()), { uiDensity: "comfortable", chatWidth: null, messageSpacing: null });
    assert.deepEqual(appearanceAttributes(tokens({ density: "compact", chatWidth: "full", messageSpacing: "relaxed" })), {
      uiDensity: "compact",
      chatWidth: "full",
      messageSpacing: "relaxed",
    });
  });
});

describe("applyAppearance", () => {
  test("full chat width sets the attribute and comfortable removes it", () => {
    const root = stubRoot();
    applyAppearance(tokens({ chatWidth: "full" }), root);
    assert.equal(root.dataset.chatWidth, "full");
    assert.equal(root.dataset.uiDensity, "comfortable");

    applyAppearance(tokens({ chatWidth: "comfortable" }), root);
    assert.equal("chatWidth" in root.dataset, false);
    assert.equal(root.dataset.uiDensity, "comfortable");
  });

  test("a null sidebar width removes the inline property", () => {
    const root = stubRoot();
    applyAppearance(tokens({ sidebarWidth: 300 }), root);
    assert.equal(root.properties.get("--aidex-sidebar-width"), "300px");

    applyAppearance(tokens({ sidebarWidth: null }), root);
    assert.equal(root.properties.has("--aidex-sidebar-width"), false);
    assert.ok(root.calls.includes("remove --aidex-sidebar-width"));
  });

  test("writes only its own properties and dataset keys, never the accent", () => {
    const root = stubRoot({ theme: "light", density: "compact", reduceMotion: "true", keyboardHints: "false" });
    root.properties.set("--aidex-color-accent", "#8b7bff");
    applyAppearance(tokens({ density: "spacious", messageSpacing: "compact", codeFontSize: 15, sidebarWidth: 240 }), root);

    assert.ok(root.calls.every((call) => !call.endsWith("--aidex-color-accent")), root.calls.join(", "));
    assert.equal(root.properties.get("--aidex-color-accent"), "#8b7bff");
    const touched = new Set(root.calls.map((call) => call.split(" ")[1]));
    assert.deepEqual([...touched].sort(), [
      "--aidex-code-line-height",
      "--aidex-sidebar-width",
      "--aidex-text-chat",
      "--aidex-text-code",
    ]);
    assert.deepEqual(root.dataset, {
      theme: "light",
      density: "compact",
      reduceMotion: "true",
      keyboardHints: "false",
      uiDensity: "spacious",
      messageSpacing: "compact",
    });
    assert.equal(root.properties.get("--aidex-text-code"), "15px");
    assert.equal(root.properties.get("--aidex-code-line-height"), "23px");
  });
});

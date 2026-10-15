import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { createSelectChangeEvent } from "./selectEvent";

describe("createSelectChangeEvent", () => {
  test("exposes the value and name on target and currentTarget", () => {
    const event = createSelectChangeEvent("stable", "channel");
    assert.equal(event.target.value, "stable");
    assert.equal(event.target.name, "channel");
    assert.equal(event.currentTarget.value, "stable");
    assert.equal(event.currentTarget.name, "channel");
    assert.equal(event.type, "change");
  });

  test("defaults the name to an empty string, as an unnamed select has", () => {
    const event = createSelectChangeEvent("dark");
    assert.equal(event.target.name, "");
    assert.equal(event.target.value, "dark");
  });

  test("keeps an empty value, e.g. a cleared choice", () => {
    assert.equal(createSelectChangeEvent("").target.value, "");
  });

  test("supports the handler methods React events have", () => {
    const event = createSelectChangeEvent("a");
    assert.equal(event.isDefaultPrevented(), false);
    event.preventDefault();
    assert.equal(event.isDefaultPrevented(), true);
    assert.equal(event.defaultPrevented, true);
    assert.equal(event.isPropagationStopped(), false);
    event.stopPropagation();
    assert.equal(event.isPropagationStopped(), true);
    event.persist();
  });

  test("works with handlers that destructure target", () => {
    const seen: string[] = [];
    const handler = ({ target: { value } }: { target: { value: string } }) => seen.push(value);
    handler(createSelectChangeEvent("x"));
    assert.deepEqual(seen, ["x"]);
  });
});

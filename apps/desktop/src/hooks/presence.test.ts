import { test } from "node:test";
import assert from "node:assert/strict";
import { AWAY_AFTER_MS, createPresenceTracker } from "./presence";

function fakeWindow() {
  const window = { now: 1_000, visible: true };
  const tracker = createPresenceTracker({ now: () => window.now, visible: () => window.visible });
  let returns = 0;
  tracker.onReturn(() => {
    returns += 1;
  });
  return { window, tracker, returns: () => returns };
}

test("polls run while the user is present and stop once the window had no input for the away period", () => {
  const { window, tracker } = fakeWindow();
  assert.equal(tracker.pollAllowed(), true);
  window.now += AWAY_AFTER_MS - 1;
  assert.equal(tracker.pollAllowed(), true);
  window.now += 1;
  assert.equal(tracker.present(), false);
  assert.equal(tracker.pollAllowed(), false, "an unattended window must not keep reading through the vault");
  tracker.input();
  assert.equal(tracker.pollAllowed(), true);
});

test("a hidden window never polls, even for work the daemon keeps the vault open for", () => {
  const { window, tracker } = fakeWindow();
  window.visible = false;
  tracker.visibilityChanged();
  assert.equal(tracker.pollAllowed(), false);
  assert.equal(tracker.pollAllowed({ whileAway: true }), false);
});

test("whileAway polls keep running in a visible window without input", () => {
  const { window, tracker } = fakeWindow();
  window.now += AWAY_AFTER_MS * 10;
  assert.equal(tracker.pollAllowed(), false);
  assert.equal(tracker.pollAllowed({ whileAway: true }), true);
});

test("coming back after being away, or showing the window again, tells the polls to catch up once", () => {
  const { window, tracker, returns } = fakeWindow();
  tracker.input();
  window.now += 1000;
  tracker.input();
  assert.equal(returns(), 0, "input while present is not a return");

  window.now += AWAY_AFTER_MS;
  tracker.input();
  assert.equal(returns(), 1);

  window.visible = false;
  tracker.visibilityChanged();
  assert.equal(returns(), 1, "hiding is not a return");
  window.now += 5;
  window.visible = true;
  tracker.visibilityChanged();
  assert.equal(returns(), 2);
  assert.equal(tracker.present(), true, "showing the window counts as input");
  tracker.visibilityChanged();
  assert.equal(returns(), 2, "a repeated visible notice is not another return");
});

test("a released return listener is not called again", () => {
  const { window } = fakeWindow();
  const tracker = createPresenceTracker({ now: () => window.now, visible: () => window.visible });
  let calls = 0;
  const stop = tracker.onReturn(() => {
    calls += 1;
  });
  window.now += AWAY_AFTER_MS;
  tracker.input();
  stop();
  window.now += AWAY_AFTER_MS;
  tracker.input();
  assert.equal(calls, 1);
});

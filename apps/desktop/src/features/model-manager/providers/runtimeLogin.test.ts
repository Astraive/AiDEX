import { test } from "node:test";
import assert from "node:assert/strict";
import { shouldOpenSignInPage } from "./runtimeLogin";

const pending = { state: "pending" as const, flow_id: "flow_1", authorization_url: "https://auth.openai.com/oauth/authorize" };

test("the sign-in page opens once per flow, as soon as the flow reports its address", () => {
  assert.equal(shouldOpenSignInPage(null, { ...pending, authorization_url: null }), false, "no address yet");
  assert.equal(shouldOpenSignInPage(null, pending), true);
  assert.equal(shouldOpenSignInPage("flow_1", pending), false, "already opened for this flow");
  assert.equal(shouldOpenSignInPage("flow_1", { ...pending, flow_id: "flow_2" }), true, "a new sign-in opens again");
  assert.equal(shouldOpenSignInPage(null, { ...pending, state: "connected" }), false, "a finished flow opens nothing");
});

import assert from "node:assert/strict";
import { test } from "node:test";
import { AidexError } from "@aidex/client/errors";
import {
  createRefetchScheduler,
  queryFailed,
  queryStarted,
  querySucceeded,
  type QueryState,
} from "./bridgeQueryState";

const locked = new AidexError({ code: "permission_denied", kind: "vault_locked", message: "vault is locked" });

test("refetches during one pending load produce exactly one follow-up load", () => {
  const scheduler = createRefetchScheduler();
  let loads = 0;
  const refetch = () => {
    if (scheduler.request()) loads += 1;
  };
  scheduler.start();
  loads += 1;
  for (let poll = 0; poll < 5; poll += 1) refetch();
  assert.equal(loads, 1, "no load starts while one is in flight");
  assert.equal(scheduler.finish(), true, "the queued refetches run once");
  scheduler.start();
  loads += 1;
  assert.equal(scheduler.finish(), false, "nothing more is queued");
  assert.equal(loads, 2);
});

test("an idle refetch loads at once and a settled load queues nothing", () => {
  const scheduler = createRefetchScheduler();
  assert.equal(scheduler.request(), true);
  scheduler.start();
  assert.equal(scheduler.finish(), false);
  assert.equal(scheduler.request(), true);
});

test("an abandoned load leaves no refetch stuck in the queue", () => {
  const scheduler = createRefetchScheduler();
  scheduler.start();
  assert.equal(scheduler.request(), false);
  // The query was disabled or its key changed mid-flight.
  scheduler.reset();
  assert.equal(scheduler.finish(), false, "the abandoned load does not trigger the queued refetch");
  assert.equal(scheduler.request(), true, "the next refetch loads immediately");
});

test("a refetch keeps data of the same key and a new key starts empty", () => {
  const loaded: QueryState<string> = querySucceeded("conversation-a", "messages");
  assert.equal(queryStarted(loaded, "conversation-a").data, "messages");
  assert.equal(queryStarted(loaded, "conversation-a").settled, false);
  assert.equal(queryStarted(loaded, "conversation-b").data, undefined);
});

test("a failing refetch clears data", () => {
  const loaded: QueryState<string> = querySucceeded("conversation-a", "decrypted messages");
  const refetching = queryStarted(loaded, "conversation-a");
  const failed = queryFailed<string>(refetching.key, locked);
  assert.equal(failed.data, undefined);
  assert.equal(failed.error?.kind, "vault_locked");
  assert.equal(failed.settled, true);
});

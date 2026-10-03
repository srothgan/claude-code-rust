import assert from "node:assert/strict";
import test from "node:test";
import type { Query } from "@anthropic-ai/claude-agent-sdk";
import { applyUltracode, readUltracodeState, ultracodeError, UltracodeVerificationError } from "./ultracode.js";

function queryWithSettings(applied: unknown): Query {
  return { getSettings: async () => ({ applied }) } as unknown as Query;
}

test("Ultracode runtime adapter detects undeclared getSettings capability", async () => {
  for (const query of [{}, { getSettings: true }]) {
    await assert.rejects(readUltracodeState(query as Query), /unavailable with the installed Agent SDK runtime/);
  }
});

test("Ultracode runtime adapter accepts exactly four verified combinations", async () => {
  for (const available of [false, true]) {
    for (const requested of [false, true]) {
      for (const effective of [false, true]) {
        const query = queryWithSettings({ ultracodeAvailable: available, ultracodeRequested: requested, ultracode: effective });
        if (effective === (available && requested)) {
          assert.deepEqual(await readUltracodeState(query), { available, requested, effective });
        } else {
          await assert.rejects(readUltracodeState(query), /Invalid Ultracode settings/);
        }
      }
    }
  }
});

test("Ultracode runtime adapter rejects missing and non-boolean settings", async () => {
  for (const field of ["ultracodeAvailable", "ultracodeRequested", "ultracode"]) {
    for (const value of [undefined, null, 0, "false", [], {}]) {
      const applied = { ultracodeAvailable: false, ultracodeRequested: false, ultracode: false, [field]: value };
      await assert.rejects(readUltracodeState(queryWithSettings(applied)), /Invalid Ultracode settings/);
    }
  }
  for (const settings of [undefined, null, [], {}, { applied: null }, { applied: [] }]) {
    await assert.rejects(readUltracodeState({ getSettings: async () => settings } as unknown as Query), /Invalid applied settings response/);
  }
});

test("Ultracode apply sends explicit booleans and verifies the resulting settings", async () => {
  for (const enabled of [true, false]) {
    const calls: unknown[] = [];
    const query = queryWithSettings({ ultracodeAvailable: true, ultracodeRequested: enabled, ultracode: enabled });
    query.applyFlagSettings = async (settings) => { calls.push(settings); };
    assert.deepEqual(await applyUltracode(query, enabled), { available: true, requested: enabled, effective: enabled });
    assert.deepEqual(calls, [{ ultracode: enabled }]);
  }
});

test("an accepted unavailable request remains requested rather than being reported as active", async () => {
  const query = queryWithSettings({ ultracodeAvailable: false, ultracodeRequested: true, ultracode: false });
  query.applyFlagSettings = async () => {};
  assert.deepEqual(await applyUltracode(query, true), { available: false, requested: true, effective: false });
});

test("accepted Ultracode changes fail verification on unreadable, invalid, or mismatched state", async () => {
  for (const enabled of [true, false]) {
    for (const query of [
      {} as Query,
      { getSettings: async () => { throw new Error("read failed"); } } as unknown as Query,
      queryWithSettings({ ultracode: enabled }),
      queryWithSettings({ ultracodeAvailable: true, ultracodeRequested: !enabled, ultracode: !enabled }),
    ]) {
      query.applyFlagSettings = async () => {};
      await assert.rejects(applyUltracode(query, enabled), (error: unknown) => {
        assert.ok(error instanceof UltracodeVerificationError);
        assert.match(error.message, /accepted.*could not be verified/);
        assert.ok(error.cause instanceof Error);
        return true;
      });
    }
  }
});

test("Ultracode errors map user-actionable SDK failures and hide raw protocol defects", () => {
  assert.equal(ultracodeError(new Error("apply_flag_settings: ultracode is not available for this session (dynamic workflows are off)")), "Cannot enable Ultracode: dynamic workflows are disabled for this session.");
  assert.equal(ultracodeError(new Error("apply_flag_settings: ultracode is not available for this session (haiku does not support it)")), "Cannot enable Ultracode: haiku does not support it.");
  assert.equal(ultracodeError(new Error("apply_flag_settings: ultracode must be a boolean")), "Cannot change Ultracode: an Agent SDK bridge/protocol error occurred.");
});

import assert from "node:assert/strict";
import test from "node:test";
import type { Query } from "@anthropic-ai/claude-agent-sdk";
import {
  applyUltracode,
  readUltracodeState,
  UltracodeVerificationError,
} from "./ultracode.js";

function queryWithSettings(applied: unknown): Query {
  return { getSettings: async () => ({ applied }) } as unknown as Query;
}

test("Ultracode decoding distinguishes independent available, requested and effective states", async () => {
  const accepted = [
    {
      applied: { ultracodeAvailable: false, ultracodeRequested: false, ultracode: false },
      expected: { available: false, requested: false, effective: false },
    },
    {
      applied: { ultracodeAvailable: false, ultracodeRequested: true, ultracode: false },
      expected: { available: false, requested: true, effective: false },
    },
    {
      applied: { ultracodeAvailable: true, ultracodeRequested: false, ultracode: false },
      expected: { available: true, requested: false, effective: false },
    },
    {
      applied: { ultracodeAvailable: true, ultracodeRequested: true, ultracode: true },
      expected: { available: true, requested: true, effective: true },
    },
  ];
  for (const { applied, expected } of accepted)
    assert.deepEqual(await readUltracodeState(queryWithSettings(applied)), expected);
  const inconsistent = [
    { ultracodeAvailable: false, ultracodeRequested: false, ultracode: true },
    { ultracodeAvailable: false, ultracodeRequested: true, ultracode: true },
    { ultracodeAvailable: true, ultracodeRequested: false, ultracode: true },
    { ultracodeAvailable: true, ultracodeRequested: true, ultracode: false },
  ];
  for (const applied of inconsistent)
    await assert.rejects(
      readUltracodeState(queryWithSettings(applied)),
      /Invalid Ultracode settings/,
    );
  for (const query of [{}, { getSettings: true }]) {
    await assert.rejects(readUltracodeState(query as Query), /unavailable.*SDK runtime/);
    assert.deepEqual(await readUltracodeState(queryWithSettings(accepted[1].applied)), {
      available: false,
      requested: true,
      effective: false,
    });
  }
  for (const settings of [undefined, null, [], {}, { applied: null }, { applied: [] }]) {
    await assert.rejects(
      readUltracodeState({ getSettings: async () => settings } as unknown as Query),
      /Invalid (?:applied )?settings response/,
    );
    assert.deepEqual(await readUltracodeState(queryWithSettings(accepted[3].applied)), {
      available: true,
      requested: true,
      effective: true,
    });
  }
  for (const field of ["ultracodeAvailable", "ultracodeRequested", "ultracode"]) {
    for (const value of [undefined, null, 0, "false", [], {}]) {
      const applied = {
        ultracodeAvailable: true,
        ultracodeRequested: true,
        ultracode: true,
        [field]: value,
      };
      await assert.rejects(
        readUltracodeState(queryWithSettings(applied)),
        /Invalid Ultracode settings/,
      );
    }
    assert.deepEqual(await readUltracodeState(queryWithSettings(accepted[3].applied)), {
      available: true,
      requested: true,
      effective: true,
    });
  }
});

test("Ultracode application settles before verification and preserves accepted versus rejected outcomes", async () => {
  const rows = [
    {
      enabled: true,
      applied: { ultracodeAvailable: true, ultracodeRequested: true, ultracode: true },
      expected: { available: true, requested: true, effective: true },
      payload: { ultracode: true },
    },
    {
      enabled: false,
      applied: { ultracodeAvailable: true, ultracodeRequested: false, ultracode: false },
      expected: { available: true, requested: false, effective: false },
      payload: { ultracode: false },
    },
    {
      enabled: true,
      applied: { ultracodeAvailable: false, ultracodeRequested: true, ultracode: false },
      expected: { available: false, requested: true, effective: false },
      payload: { ultracode: true },
    },
  ];
  for (const row of rows) {
    let release!: () => void;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    let started!: () => void;
    const admitted = new Promise<void>((resolve) => {
      started = resolve;
    });
    const calls: unknown[] = [];
    let settled = false;
    const query = {
      applyFlagSettings: async (settings: unknown) => {
        calls.push(settings);
        started();
        await held;
        settled = true;
      },
      getSettings: async () => {
        calls.push("read");
        assert.equal(settled, true, "verification must observe a settled native mutation");
        return { applied: row.applied };
      },
    } as unknown as Query;
    const operation = applyUltracode(query, row.enabled);
    try {
      await admitted;
      await new Promise<void>((resolve) => setImmediate(resolve));
      assert.deepEqual(calls, [row.payload], "no read while the accepted mutation is pending");
    } finally {
      release();
    }
    assert.deepEqual(await operation, row.expected);
    assert.deepEqual(calls, [row.payload, "read"]);
  }
  for (const enabled of [true, false]) {
    const rejected = new Error("native application refused");
    let reads = 0;
    await assert.rejects(
      applyUltracode(
        {
          applyFlagSettings: async () => {
            throw rejected;
          },
          getSettings: async () => {
            reads++;
            throw new Error("unreachable read");
          },
        } as unknown as Query,
        enabled,
      ),
      (error) => error === rejected,
    );
    assert.equal(reads, 0, "rejected application does not enter post-acceptance verification");
    const readFailure = new Error("distinct read failure");
    const queries = [
      { query: {} as Query, cause: /unavailable.*SDK runtime/ },
      {
        query: {
          getSettings: async () => {
            throw readFailure;
          },
        } as unknown as Query,
        cause: readFailure,
      },
      { query: queryWithSettings({ ultracode: enabled }), cause: /Invalid Ultracode settings/ },
      {
        query: queryWithSettings(
          enabled
            ? { ultracodeAvailable: true, ultracodeRequested: false, ultracode: false }
            : { ultracodeAvailable: true, ultracodeRequested: true, ultracode: true },
        ),
        cause: /request mismatch/,
      },
    ];
    for (const { query, cause } of queries) {
      const calls: unknown[] = [];
      query.applyFlagSettings = async (settings) => {
        calls.push(settings);
      };
      await assert.rejects(applyUltracode(query, enabled), (error: unknown) => {
        assert.ok(error instanceof UltracodeVerificationError);
        assert.deepEqual(calls, [{ ultracode: enabled }], "application was actually accepted");
        assert.ok(error.cause instanceof Error);
        if (cause instanceof Error) assert.equal(error.cause, cause);
        else assert.match(error.cause.message, cause);
        return true;
      });
    }
  }
});

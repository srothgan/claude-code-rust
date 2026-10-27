import test from "node:test";
import assert from "node:assert/strict";
import { resourceLinkContents } from "./resource_links.js";

test("resourceLinkContents validates optional fields and skips malformed entries", () => {
  assert.deepEqual(
    resourceLinkContents([
      {
        uri: " mcp://docs/report ",
        name: " report.csv ",
        title: " Quarterly report ",
        description: " Generated report ",
        mimeType: " text/csv ",
        size: 42,
        annotations: { audience: ["user"], priority: 0.8 },
      },
      { uri: "", name: "missing-uri" },
      { uri: "mcp://docs/missing-name", name: "" },
      {
        uri: "mcp://docs/bad-annotations",
        name: "bad",
        annotations: { invalid: undefined },
      },
      { uri: "mcp://docs/readme", name: "README.md", size: -1 },
    ]),
    [
      {
        type: "resource_link",
        uri: "mcp://docs/report",
        name: "report.csv",
        title: "Quarterly report",
        description: "Generated report",
        mime_type: "text/csv",
        size: 42,
        annotations: { audience: ["user"], priority: 0.8 },
      },
      {
        type: "resource_link",
        uri: "mcp://docs/readme",
        name: "README.md",
      },
    ],
  );
});

test("resourceLinkContents enforces accepted-count and aggregate serialized-byte limits", () => {
  const links = Array.from({ length: 55 }, (_, index) => ({
    uri: `mcp://docs/${index}`,
    name: `file-${index}`,
    title: `Report ${index}`,
    size: index,
  }));
  const oversized = {
    uri: "mcp://docs/oversized",
    name: "x".repeat(70 * 1024),
    title: "Oversized report",
    size: 0,
  };

  assert.deepEqual(
    resourceLinkContents([links[0], oversized, ...links.slice(1)]),
    Array.from({ length: 50 }, (_, index) => ({
      type: "resource_link",
      uri: `mcp://docs/${index}`,
      name: `file-${index}`,
      title: `Report ${index}`,
      size: index,
    })),
    "retain the first 50 accepted descriptors without charging a rejected entry a slot",
  );

  const budget = 64 * 1024;
  // Each unit uses four JS characters, five raw UTF-8 bytes and eight JSON bytes.
  const unicodeDescription = '\u00e9"\\\n'.repeat(8000) + "end";
  const expectedFirst = {
    type: "resource_link",
    uri: "mcp://docs/unicode",
    name: "unicode.txt",
    description: unicodeDescription,
    mime_type: "text/plain",
    annotations: { audience: ["user"], priority: 0.8 },
  };
  const tailMetadata = {
    type: "resource_link",
    uri: "mcp://docs/accepted",
    name: "accepted",
    title: "Budget tail",
    size: 11,
  };
  // Size independently specified wire descriptors, including both brackets and the comma.
  const tailDescription = "s".repeat(
    budget - Buffer.byteLength(JSON.stringify([
      expectedFirst,
      { ...tailMetadata, description: "" },
    ])),
  );
  const expectedTail = { ...tailMetadata, description: tailDescription };
  const rejected = {
    ...expectedTail,
    uri: "mcp://docs/rejected",
    name: "rejected",
    description: tailDescription + "x",
  };
  const expected = [expectedFirst, expectedTail];

  assert.equal(Buffer.byteLength(JSON.stringify(expected)), budget);
  assert.equal(
    Buffer.byteLength(JSON.stringify([expectedFirst, rejected])),
    budget + 1,
  );
  assert.ok(JSON.stringify([expectedFirst, rejected]).length < budget);
  for (const descriptor of [expectedFirst, rejected, expectedTail]) {
    assert.ok(Buffer.byteLength(JSON.stringify([descriptor])) < budget);
  }

  const parsed = resourceLinkContents([
    {
      uri: "mcp://docs/unicode",
      name: "unicode.txt",
      description: unicodeDescription,
      mimeType: "text/plain",
      annotations: { audience: ["user"], priority: 0.8 },
    },
    rejected,
    {
      uri: "mcp://docs/accepted",
      name: "accepted",
      title: "Budget tail",
      size: 11,
      description: tailDescription,
    },
    { uri: "mcp://docs/full", name: "no-room" },
  ]);
  assert.deepEqual(
    parsed,
    expected,
    "skip the candidate one byte over the aggregate budget, then retain the smaller tail at the exact limit",
  );
  assert.equal(Buffer.byteLength(JSON.stringify(parsed)), budget);
});

test("resourceLinkContents preserves annotation keys as inert JSON data", () => {
  const annotations = JSON.parse('{"__proto__":{"polluted":true}}') as unknown;
  const [content] = resourceLinkContents([
    { uri: "mcp://docs/safe", name: "safe", annotations },
  ]);

  assert.equal(content?.type, "resource_link");
  if (content?.type !== "resource_link") {
    return;
  }
  assert.ok(content.annotations);
  assert.deepEqual(content.annotations, annotations);
  assert.equal(Object.hasOwn(content.annotations, "__proto__"), true);
  assert.equal(({} as { polluted?: boolean }).polluted, undefined);
});

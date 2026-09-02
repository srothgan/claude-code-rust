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

test("resourceLinkContents enforces count and serialized-size caps per entry", () => {
  const links = Array.from({ length: 55 }, (_, index) => ({
    uri: `mcp://docs/${index}`,
    name: `file-${index}`,
  }));
  links.splice(1, 0, {
    uri: "mcp://docs/oversized",
    name: "x".repeat(70 * 1024),
  });

  const parsed = resourceLinkContents(links);
  const parsedLinks = parsed.filter(
    (link): link is Extract<typeof link, { type: "resource_link" }> =>
      link.type === "resource_link",
  );

  assert.equal(parsedLinks.length, 50);
  assert.equal(
    parsedLinks.some((link) => link.name.length > 64 * 1024),
    false,
  );
  assert.equal(parsedLinks.at(0)?.uri, "mcp://docs/0");
  assert.equal(parsedLinks.at(1)?.uri, "mcp://docs/1");
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

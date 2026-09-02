import type { Json, ToolCallContent } from "../types.js";
import { asRecordOrNull } from "./shared.js";

const MAX_RESOURCE_LINKS = 50;
const MAX_RESOURCE_LINK_BYTES = 64 * 1024;

function nonEmptyString(value: unknown): string | undefined {
  return typeof value === "string" && value.trim().length > 0
    ? value.trim()
    : undefined;
}

function jsonValue(value: unknown): Json | undefined {
  if (
    value === null ||
    typeof value === "string" ||
    typeof value === "boolean"
  ) {
    return value;
  }
  if (typeof value === "number") {
    return Number.isFinite(value) ? value : undefined;
  }
  if (Array.isArray(value)) {
    const entries: Json[] = [];
    for (const entry of value) {
      const parsed = jsonValue(entry);
      if (parsed === undefined) {
        return undefined;
      }
      entries.push(parsed);
    }
    return entries;
  }
  const record = asRecordOrNull(value);
  if (!record) {
    return undefined;
  }
  const parsed: Array<[string, Json]> = [];
  for (const [key, entry] of Object.entries(record)) {
    const value = jsonValue(entry);
    if (value === undefined) {
      return undefined;
    }
    parsed.push([key, value]);
  }
  return Object.fromEntries(parsed);
}

function resourceLinkContent(value: unknown): ToolCallContent | undefined {
  const link = asRecordOrNull(value);
  const uri = nonEmptyString(link?.uri);
  const name = nonEmptyString(link?.name);
  if (!link || !uri || !name) {
    return undefined;
  }

  const title = nonEmptyString(link.title);
  const description = nonEmptyString(link.description);
  const mimeType = nonEmptyString(link.mimeType);
  const size =
    typeof link.size === "number" &&
    Number.isSafeInteger(link.size) &&
    link.size >= 0
      ? link.size
      : undefined;
  const annotationsValue =
    link.annotations === undefined ? undefined : jsonValue(link.annotations);
  const annotations = asRecordOrNull(annotationsValue) as
    | Record<string, Json>
    | null;
  if (link.annotations !== undefined && !annotations) {
    return undefined;
  }

  return {
    type: "resource_link",
    uri,
    name,
    ...(title ? { title } : {}),
    ...(description ? { description } : {}),
    ...(mimeType ? { mime_type: mimeType } : {}),
    ...(size !== undefined ? { size } : {}),
    ...(annotations ? { annotations } : {}),
  };
}

export function resourceLinkContents(value: unknown): ToolCallContent[] {
  if (!Array.isArray(value)) {
    return [];
  }

  const links: ToolCallContent[] = [];
  let serializedBytes = 2;
  for (const entry of value) {
    if (links.length >= MAX_RESOURCE_LINKS) {
      break;
    }
    const link = resourceLinkContent(entry);
    if (!link) {
      continue;
    }
    const linkBytes = Buffer.byteLength(JSON.stringify(link));
    const separatorBytes = links.length > 0 ? 1 : 0;
    if (
      serializedBytes + separatorBytes + linkBytes >
      MAX_RESOURCE_LINK_BYTES
    ) {
      continue;
    }
    serializedBytes += separatorBytes + linkBytes;
    links.push(link);
  }
  return links;
}

export function appendResourceLinks(
  content: ToolCallContent[] | undefined,
  value: unknown,
): ToolCallContent[] | undefined {
  const links = resourceLinkContents(value);
  return links.length > 0 ? [...(content ?? []), ...links] : content;
}

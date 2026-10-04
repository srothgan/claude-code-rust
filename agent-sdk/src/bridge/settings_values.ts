import { isDeepStrictEqual } from "node:util";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { HOOK_EVENTS, resolveSettings } from "@anthropic-ai/claude-agent-sdk";
import type { Json, SettingDescriptor, SettingsEditorSchema } from "../types.js";

// Project only fields the editor owns for isolated native validation. Persist the
// original value: extra object fields must not be stripped from the user's file.
function validationValue(value: Json, schema: SettingsEditorSchema): Json {
  const item = schema.item;
  if (schema.type === "array" && Array.isArray(value) && item) {
    return value.map(child => validationValue(child, item));
  }
  if (value === null || typeof value !== "object" || Array.isArray(value)) return value;
  if (schema.type === "variant") {
    const type = value.type;
    const variant = typeof type === "string" ? schema.variants?.[type] : undefined;
    return variant ? { ...validationValue(value, variant) as Record<string, Json>, type } : value;
  }
  if (schema.type === "object") {
    return Object.fromEntries((schema.fields ?? []).filter(field => Object.hasOwn(value, field.key)).map(field => [field.key, validationValue(value[field.key], field.schema)]));
  }
  if (schema.type === "map" && item) {
    return Object.fromEntries(Object.entries(value).map(([key, child]) => [key, validationValue(child, item)]));
  }
  return value;
}

export function settingLeaf(document: unknown, keys: string[]): Json | undefined {
  if (keys.length === 0) return undefined;
  let value: unknown = document;
  for (const key of keys) {
    value = typeof value === "object" && value !== null && !Array.isArray(value)
      ? (value as Record<string, unknown>)[key] : undefined;
  }
  return value as Json | undefined;
}

/** Validate the edited value only; unknown or invalid siblings remain untouched. */
export async function validateSettingValue(setting: SettingDescriptor, value: Json): Promise<void> {
  switch (setting.kind) {
    case "boolean": case "string": case "number":
      if (typeof value !== setting.kind) throw new Error(`Expected ${setting.kind}.`);
      break;
    case "string_list":
      if (!Array.isArray(value) || !value.every(item => typeof item === "string" && item.length > 0)) {
        throw new Error("Enter one nonempty value per entry.");
      }
      break;
    case "json":
      if (value === null || typeof value !== "object" || Array.isArray(value)) throw new Error("Expected a JSON object.");
      break;
  }
  if (!setting.allows_custom && !setting.options.includes(value)) throw new Error("Choose one of the available options.");
  if (typeof value === "string" && value.length === 0) throw new Error("Use reset to remove the saved value instead of storing an empty value.");
  if (setting.kind === "number" && (!Number.isInteger(value) || Number(value) < 1 || Number(value) > 65535)) {
    throw new Error("Enter a whole TCP port from 1 to 65535.");
  }
  if (setting.id === "hooks" && typeof value === "object" && value !== null) {
    const unknown = Object.keys(value).find(event => !(HOOK_EVENTS as readonly string[]).includes(event));
    if (unknown) throw new Error(`Unknown hook event: ${unknown}. Use an event supported by the installed version.`);
  }
  if (setting.id === "sandbox.network.tlsTerminate") {
    const fields = value as Record<string, Json>;
    if ((fields.caCertPath === undefined) !== (fields.caKeyPath === undefined)) throw new Error("Supply both certificate and key paths, or neither.");
  }
  if (!["string_list", "json"].includes(setting.kind)) return;

  // The public SDK exposes resolution, not its schema. Resolve an isolated
  // candidate through that schema, without changing profiles or executing hooks.
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), "claude-rs-value-"));
  try {
    const checked = setting.kind === "json" && setting.editor ? validationValue(value, setting.editor) : value;
    const candidate = setting.key_path.reduceRight<Json>((child, key) => ({ [key]: child }), checked);
    await fs.mkdir(path.join(directory, ".claude"));
    await fs.writeFile(path.join(directory, ".claude", "settings.json"), JSON.stringify(candidate), { mode: 0o600 });
    const resolved = await resolveSettings({ cwd: directory, settingSources: ["project"] });
    const accepted = settingLeaf(resolved.sources.find(source => source.source === "project")?.settings, setting.key_path);
    if (!isDeepStrictEqual(accepted, checked)) throw new Error("This value contains entries the installed version cannot load. Review the setting's format before saving.");
  } finally {
    await fs.rm(directory, { recursive: true, force: true });
  }
}

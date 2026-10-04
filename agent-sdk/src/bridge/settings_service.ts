import { createHash, randomUUID } from "node:crypto";
import { execFile } from "node:child_process";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { promisify } from "node:util";
import { resolveSettings } from "@anthropic-ai/claude-agent-sdk";
import type { Json, SettingDescriptor, SettingsMutation, SettingsScope, SettingsSnapshot, SettingsResult, AvailableModel, AvailableAgent } from "../types.js";
import { isAppSetting, settingsCatalog } from "./settings_catalog.js";
import { SETTINGS_CATEGORIES } from "./settings_layout.js";
import { settingLeaf as leaf, validateSettingValue } from "./settings_values.js";

const SCOPES: SettingsScope[] = ["user", "project", "local"];
const NOT_LOADED = "not loaded";
const runFile = promisify(execFile);
const object = (value: unknown): value is Record<string, Json> => typeof value === "object" && value !== null && !Array.isArray(value);
function revision(value: unknown): string {
  return createHash("sha256").update(JSON.stringify(value) ?? "undefined").digest("hex");
}
function sourcePaths(cwd: string): Record<SettingsScope, string> {
  // Missing sources have no public SDK path. Existing sources always use SDK-reported paths.
  return {
    user: path.join(process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), ".claude"), "settings.json"),
    project: path.join(cwd, ".claude", "settings.json"),
    local: path.join(cwd, ".claude", "settings.local.json"),
  };
}
type Document = { status: "missing" | "valid" | "invalid" | "unreadable"; document?: Record<string, Json>; raw?: string; error?: string; mode?: number };
async function readDocument(file: string): Promise<Document> {
  try {
    const stat = await fs.lstat(file);
    if (!stat.isFile() || stat.isSymbolicLink()) return { status: "unreadable", error: "Settings path must be a regular file." };
    const raw = await fs.readFile(file, "utf8");
    try {
      const document: unknown = JSON.parse(raw);
      return object(document) ? { status: "valid", document, raw, mode: stat.mode } : { status: "invalid", error: "Expected a JSON object." };
    } catch { return { status: "invalid", error: "Invalid JSON; repair the file before editing." }; }
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "ENOENT" ? { status: "missing", document: {} } : { status: "unreadable", error: "Cannot read settings file." };
  }
}

export async function inspectSettings(cwd: string, models: AvailableModel[] = [], agents: AvailableAgent[] = [], appSettingsPath?: string): Promise<SettingsSnapshot> {
  const resolved = await resolveSettings({ cwd, settingSources: SCOPES });
  const paths = sourcePaths(cwd);
  for (const source of resolved.sources) if (SCOPES.includes(source.source as SettingsScope) && source.path) paths[source.source as SettingsScope] = source.path;
  const catalog = settingsCatalog(models, agents, resolved.effective.model);
  const appDocument = appSettingsPath ? await readDocument(appSettingsPath) : { status: "unreadable" as const, error: "App settings are unavailable." };
  for (const setting of catalog.filter(setting => isAppSetting(setting))) {
    if (!appSettingsPath || !appDocument.document) {
      setting.writable_scopes = [];
      setting.unavailable = appDocument.error ?? "App settings are unavailable.";
    }
  }
  const sources = await Promise.all(SCOPES.map(async scope => {
    const file = paths[scope];
    const read = await readDocument(file);
    // An omitted source may have been rejected or contain only ignored entries.
    // Report loading separately; do not recreate the SDK's schema validation.
    const notLoaded = read.status === "valid" && read.document !== undefined && Object.keys(read.document).length > 0 && !resolved.sources.some(source => source.source === scope);
    const error = notLoaded ? "Claude did not load settings from this file." : read.error;
    return { scope, path: file, status: notLoaded ? NOT_LOADED : read.status, ...(error ? { error } : {}), values: catalog.map(setting => {
      const value = leaf(isAppSetting(setting) ? scope === "user" ? appDocument.document : undefined : read.document, setting.key_path);
      return { id: setting.id, revision: revision(value), ...(value !== undefined ? { value } : {}) };
    }) };
  }));
  // Only explicitly editable catalog values cross stdio, never opaque profile/auth data.
  const values = catalog.map(setting => {
    const value = leaf(isAppSetting(setting) ? appDocument.document : resolved.effective, setting.key_path);
    const contributors = resolved.sources.filter(source => leaf(source.settings, setting.key_path) !== undefined);
    const managed = resolved.sources.find(source => source.source === "managed")?.settings;
    const pluginOnly = managed?.strictPluginOnlyCustomization;
    const policy = contributors.some(source => source.source === "managed")
      || (["permissions.allow", "permissions.ask", "permissions.deny"].includes(setting.id) && managed?.allowManagedPermissionRulesOnly === true)
      || (setting.id === "hooks" && (managed?.allowManagedHooksOnly === true || pluginOnly === true || (Array.isArray(pluginOnly) && pluginOnly.includes("hooks"))))
      || (["sandbox.network.allowedDomains", "sandbox.network.httpProxyPort", "sandbox.network.socksProxyPort"].includes(setting.id) && managed?.sandbox?.network?.allowManagedDomainsOnly === true)
      || (setting.id === "sandbox.filesystem.allowRead" && managed?.sandbox?.filesystem?.allowManagedReadPathsOnly === true)
      || (setting.id === "sandbox.filesystem.disabled" && (managed?.sandbox?.filesystem !== undefined || managed?.sandbox?.credentials?.files?.some(entry => entry.mode === "deny") === true));
    return { id: setting.id, ...(value !== undefined ? { value } : {}), contributors: isAppSetting(setting) ? value === undefined ? [] : ["user"] : contributors.map(source => source.source), policy_restricted: policy };
  });
  for (const value of values) {
    if (value.policy_restricted) {
      const setting = catalog.find(setting => setting.id === value.id);
      if (setting) { setting.writable_scopes = []; setting.unavailable = "Your organization controls this setting."; }
    }
  }
  const resolution_sources = resolved.sources.map(source => ({ source: source.source, ...(source.path ? { path: source.path } : {}), ...(source.policyOrigin ? { policy_origin: source.policyOrigin } : {}) }));
  const provenance: SettingsSnapshot["provenance"] = {};
  for (const setting of catalog) {
    if (isAppSetting(setting) && appSettingsPath) {
      provenance[setting.id] = { source: "user", path: appSettingsPath };
      continue;
    }
    const source = resolved.provenance[setting.key_path[0] as keyof typeof resolved.provenance];
    if (source) provenance[setting.id] = { source: source.source, ...(source.path ? { path: source.path } : {}), ...(source.policyOrigin ? { policy_origin: source.policyOrigin } : {}) };
  }
  return { cwd, context: revision({ cwd, paths, appSettingsPath, effortPath: catalog.find(setting => setting.id === "defaultEffort")?.key_path }), categories: SETTINGS_CATEGORIES, catalog, sources, values, resolution_sources, provenance, ...(resolved.effective.timeZone ? { time_zone: resolved.effective.timeZone } : {}), diagnostics: ["SDK raw cascade: active session choices and trust filtering are separate. policyHelper is not executed by resolveSettings."] };
}

function patch(document: Record<string, Json>, keys: string[], value: Json | undefined): void {
  let current = document;
  const parents: Array<[Record<string, Json>, string]> = [];
  for (const key of keys.slice(0, -1)) {
    if (current[key] !== undefined && !object(current[key])) throw new Error("A parent key is not an object; repair it before editing.");
    if (current[key] === undefined) {
      if (value === undefined) return;
      current[key] = {};
    }
    parents.push([current, key]);
    current = current[key] as Record<string, Json>;
  }
  const last = keys.at(-1);
  if (!last) throw new Error("Empty setting path.");
  if (value === undefined) {
    delete current[last];
    for (const [parent, key] of parents.reverse()) {
      if (object(parent[key]) && Object.keys(parent[key]).length === 0) delete parent[key];
      else break;
    }
  } else current[last] = value;
}

async function writeTarget(file: string, setting: SettingDescriptor, mutation: SettingsMutation): Promise<"saved" | "unchanged" | "conflict"> {
  await fs.mkdir(path.dirname(file), { recursive: true });
  // Cooperative lock serializes our writers; byte rechecks detect edits from external editors.
  const lockPath = `${file}.claude-rs.lock`;
  const lock = await fs.open(lockPath, "wx", 0o600);
  try {
    for (let attempt = 0; attempt < 3; attempt++) {
      const read = await readDocument(file);
      if (!read.document) throw new Error(read.error ?? "Cannot read settings.");
      const previous = leaf(read.document, setting.key_path);
      if (revision(previous) !== mutation.expected_revision) return "conflict";
      const next = mutation.operation === "remove" ? undefined : mutation.value;
      if (revision(previous) === revision(next)) return "unchanged";
      patch(read.document, setting.key_path, next);
      const temp = `${file}.${randomUUID()}.tmp`;
      if (mutation.scope === "local") await excludeLocalSettings(file, temp);
      try {
        const handle = await fs.open(temp, "wx", read.mode ?? 0o600);
        try { await handle.writeFile(`${JSON.stringify(read.document, null, 2)}\n`); await handle.sync(); } finally { await handle.close(); }
        const current = await readDocument(file);
        if (current.raw !== read.raw || current.status !== read.status) continue;
        await fs.rename(temp, file);
        return "saved";
      } finally { await fs.rm(temp, { force: true }); }
    }
    return "conflict";
  } finally { await lock.close(); await fs.unlink(lockPath); }
}

async function excludeLocalSettings(file: string, temp: string): Promise<void> {
  const physicalFile = path.join(await fs.realpath(path.dirname(file)), path.basename(file));
  let cwd = path.dirname(physicalFile);
  const git = (args: string[]) => runFile("git", args, { cwd, windowsHide: true });
  let root: string;
  try { root = (await git(["rev-parse", "--show-toplevel"])).stdout.trim(); }
  catch (error) {
    const failure = error as NodeJS.ErrnoException & { stderr?: string };
    if (failure.code === "ENOENT" || failure.stderr?.includes("not a git repository")) return;
    throw new Error("Cannot verify Git protection for local settings.");
  }
  cwd = root;
  const relative = path.relative(await fs.realpath(root), physicalFile).split(path.sep).join("/");
  try {
    await git(["ls-files", "--error-unmatch", "--", relative]);
    throw new Error("Local settings are tracked by Git. Untrack the file before editing local values.");
  } catch (error) {
    if ((error as { code?: unknown }).code !== 1) throw error;
  }
  async function ignored(candidate: string): Promise<boolean> {
    try { await git(["check-ignore", "--quiet", "--", candidate]); return true; }
    catch (error) { if ((error as { code?: unknown }).code !== 1) throw error; return false; }
  }
  const relativeTemp = path.posix.join(path.posix.dirname(relative), path.basename(temp));
  if (await ignored(relative) && await ignored(relativeTemp)) return;
  const exclude = (await git(["rev-parse", "--path-format=absolute", "--git-path", "info/exclude"])).stdout.trim();
  await fs.mkdir(path.dirname(exclude), { recursive: true });
  // Append a literal path without replacing existing user exclusions.
  const pattern = relative.replace(/[\\*?[\]#! ]/g, "\\$&");
  await fs.appendFile(exclude, `\n/${pattern}\n/${pattern}.*\n`, { mode: 0o600 });
  await git(["check-ignore", "--quiet", "--", relative]);
  await git(["check-ignore", "--quiet", "--", relativeTemp]);
}

export async function mutateSetting(cwd: string, mutation: SettingsMutation, models: AvailableModel[] = [], agents: AvailableAgent[] = [], appSettingsPath?: string): Promise<SettingsResult> {
  let persistence: SettingsResult["persistence"] = "failure";
  let snapshot: SettingsSnapshot | undefined;
  try {
    snapshot = await inspectSettings(cwd, models, agents, appSettingsPath);
    if (snapshot.context !== mutation.context) throw new Error("Settings context changed; refresh before editing.");
    const setting = snapshot.catalog.find(entry => entry.id === mutation.id);
    if (!setting?.writable_scopes.includes(mutation.scope)) throw new Error("This setting cannot be edited at this scope.");
    if (mutation.operation === "set") {
      if (mutation.value === undefined) throw new Error("A set operation requires a value.");
      await validateSettingValue(setting, mutation.value);
    }
    const source = snapshot.sources.find(entry => entry.scope === mutation.scope);
    if (!source) throw new Error("Settings source is unavailable.");
    persistence = await writeTarget(isAppSetting(setting) && appSettingsPath ? appSettingsPath : source.path, setting, mutation);
    const refreshed = await inspectSettings(cwd, models, agents, appSettingsPath);
    const notLoaded = !isAppSetting(setting) && mutation.operation === "set" && refreshed.sources.find(source => source.scope === mutation.scope)?.status === NOT_LOADED;
    const error = persistence === "conflict" ? "This value changed since it was displayed. Review the refreshed saved value before retrying."
      : notLoaded ? "Your change was saved, but Claude did not load this settings file. Check its values before the change can take effect." : undefined;
    return { persistence, application: persistence === "conflict" || notLoaded ? "blocked" : setting.application, snapshot: refreshed, ...(error ? { error } : {}) };
  } catch (error) {
    // A refresh failure after a successful save must not be reported as a failed save.
    return { persistence, application: "blocked", ...(persistence === "failure" && snapshot ? { snapshot } : {}), error: error instanceof Error ? error.message : "Settings operation failed." };
  }
}

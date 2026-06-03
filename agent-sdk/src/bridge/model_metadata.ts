import type { ModelInfo } from "@anthropic-ai/claude-agent-sdk";
import type { AvailableModel, CurrentModel, EffortLevel } from "../types.js";

type ModelMetadataSession = {
  model: string;
  requestedModelId?: string;
  resolvedRuntimeModelId?: string;
  availableModels: AvailableModel[];
};

type NormalizedModelKey = {
  original: string;
  family: "opus" | "sonnet" | "haiku" | "unknown";
  versionParts: number[];
  variantParts: string[];
  buildParts: string[];
  contextSuffix?: string;
};

const OPUS_MODEL_ALIAS = "opus";
const MAX_MODEL_VERSION_PARTS = 2;
const RELEASE_BUILD_TOKEN = /^20\d{6}$/;

function isEffortLevel(value: unknown): value is EffortLevel {
  return (
    value === "low" ||
    value === "medium" ||
    value === "high" ||
    value === "xhigh" ||
    value === "max"
  );
}

function normalizeModelKey(id: string): NormalizedModelKey {
  const original = id.trim();
  if (!original) {
    return { original, family: "unknown", versionParts: [], variantParts: [], buildParts: [] };
  }

  const lower = original.toLowerCase();
  const contextMatch = lower.match(/\[([^\]]+)\]$/);
  const contextSuffix = contextMatch?.[1];
  const withoutContext = contextMatch ? lower.slice(0, contextMatch.index) : lower;
  const withoutPrefix = withoutContext.startsWith("claude-")
    ? withoutContext.slice("claude-".length)
    : withoutContext;
  const parts = withoutPrefix.split("-").filter((part) => part.length > 0);
  const familyPart = parts[0] ?? "";
  const family =
    familyPart === "opus" || familyPart === "sonnet" || familyPart === "haiku"
      ? familyPart
      : "unknown";
  const versionParts: number[] = [];
  const variantParts: string[] = [];
  const buildParts: string[] = [];

  if (family !== "unknown") {
    for (const part of parts.slice(1)) {
      if (/^\d+$/.test(part)) {
        if (versionParts.length < MAX_MODEL_VERSION_PARTS) {
          const parsed = Number.parseInt(part, 10);
          if (Number.isFinite(parsed)) {
            versionParts.push(parsed);
          }
          continue;
        }
        if (RELEASE_BUILD_TOKEN.test(part)) {
          buildParts.push(part);
          continue;
        }
      }
      variantParts.push(part);
    }
  }

  return {
    original,
    family,
    versionParts,
    variantParts,
    buildParts,
    ...(contextSuffix ? { contextSuffix } : {}),
  };
}

function modelKeysAreCompatible(leftId: string, rightId: string): boolean {
  const left = normalizeModelKey(leftId);
  const right = normalizeModelKey(rightId);
  if (left.family === "unknown" || right.family === "unknown") {
    return left.original.toLowerCase() === right.original.toLowerCase();
  }
  if (left.family !== right.family) {
    return false;
  }
  if (left.variantParts.join(".") !== right.variantParts.join(".")) {
    return false;
  }
  if (left.versionParts.length === 0 || right.versionParts.length === 0) {
    return true;
  }
  return left.versionParts.join(".") === right.versionParts.join(".");
}

function sameContextSuffix(leftId: string, rightId: string): boolean {
  const left = normalizeModelKey(leftId);
  const right = normalizeModelKey(rightId);
  return (left.contextSuffix?.toLowerCase() ?? "") === (right.contextSuffix?.toLowerCase() ?? "");
}

function sameFamilyAndVersion(leftId: string, rightId: string): boolean {
  const left = normalizeModelKey(leftId);
  const right = normalizeModelKey(rightId);
  if (left.family === "unknown" || right.family === "unknown") {
    return left.original.toLowerCase() === right.original.toLowerCase();
  }
  if (left.family !== right.family) {
    return false;
  }
  if (left.versionParts.length === 0 || right.versionParts.length === 0) {
    return left.versionParts.length === right.versionParts.length;
  }
  return left.versionParts.join(".") === right.versionParts.join(".");
}

function hasVariantSiblingConflict(
  availableModels: AvailableModel[],
  candidateId: string,
  resolvedId: string,
): boolean {
  if (sameContextSuffix(candidateId, resolvedId)) {
    return false;
  }

  const resolvedContext = normalizeModelKey(resolvedId).contextSuffix?.toLowerCase() ?? "";
  if (!resolvedContext) {
    return false;
  }

  return availableModels.some((entry) => {
    if (entry.id === candidateId) {
      return false;
    }
    if (!sameFamilyAndVersion(entry.id, resolvedId)) {
      return false;
    }
    const entryContext = normalizeModelKey(entry.id).contextSuffix?.toLowerCase() ?? "";
    return entryContext === resolvedContext;
  });
}

function humanizeModelId(id: string): string {
  const normalized = normalizeModelKey(id);
  if (normalized.family === "unknown") {
    return id;
  }

  const familyLabel =
    normalized.family === "opus"
      ? "Opus"
      : normalized.family === "sonnet"
        ? "Sonnet"
        : "Haiku";
  const versionLabel =
    normalized.versionParts.length > 0 ? ` ${normalized.versionParts.join(".")}` : "";
  const contextLabel =
    normalized.contextSuffix?.toLowerCase() === "1m"
      ? " [1M]"
      : normalized.contextSuffix
        ? ` [${normalized.contextSuffix}]`
        : "";
  return `${familyLabel}${versionLabel}${contextLabel}`;
}

function shortDisplayNameForModelId(id: string): string {
  return humanizeModelId(id);
}

function currentModelIsAuthoritative(
  resolvedId: string,
  requestedId: string | undefined,
): boolean {
  const resolved = resolvedId.trim();
  if (!resolved || resolved === "Connecting...") {
    return Boolean(requestedId?.trim());
  }
  return true;
}

function resolveCatalogModel(
  availableModels: AvailableModel[],
  resolvedId: string,
  requestedId: string | undefined,
): AvailableModel | undefined {
  const exactResolved = availableModels.find((entry) => entry.id === resolvedId);
  if (exactResolved) {
    return exactResolved;
  }

  if (requestedId) {
    const exactRequested = availableModels.find((entry) => entry.id === requestedId);
    if (
      exactRequested &&
      modelKeysAreCompatible(exactRequested.id, resolvedId) &&
      !hasVariantSiblingConflict(availableModels, exactRequested.id, resolvedId)
    ) {
      return exactRequested;
    }
  }

  const compatible = availableModels.filter(
    (entry) =>
      modelKeysAreCompatible(entry.id, resolvedId) &&
      !hasVariantSiblingConflict(availableModels, entry.id, resolvedId),
  );
  return compatible.length === 1 ? compatible[0] : undefined;
}

export function mapAvailableModels(models: ModelInfo[] | undefined): AvailableModel[] {
  if (!Array.isArray(models)) {
    return [];
  }

  return models
    .filter((entry): entry is ModelInfo & { value: string; displayName: string } => {
      return (
        typeof entry?.value === "string" &&
        entry.value.trim().length > 0 &&
        typeof entry.displayName === "string" &&
        entry.displayName.trim().length > 0
      );
    })
    .map((entry) => ({
      id: entry.value,
      display_name: entry.displayName,
      supports_effort: entry.supportsEffort === true,
      supported_effort_levels: Array.isArray(entry.supportedEffortLevels)
        ? entry.supportedEffortLevels.filter(isEffortLevel)
        : [],
      ...(typeof entry.supportsAdaptiveThinking === "boolean"
        ? { supports_adaptive_thinking: entry.supportsAdaptiveThinking }
        : {}),
      ...(typeof entry.supportsFastMode === "boolean"
        ? { supports_fast_mode: entry.supportsFastMode }
        : {}),
      ...(typeof entry.supportsAutoMode === "boolean"
        ? { supports_auto_mode: entry.supportsAutoMode }
        : {}),
      ...(typeof entry.description === "string" && entry.description.trim().length > 0
        ? { description: entry.description }
        : {}),
    }));
}

export function resolveCurrentModel(session: ModelMetadataSession): CurrentModel {
  const requestedId = session.requestedModelId?.trim() || undefined;
  const resolvedId =
    session.resolvedRuntimeModelId?.trim() ||
    session.model.trim() ||
    requestedId ||
    OPUS_MODEL_ALIAS;
  const catalogModel = resolveCatalogModel(session.availableModels, resolvedId, requestedId);
  const runtimeDisplayId = resolvedId || requestedId || OPUS_MODEL_ALIAS;
  const displayNameShort = shortDisplayNameForModelId(runtimeDisplayId);
  const displayNameLong = catalogModel?.display_name ?? humanizeModelId(runtimeDisplayId);
  return {
    resolved_id: resolvedId,
    display_name_short: displayNameShort,
    display_name_long: displayNameLong,
    supports_effort: catalogModel?.supports_effort === true,
    supported_effort_levels: catalogModel?.supported_effort_levels ?? [],
    is_authoritative: currentModelIsAuthoritative(resolvedId, requestedId),
    ...(requestedId ? { requested_id: requestedId } : {}),
    ...(catalogModel ? { catalog_id: catalogModel.id } : {}),
    ...(catalogModel?.supports_fast_mode !== undefined
      ? { supports_fast_mode: catalogModel.supports_fast_mode }
      : {}),
    ...(catalogModel?.supports_auto_mode !== undefined
      ? { supports_auto_mode: catalogModel.supports_auto_mode }
      : {}),
    ...(catalogModel?.supports_adaptive_thinking !== undefined
      ? { supports_adaptive_thinking: catalogModel.supports_adaptive_thinking }
      : {}),
  };
}

export function currentModelsEqual(left: CurrentModel | undefined, right: CurrentModel): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

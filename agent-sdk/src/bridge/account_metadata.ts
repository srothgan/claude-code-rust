import type { AccountInfo as SdkAccountInfo } from "@anthropic-ai/claude-agent-sdk";
import type { AccountInfo } from "../types.js";

export type KnownApiProvider =
  | "firstParty"
  | "bedrock"
  | "vertex"
  | "foundry"
  | "anthropicAws"
  | "anthropicGoogleCloud"
  | "mantle"
  | "gateway";

const KNOWN_API_PROVIDERS = new Set<string>([
  "firstParty",
  "bedrock",
  "vertex",
  "foundry",
  "anthropicAws",
  "anthropicGoogleCloud",
  "mantle",
  "gateway",
]);

function trimmedString(value: unknown): string | undefined {
  return typeof value === "string" && value.trim().length > 0
    ? value.trim()
    : undefined;
}

export function isKnownApiProvider(
  provider: string | undefined,
): provider is KnownApiProvider {
  return provider !== undefined && KNOWN_API_PROVIDERS.has(provider);
}

export function apiProviderIsExternal(provider: string | undefined): boolean {
  return provider !== undefined && provider !== "firstParty";
}

export function mapSdkAccountInfo(account: SdkAccountInfo): AccountInfo {
  const apiProvider = trimmedString(account.apiProvider);
  return {
    ...(trimmedString(account.email)
      ? { email: trimmedString(account.email) }
      : {}),
    ...(trimmedString(account.organization)
      ? { organization: trimmedString(account.organization) }
      : {}),
    ...(trimmedString(account.subscriptionType)
      ? { subscription_type: trimmedString(account.subscriptionType) }
      : {}),
    ...(trimmedString(account.tokenSource)
      ? { token_source: trimmedString(account.tokenSource) }
      : {}),
    ...(trimmedString(account.apiKeySource)
      ? { api_key_source: trimmedString(account.apiKeySource) }
      : {}),
    ...(apiProvider ? { api_provider: apiProvider } : {}),
  };
}

export function shouldEmitStartupAuthRequiredForAccount(
  account: SdkAccountInfo,
): boolean {
  const provider = trimmedString(account.apiProvider);
  if (apiProviderIsExternal(provider)) {
    return false;
  }
  return !trimmedString(account.email) && !trimmedString(account.apiKeySource);
}

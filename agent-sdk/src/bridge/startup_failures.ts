/** Bound and redact SDK stderr before it enters startup diagnostics or the wire. */
export function startupFailureDetails(errors: unknown): string[] {
  if (!Array.isArray(errors) || !errors.every((entry) => typeof entry === "string")) {
    return [];
  }
  return errors.slice(0, 8).map((error: string) =>
    redactStartupDetail(error).slice(0, 1_024),
  );
}

export function redactStartupDetail(value: string): string {
  return value
    .replace(/\bBearer\s+[^\s"',;]+/gi, "Bearer [redacted]")
    .replace(/\bsk-(?:ant-)?[a-z0-9_-]+/gi, "[redacted]")
    .replace(/((?:[a-z0-9_-]*(?:api[_-]?key|token|secret|password|credential|authorization)[a-z0-9_-]*)["']?\s*[:=]\s*)(?:"[^"\n]*"|'[^'\n]*'|[^\s,;]+)/gi, "$1[redacted]")
    .replace(/(https?:\/\/)[^\s/@]+@/gi, "$1[redacted]@");
}

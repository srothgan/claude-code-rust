import { homedir } from "node:os";

/** Bound and redact SDK stderr before it enters startup diagnostics or the wire. */
export function startupFailureDetails(errors: unknown): string[] {
  if (!Array.isArray(errors) || !errors.every((entry) => typeof entry === "string")) {
    return [];
  }
  return errors.slice(0, 8).map((error: string) =>
    redactStartupDetail(error).slice(0, 1_024),
  );
}

/**
 * Twin of `redact_text` in `src/cli/redaction.rs`, which is the reference: its
 * table tests list the shapes both must cover, and a rule added to one is added
 * to the other.
 */
export function redactStartupDetail(value: string, home: string = homedir()): string {
  return redactHomePrefix(
    value
      .replace(/\bBearer\s+[^\s"',;]+/gi, "Bearer [redacted]")
      .replace(/\bsk-[a-z0-9_-]+/gi, "[redacted]")
      .replace(/(https?:\/\/)[^\s/@]+@/gi, "$1[redacted]@")
      .replace(
        /((?:api[_-]?key|secret|password|credential|authorization)[a-z0-9_-]*["']?\s*[:=]\s*)(?:(?:basic|digest|negotiate|bearer|token)\s+)?(?:"[^"\n]*"|'[^'\n]*'|[^\s,;&]+)/gi,
        "$1[redacted]",
      )
      // `token`, `access_token`, `refresh-token`, `accessToken`; not `max_tokens`.
      .replace(
        /((?<![A-Za-z0-9])(?:[A-Za-z0-9_-]*[_-])?[tT][oO][kK][eE][nN]|[A-Za-z0-9]*Token)(?![A-Za-z0-9_-])(["']?\s*[:=]\s*)(?:(?:basic|digest|negotiate|bearer|token)\s+)?(?:"[^"\n]*"|'[^'\n]*'|[^\s,;&]+)/gi,
        "$1$2[redacted]",
      ),
    home,
  );
}

/**
 * Replaces the user's home directory with `~`. A path that only starts with the
 * same characters (`/Users/alice2`) is another directory and stays. A
 * Windows-style home matches case-insensitively with `/`, `\` or `\\`
 * (JSON-escaped) for each separator.
 */
function redactHomePrefix(value: string, home: string): string {
  const trimmed = home.replace(/[\\/]+$/, "");
  if (trimmed.length === 0) {
    return value;
  }
  const windowsStyle = trimmed.includes("\\") || trimmed[1] === ":";
  const pattern = Array.from(trimmed)
    .map((char) =>
      windowsStyle && (char === "/" || char === "\\")
        ? String.raw`(?:\\\\|[\\/])`
        : char.replace(/[.*+?^${}()|[\]\\/-]/g, String.raw`\$&`),
    )
    .join("");
  const name = String.raw`[\w\u0080-\uffff-]`;
  return value.replace(
    new RegExp(
      String.raw`(?<![\w\u0080-\uffff.~-])(?:(?<![\\/])|(?<=:\/\/))${pattern}(?!${name}|\.${name})`,
      windowsStyle ? "gi" : "g",
    ),
    "~",
  );
}

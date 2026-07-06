import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export function resolveRepoRoot(importMetaUrl) {
  let currentDir = path.dirname(fileURLToPath(importMetaUrl));

  while (true) {
    if (fs.existsSync(path.join(currentDir, "Cargo.toml")) && fs.existsSync(path.join(currentDir, "package.json"))) {
      return currentDir;
    }

    const parentDir = path.dirname(currentDir);
    if (parentDir === currentDir) {
      throw new Error(`Could not resolve repository root from ${fileURLToPath(importMetaUrl)}`);
    }
    currentDir = parentDir;
  }
}

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";

export function normalizePath(filePath) {
  return filePath.replaceAll(path.sep, "/");
}

export function fileSha256(filePath) {
  return crypto.createHash("sha256").update(fs.readFileSync(filePath)).digest("hex");
}

export function writeJson(destination, value) {
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(destination, `${JSON.stringify(value, null, 2)}\n`, "utf8");
}

export function readJson(jsonPath) {
  return JSON.parse(fs.readFileSync(jsonPath, "utf8"));
}

export function readArgValue(args, index, flag) {
  const value = args[index];
  if (!value || value.startsWith("--")) {
    throw new Error(`Missing value for ${flag}`);
  }
  return value;
}

export function listRelativeFiles(rootDir) {
  const files = [];
  collectFiles(rootDir, rootDir, files);
  return files.sort();
}

function collectFiles(rootDir, currentDir, files) {
  for (const entry of fs.readdirSync(currentDir, { withFileTypes: true }).sort(compareDirentNames)) {
    const fullPath = path.join(currentDir, entry.name);
    const relativePath = normalizePath(path.relative(rootDir, fullPath));

    if (entry.isDirectory()) {
      collectFiles(rootDir, fullPath, files);
      continue;
    }

    if (entry.isFile()) {
      files.push(relativePath);
    }
  }
}

export function findSymlinks(rootDir) {
  const symlinks = [];
  collectSymlinks(rootDir, rootDir, symlinks);
  return symlinks.sort();
}

function collectSymlinks(rootDir, currentDir, symlinks) {
  for (const entryName of fs.readdirSync(currentDir).sort()) {
    const fullPath = path.join(currentDir, entryName);
    const relativePath = normalizePath(path.relative(rootDir, fullPath));
    const stat = fs.lstatSync(fullPath);

    if (stat.isSymbolicLink()) {
      symlinks.push(relativePath);
      continue;
    }

    if (stat.isDirectory()) {
      collectSymlinks(rootDir, fullPath, symlinks);
    }
  }
}

export function assertNoSymlinks(rootDir, context) {
  const symlinks = findSymlinks(rootDir);
  if (symlinks.length > 0) {
    throw new Error(`${context} contains symlinks:\n${symlinks.map((entry) => `- ${entry}`).join("\n")}`);
  }
}

export function commandExists(command) {
  try {
    if (process.platform === "win32") {
      execFileSync("where.exe", [command], { stdio: "ignore", windowsHide: true });
    } else {
      execFileSync("sh", ["-c", `command -v "$1" >/dev/null 2>&1`, "sh", command], {
        stdio: "ignore",
        windowsHide: true
      });
    }
    return true;
  } catch {
    return false;
  }
}

export function requireCommand(command, purpose) {
  if (!commandExists(command)) {
    throw new Error(`Missing required command '${command}' for ${purpose}`);
  }
}

export function createTarGzArchive({ sourceParent, rootName, destination }) {
  requireCommand("tar", "creating tar.gz install archives");
  fs.rmSync(destination, { force: true });

  if (tarSupportsDeterministicOptions()) {
    const deterministicArgs = [
      "--sort=name",
      "--mtime=@0",
      "--owner=0",
      "--group=0",
      "--numeric-owner",
      "-czf",
      destination,
      rootName
    ];

    execFileSync("tar", deterministicArgs, {
      cwd: sourceParent,
      stdio: "inherit",
      windowsHide: true
    });
    return;
  }

  execFileSync("tar", ["-czf", destination, rootName], {
    cwd: sourceParent,
    stdio: "inherit",
    windowsHide: true
  });
}

export function createZipArchive({ sourceParent, rootName, destination }) {
  fs.rmSync(destination, { force: true });

  if (commandExists("zip")) {
    execFileSync("zip", ["-X", "-r", destination, rootName], {
      cwd: sourceParent,
      stdio: "inherit",
      windowsHide: true
    });
    return;
  }

  if (process.platform === "win32" && commandExists("powershell.exe")) {
    execFileSync(
      "powershell.exe",
      [
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        `Compress-Archive -LiteralPath ${powerShellSingleQuote(
          path.join(sourceParent, rootName)
        )} -DestinationPath ${powerShellSingleQuote(destination)} -Force`
      ],
      { stdio: "inherit", windowsHide: true }
    );
    return;
  }

  throw new Error(
    "Missing required command 'zip' for creating Windows install archives. " +
      "Install zip, or run this script on Windows where Compress-Archive is available."
  );
}

export function extractArchive({ archivePath, destinationDir }) {
  fs.rmSync(destinationDir, { recursive: true, force: true });
  fs.mkdirSync(destinationDir, { recursive: true });

  if (archivePath.endsWith(".tar.gz")) {
    requireCommand("tar", "extracting tar.gz install archives");
    execFileSync("tar", ["-xzf", archivePath, "-C", destinationDir], {
      stdio: "inherit",
      windowsHide: true
    });
    return;
  }

  if (archivePath.endsWith(".zip")) {
    if (commandExists("unzip")) {
      execFileSync("unzip", ["-q", archivePath, "-d", destinationDir], {
        stdio: "inherit",
        windowsHide: true
      });
      return;
    }

    if (process.platform === "win32" && commandExists("powershell.exe")) {
      execFileSync(
        "powershell.exe",
        [
          "-NoProfile",
          "-ExecutionPolicy",
          "Bypass",
          "-Command",
          `Expand-Archive -LiteralPath ${powerShellSingleQuote(
            archivePath
          )} -DestinationPath ${powerShellSingleQuote(destinationDir)} -Force`
        ],
        { stdio: "inherit", windowsHide: true }
      );
      return;
    }

    throw new Error(
      "Missing required command 'unzip' for extracting Windows install archives. " +
        "Install unzip, or run this script on Windows where Expand-Archive is available."
    );
  }

  throw new Error(`Unsupported archive format: ${archivePath}`);
}

export function findSingleTopLevelDirectory(extractDir) {
  const entries = fs.readdirSync(extractDir, { withFileTypes: true });
  const dirs = entries.filter((entry) => entry.isDirectory()).map((entry) => entry.name);
  const otherEntries = entries.filter((entry) => !entry.isDirectory()).map((entry) => entry.name);

  if (dirs.length !== 1 || otherEntries.length !== 0) {
    throw new Error(
      `Expected exactly one top-level directory in ${extractDir}; ` +
        `dirs=${JSON.stringify(dirs)}, files=${JSON.stringify(otherEntries)}`
    );
  }

  return path.join(extractDir, dirs[0]);
}

function compareDirentNames(left, right) {
  return left.name.localeCompare(right.name);
}

function powerShellSingleQuote(value) {
  return `'${String(value).replaceAll("'", "''")}'`;
}

function tarSupportsDeterministicOptions() {
  try {
    const output = execFileSync("tar", ["--version"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
      windowsHide: true
    });
    return output.includes("GNU tar");
  } catch {
    return false;
  }
}

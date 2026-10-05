import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const SESSION_LIMIT_MS = 30_000;
const INTERRUPT_REPEAT_MS = 50;

// Runs POSIX shell source as the only foreground process of a pseudo-terminal, the way a
// user's terminal runs the installer, and resolves with its exit status and terminal output.
// `onOutput(output, terminal)` sees the accumulated output after every chunk and may answer it.
export async function runTerminalSession(source, onOutput) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-terminal-"));
  const sourcePath = path.join(directory, "session.sh");
  fs.writeFileSync(sourcePath, source);
  const env = { ...process.env, TERM: "xterm", LC_ALL: "C", NO_COLOR: "1", SHELL: "/bin/sh" };
  delete env.CI;
  // `exec` replaces the shell that `script` starts, so no wrapper process shares the terminal:
  // a wrapper would receive the same terminal signals and could end the session, or relay a
  // second signal, while the shell under test is still cleaning up.
  const child = spawn("script", ["-q", "-e", "-c", `exec sh '${sourcePath}'`, "/dev/null"], { env });
  let output = "";
  let interruptTimer;
  let limit;
  let timedOut = false;
  const terminal = {
    type(text) {
      child.stdin.write(text);
    },
    // The shell runs its INT trap only once its key reader returns, and a Ctrl-C that arrives
    // while that reader is still starting never reaches it. Keep pressing until the session ends.
    interrupt() {
      if (interruptTimer) return;
      child.stdin.write("\u0003");
      interruptTimer = setInterval(() => child.stdin.write("\u0003"), INTERRUPT_REPEAT_MS);
    },
  };

  try {
    return await new Promise((resolve, reject) => {
      limit = setTimeout(() => {
        timedOut = true;
        child.kill("SIGKILL");
      }, SESSION_LIMIT_MS);
      const collect = (chunk) => {
        output += chunk;
        onOutput(output, terminal);
      };
      child.on("error", reject);
      // Keys typed after the session ended have no reader; the status and output decide the result.
      child.stdin.on("error", (error) => {
        if (error.code !== "EPIPE") reject(error);
      });
      child.stdout.on("data", collect);
      child.stderr.on("data", collect);
      child.on("close", (status) => {
        child.stdin.end();
        if (timedOut) {
          reject(new Error(`terminal session did not end within ${SESSION_LIMIT_MS}ms\n${output}`));
        } else {
          resolve({ status, output });
        }
      });
    });
  } finally {
    clearTimeout(limit);
    clearInterval(interruptTimer);
    fs.rmSync(directory, { recursive: true, force: true });
  }
}

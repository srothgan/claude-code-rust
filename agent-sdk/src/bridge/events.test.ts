import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";

const eventsModuleUrl = new URL("./events.js", import.meta.url).href;

function runWriterInChild(lineBytes: number): Promise<{ code: number | null; stdout: Buffer; stderr: string }> {
  const script = `
    const { writeProtocolEventToStdout } = await import(${JSON.stringify(eventsModuleUrl)});
    writeProtocolEventToStdout(JSON.stringify({ event: "settings_result", padding: "x".repeat(${lineBytes}) }) + "\\n");
  `;
  return new Promise((resolve, reject) => {
    const child = spawn(process.execPath, ["--input-type=module", "-e", script], {
      cwd: fileURLToPath(new URL(".", import.meta.url)),
      stdio: ["ignore", "pipe", "pipe"],
    });
    const chunks: Buffer[] = [];
    let stderr = "";
    child.stdout.on("data", (chunk: Buffer) => chunks.push(chunk));
    child.stderr.on("data", (chunk: Buffer) => {
      stderr += chunk.toString();
    });
    child.on("error", reject);
    child.on("close", (code) => resolve({ code, stdout: Buffer.concat(chunks), stderr }));
  });
}

test("protocol events larger than the pipe buffer reach the reader as one complete line", async () => {
  const { code, stdout, stderr } = await runWriterInChild(1024 * 1024);

  assert.equal(code, 0, stderr);
  const lines = stdout.toString().split("\n");
  assert.equal(lines.length, 2);
  assert.equal(lines[1], "");
  assert.equal(JSON.parse(lines[0]).padding.length, 1024 * 1024);
});

import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { PLATFORM_PACKAGES } from "./npm-package-config.mjs";
import {
  npmPackageVersionExists,
  platformPublishPlan,
  publishPlatformPackages,
} from "./publish-npm-platform-packages.mjs";

test("platformPublishPlan covers only platform packages", () => {
  const plan = platformPublishPlan("dist-pack", "1.2.3");

  assert.equal(plan.length, PLATFORM_PACKAGES.length);
  assert.deepEqual(
    plan.map((entry) => entry.packageName),
    PLATFORM_PACKAGES.map((entry) => entry.packageName),
  );
  assert.equal(plan.some((entry) => entry.packageName === "claude-code-rust"), false);
});

test("npmPackageVersionExists returns true when npm reports the requested version", () => {
  const calls = [];

  const exists = npmPackageVersionExists("@scope/pkg", "1.2.3", {
    execNpm(args) {
      calls.push(args);
      return '"1.2.3"\n';
    },
  });

  assert.equal(exists, true);
  assert.deepEqual(calls, [["view", "@scope/pkg@1.2.3", "version", "--json"]]);
});

test("npmPackageVersionExists returns false for missing package versions", () => {
  const exists = npmPackageVersionExists("@scope/pkg", "1.2.3", {
    execNpm() {
      throw npmError({ stderr: "npm error code E404\nNo match found for version 1.2.3" });
    },
  });

  assert.equal(exists, false);
});

test("npmPackageVersionExists rejects unexpected npm lookup failures", () => {
  assert.throws(
    () =>
      npmPackageVersionExists("@scope/pkg", "1.2.3", {
        execNpm() {
          throw npmError({ stderr: "npm error code E500\nregistry unavailable" });
        },
      }),
    /Could not inspect npm package state for @scope\/pkg@1\.2\.3/,
  );
});

test("publishPlatformPackages skips already published versions and publishes missing packages", () => {
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "claude-rs-publish-"));
  try {
    const packDir = path.join(tempDir, "dist-pack");
    const ledgerDir = path.join(tempDir, "ledger");
    const version = "1.2.3";
    fs.mkdirSync(packDir, { recursive: true });

    const plan = platformPublishPlan(packDir, version);
    for (const entry of plan) {
      fs.writeFileSync(entry.tarball, "package bytes", "utf8");
    }

    const alreadyPublished = plan[0].packageName;
    const published = [];

    publishPlatformPackages({
      packDir,
      ledgerDir,
      version,
      now: () => "2026-07-06T00:00:00.000Z",
      execNpm(args) {
        if (args[0] === "view") {
          return args[1] === `${alreadyPublished}@${version}`
            ? `"${version}"\n`
            : (() => {
                throw npmError({ stderr: "npm error code E404\nNo match found" });
              })();
        }
        if (args[0] === "publish") {
          published.push(path.basename(args[1]));
          return "";
        }
        throw new Error(`unexpected npm command: ${args.join(" ")}`);
      },
    });

    assert.deepEqual(
      published,
      plan.slice(1).map((entry) => path.basename(entry.tarball)),
    );

    const ledger = JSON.parse(fs.readFileSync(path.join(ledgerDir, `platform-packages-${version}.json`), "utf8"));
    assert.equal(ledger.entries[0].status, "already_published");
    assert.deepEqual(
      ledger.entries.slice(1).map((entry) => entry.status),
      plan.slice(1).map(() => "published"),
    );
    assert.equal(
      ledger.entries.some(
        (entry) => "local_npm_pack_integrity" in entry || "registry_dist_integrity" in entry,
      ),
      false,
    );
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

function npmError({ stdout = "", stderr = "" }) {
  const error = new Error("npm failed");
  error.stdout = Buffer.from(stdout, "utf8");
  error.stderr = Buffer.from(stderr, "utf8");
  return error;
}

#!/usr/bin/env node
"use strict";

// Unit tests for bin.js helper functions.
// Run: node npm/bin.test.js

const { platform, arch, homedir } = require("os");
const { join } = require("path");
const assert = require("assert");

// We can't require bin.js directly since it calls main() on load.
// Instead, test the logic by reimplementing the pure helpers and verifying
// the source file contains the expected patterns.

const fs = require("fs");
const binSource = fs.readFileSync(join(__dirname, "bin.js"), "utf8");

// --- platformSuffix tests ---

function platformSuffix() {
  const os = platform();
  const cpu = arch();
  if (os === "linux" && cpu === "x64") return "linux-x86_64";
  if (os === "linux" && cpu === "arm64") return "linux-aarch64";
  if (os === "darwin" && cpu === "x64") return "macos-x86_64";
  if (os === "darwin" && cpu === "arm64") return "macos-aarch64";
  if (os === "win32") return "windows-x86_64";
  return null;
}

function installDir() {
  return join(homedir(), ".simard", "bin");
}

function binaryPath() {
  const name = platform() === "win32" ? "simard.exe" : "simard";
  return join(installDir(), name);
}

let passed = 0;
let failed = 0;

function test(name, fn) {
  try {
    fn();
    passed++;
    console.log(`  PASS: ${name}`);
  } catch (err) {
    failed++;
    console.error(`  FAIL: ${name}`);
    console.error(`    ${err.message}`);
  }
}

console.log("npm/bin.js tests\n");

test("platformSuffix returns non-null on supported platforms", () => {
  const suffix = platformSuffix();
  assert.ok(suffix !== null, `expected non-null suffix, got ${suffix}`);
});

test("platformSuffix has os-arch format", () => {
  const suffix = platformSuffix();
  assert.ok(suffix.includes("-"), `expected dash in suffix: ${suffix}`);
  const parts = suffix.split("-");
  assert.strictEqual(parts.length, 2);
  assert.ok(
    ["linux", "macos", "windows"].includes(parts[0]),
    `unexpected OS: ${parts[0]}`
  );
  assert.ok(
    ["x86_64", "aarch64"].includes(parts[1]),
    `unexpected arch: ${parts[1]}`
  );
});

test("installDir ends with .simard/bin", () => {
  const dir = installDir();
  assert.ok(
    dir.endsWith(join(".simard", "bin")),
    `expected .simard/bin suffix: ${dir}`
  );
});

test("binaryPath ends with simard or simard.exe", () => {
  const bp = binaryPath();
  const name = require("path").basename(bp);
  assert.ok(
    name === "simard" || name === "simard.exe",
    `unexpected binary name: ${name}`
  );
});

test("binaryPath is inside installDir", () => {
  const bp = binaryPath();
  const dir = installDir();
  assert.ok(bp.startsWith(dir), `${bp} should be inside ${dir}`);
});

test("package.json version matches bin.js PKG_VERSION source", () => {
  const pkg = require("./package.json");
  assert.ok(pkg.version, "package.json should have a version");
  assert.ok(
    binSource.includes('require("./package.json").version'),
    "bin.js should read version from package.json"
  );
});

test("package.json has correct bin entry", () => {
  const pkg = require("./package.json");
  assert.strictEqual(pkg.bin.simard, "bin.js");
});

test("package.json name is @rysweet/simard", () => {
  const pkg = require("./package.json");
  assert.strictEqual(pkg.name, "@rysweet/simard");
});

test("bin.js uses correct GITHUB_REPO", () => {
  assert.ok(
    binSource.includes('"rysweet/Simard"'),
    "bin.js should reference rysweet/Simard"
  );
});

test("bin.js passes through process.argv", () => {
  assert.ok(
    binSource.includes("process.argv.slice(2)"),
    "bin.js should pass through CLI args"
  );
});

test("bin.js downloads on first run when binary missing", () => {
  // Verify the download-on-miss logic exists
  assert.ok(
    binSource.includes("!existsSync(bin)"),
    "bin.js should check if binary exists"
  );
  assert.ok(
    binSource.includes("download(bin)"),
    "bin.js should call download when missing"
  );
});

test("bin.js cleans up archive after download", () => {
  assert.ok(
    binSource.includes("unlinkSync(tmp)"),
    "bin.js should clean up downloaded archive"
  );
});

console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed > 0 ? 1 : 0);

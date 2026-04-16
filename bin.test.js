#!/usr/bin/env node
"use strict";

// Unit tests for bin.js npx wrapper.
// Validates gh release download approach for private repos.
// Run: node bin.test.js

const { platform, arch, homedir } = require("os");
const { join } = require("path");
const assert = require("assert");
const fs = require("fs");

const binSource = fs.readFileSync(join(__dirname, "bin.js"), "utf8");

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

// --- Helper reimplementations for pure-function testing ---

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

function assetName() {
  const suffix = platformSuffix();
  if (!suffix) return null;
  return `simard-${suffix}.tar.gz`;
}

// --- Tests ---

console.log("bin.js tests\n");

// == Pure helper tests ==

test("platformSuffix returns non-null on supported platforms", () => {
  const suffix = platformSuffix();
  assert.ok(suffix !== null, `expected non-null suffix, got ${suffix}`);
});

test("platformSuffix has os-arch format", () => {
  const suffix = platformSuffix();
  const parts = suffix.split("-");
  assert.strictEqual(parts.length, 2);
  assert.ok(["linux", "macos", "windows"].includes(parts[0]));
  assert.ok(["x86_64", "aarch64"].includes(parts[1]));
});

test("installDir ends with .simard/bin", () => {
  const dir = installDir();
  assert.ok(dir.endsWith(join(".simard", "bin")));
});

test("binaryPath ends with simard or simard.exe", () => {
  const bp = binaryPath();
  const name = require("path").basename(bp);
  assert.ok(name === "simard" || name === "simard.exe");
});

test("binaryPath is inside installDir", () => {
  assert.ok(binaryPath().startsWith(installDir()));
});

test("assetName produces correct tarball name", () => {
  const name = assetName();
  assert.ok(name.startsWith("simard-"), `expected simard- prefix: ${name}`);
  assert.ok(name.endsWith(".tar.gz"), `expected .tar.gz suffix: ${name}`);
});

// == Source-level contract tests ==

test("uses gh release download (not gh api --output)", () => {
  assert.ok(
    binSource.includes('"release", "download"'),
    "bin.js should use gh release download"
  );
  assert.ok(
    !binSource.includes("gh api") && !binSource.includes('"api"'),
    "bin.js should NOT use gh api"
  );
  assert.ok(
    !binSource.includes("--output"),
    "bin.js should NOT use --output flag"
  );
});

test("gh release download uses --repo flag", () => {
  assert.ok(
    binSource.includes('"--repo"'),
    "gh release download should specify --repo"
  );
  assert.ok(
    binSource.includes('"rysweet/Simard"'),
    "should target rysweet/Simard repo"
  );
});

test("gh release download uses --pattern flag for asset selection", () => {
  assert.ok(
    binSource.includes('"--pattern"'),
    "gh release download should use --pattern to select asset"
  );
});

test("gh release download uses --dir flag for output directory", () => {
  assert.ok(
    binSource.includes('"--dir"'),
    "gh release download should use --dir for output location"
  );
});

test("gh release download uses --clobber for idempotent downloads", () => {
  assert.ok(
    binSource.includes('"--clobber"'),
    "gh release download should use --clobber to overwrite existing"
  );
});

test("falls back to curl for public access", () => {
  assert.ok(
    binSource.includes("curl"),
    "should use curl when gh fails"
  );
  assert.ok(
    binSource.includes("releases/latest/download"),
    "curl alternative should use GitHub releases URL"
  );
});

test("extracts tarball with tar xzf", () => {
  assert.ok(
    binSource.includes('"tar"') && binSource.includes('"xzf"'),
    "should extract with tar xzf"
  );
});

test("cleans up tarball after extraction", () => {
  assert.ok(
    binSource.includes("unlinkSync(tarball)"),
    "should clean up tarball after extraction"
  );
});

test("verifies binary exists after download", () => {
  assert.ok(
    binSource.includes("!existsSync(binPath)"),
    "should verify binary exists post-download"
  );
  assert.ok(
    binSource.includes("binary not found"),
    "should report error if binary missing after download"
  );
});

test("sets chmod 755 on non-Windows", () => {
  assert.ok(
    binSource.includes("chmodSync(binPath, 0o755)"),
    "should set execute permissions"
  );
  assert.ok(
    binSource.includes('platform() !== "win32"'),
    "chmod should be conditional on non-Windows"
  );
});

test("passes through process.argv to binary", () => {
  assert.ok(
    binSource.includes("process.argv.slice(2)"),
    "should pass CLI args through to simard binary"
  );
});

test("supports install subcommand", () => {
  assert.ok(
    binSource.includes('"install"'),
    "should support install subcommand"
  );
});

test("auto-downloads when binary missing", () => {
  assert.ok(
    binSource.includes("!existsSync(bin)") && binSource.includes("download(bin)"),
    "should auto-download when binary is missing"
  );
});

test("package.json has correct bin entry pointing to bin.js", () => {
  const pkg = JSON.parse(fs.readFileSync(join(__dirname, "package.json"), "utf8"));
  assert.strictEqual(pkg.bin.simard, "bin.js");
});

test("package.json name is @rysweet/simard", () => {
  const pkg = JSON.parse(fs.readFileSync(join(__dirname, "package.json"), "utf8"));
  assert.strictEqual(pkg.name, "@rysweet/simard");
});

// == Summary ==
console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed > 0 ? 1 : 0);

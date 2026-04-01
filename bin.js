#!/usr/bin/env node
"use strict";

const { existsSync, mkdirSync, chmodSync, unlinkSync } = require("fs");
const { execFileSync } = require("child_process");
const { join } = require("path");
const { homedir, platform, arch } = require("os");

const GITHUB_REPO = "rysweet/Simard";

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

function findLatestAssetUrl() {
  const suffix = platformSuffix();
  if (!suffix) {
    console.error(`Unsupported platform: ${platform()}-${arch()}`);
    process.exit(1);
  }
  const asset = `simard-${suffix}.tar.gz`;

  // Use gh CLI for authenticated access (works with private repos)
  try {
    const json = execFileSync("gh", [
      "api", `repos/${GITHUB_REPO}/releases/latest`,
      "--jq", `.assets[] | select(.name == "${asset}") | .url`
    ], { encoding: "utf8" }).trim();
    if (json) return json;
  } catch (_) {}

  // Fallback: try the public redirect URL
  return `https://github.com/${GITHUB_REPO}/releases/latest/download/${asset}`;
}

function download(binPath) {
  const dir = installDir();
  mkdirSync(dir, { recursive: true });
  const tmp = join(dir, "simard-download.tar.gz");

  const url = findLatestAssetUrl();
  console.error(`Downloading simard from ${GITHUB_REPO}...`);

  try {
    if (url.startsWith("https://api.github.com/")) {
      // GitHub API URL — need Accept header for binary download
      execFileSync("gh", [
        "api", url,
        "-H", "Accept: application/octet-stream",
        "--output", tmp
      ], { stdio: "inherit" });
    } else {
      execFileSync("curl", [
        "-sS", "-L", "--connect-timeout", "15", "--max-time", "120",
        "-o", tmp, url
      ], { stdio: "inherit" });
    }
    execFileSync("tar", ["xzf", tmp, "-C", dir], { stdio: "inherit" });
    if (platform() !== "win32") chmodSync(binPath, 0o755);
  } finally {
    try { unlinkSync(tmp); } catch (_) {}
  }

  if (!existsSync(binPath)) {
    console.error(`Download succeeded but binary not found at ${binPath}`);
    process.exit(1);
  }
}

// If first arg is "install", just download and exit
if (process.argv[2] === "install") {
  const bin = binaryPath();
  console.error(`Installing simard to ${bin}...`);
  download(bin);
  console.error(`Installed: ${bin}`);
  process.exit(0);
}

// Normal mode: ensure binary exists, then passthrough
const bin = binaryPath();
if (!existsSync(bin)) download(bin);

try {
  execFileSync(bin, process.argv.slice(2), { stdio: "inherit" });
} catch (err) {
  process.exit(err.status || 1);
}

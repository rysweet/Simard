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

function installDir() { return join(homedir(), ".simard", "bin"); }

function binaryPath() {
  return join(installDir(), platform() === "win32" ? "simard.exe" : "simard");
}

function assetName() {
  const suffix = platformSuffix();
  if (!suffix) { console.error(`Unsupported platform: ${platform()}-${arch()}`); process.exit(1); }
  return `simard-${suffix}.tar.gz`;
}

function download(binPath) {
  const dir = installDir();
  mkdirSync(dir, { recursive: true });
  const asset = assetName();
  console.error(`Downloading simard from ${GITHUB_REPO}...`);

  try {
    execFileSync("gh", ["release", "download", "--repo", GITHUB_REPO, "--pattern", asset, "--dir", dir, "--clobber"], { stdio: "inherit" });
  } catch (_) {
    const url = `https://github.com/${GITHUB_REPO}/releases/latest/download/${asset}`;
    execFileSync("curl", ["-sS", "-L", "--fail", "-o", join(dir, asset), url], { stdio: "inherit" });
  }

  const tarball = join(dir, asset);
  try {
    execFileSync("tar", ["xzf", tarball, "-C", dir], { stdio: "inherit" });
    if (platform() !== "win32") chmodSync(binPath, 0o755);
  } finally {
    try { unlinkSync(tarball); } catch (_) {}
  }

  if (!existsSync(binPath)) { console.error(`binary not found at ${binPath}`); process.exit(1); }
}

if (process.argv[2] === "install") {
  const bin = binaryPath();
  console.error(`Installing simard to ${bin}...`);
  download(bin);
  console.error(`Installed: ${bin}`);
  process.exit(0);
}

const bin = binaryPath();
if (!existsSync(bin)) download(bin);
try { execFileSync(bin, process.argv.slice(2), { stdio: "inherit" }); }
catch (err) { process.exit(err.status || 1); }

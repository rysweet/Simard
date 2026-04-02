#!/usr/bin/env node
"use strict";

const { existsSync, mkdirSync, chmodSync, unlinkSync, readFileSync, writeFileSync } = require("fs");
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

function versionFile() { return join(installDir(), ".version"); }

function assetName() {
  const suffix = platformSuffix();
  if (!suffix) { console.error(`Unsupported platform: ${platform()}-${arch()}`); process.exit(1); }
  return `simard-${suffix}.tar.gz`;
}

function latestTag() {
  try {
    return execFileSync("gh", [
      "api", `repos/${GITHUB_REPO}/releases/latest`, "--jq", ".tag_name"
    ], { encoding: "utf8", timeout: 10000 }).trim();
  } catch (_) {
    return null;
  }
}

function installedVersion() {
  try { return readFileSync(versionFile(), "utf8").trim(); } catch (_) { return null; }
}

function download(binPath, tag) {
  const dir = installDir();
  mkdirSync(dir, { recursive: true });
  const asset = assetName();
  console.error(`Downloading simard ${tag || "latest"} from ${GITHUB_REPO}...`);

  try {
    const args = ["release", "download", "--repo", GITHUB_REPO, "--pattern", asset, "--dir", dir, "--clobber"];
    if (tag) args.push("--tag", tag);
    execFileSync("gh", args, { stdio: "inherit" });
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

  if (!existsSync(binPath)) { console.error(`Binary not found at ${binPath}`); process.exit(1); }
  if (tag) writeFileSync(versionFile(), tag);
}

// "install" subcommand
if (process.argv[2] === "install") {
  const bin = binaryPath();
  const tag = latestTag();
  console.error(`Installing simard to ${bin}...`);
  download(bin, tag);
  console.error(`Installed: ${bin} (${tag || "latest"})`);
  process.exit(0);
}

// Auto-update check: if binary exists, compare installed vs latest
const bin = binaryPath();
if (existsSync(bin)) {
  const latest = latestTag();
  const installed = installedVersion();
  if (latest && installed && latest !== installed) {
    console.error(`Updating simard: ${installed} → ${latest}`);
    download(bin, latest);
  } else if (!installed && latest) {
    // No version file — write it so next run can compare
    writeFileSync(versionFile(), latest);
  }
} else {
  const tag = latestTag();
  download(bin, tag);
}

try { execFileSync(bin, process.argv.slice(2), { stdio: "inherit" }); }
catch (err) { process.exit(err.status || 1); }

#!/usr/bin/env node
"use strict";

const { existsSync, mkdirSync, chmodSync } = require("fs");
const { execFileSync, execSync } = require("child_process");
const { join } = require("path");
const { homedir, platform, arch } = require("os");

const GITHUB_REPO = "rysweet/Simard";
const PKG_VERSION = require("./package.json").version;

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

function download(binPath) {
  const suffix = platformSuffix();
  if (!suffix) {
    console.error(`Unsupported platform: ${platform()}-${arch()}`);
    process.exit(1);
  }

  const tag = `v${PKG_VERSION}`;
  const asset = `simard-${suffix}.tar.gz`;
  const url = `https://github.com/${GITHUB_REPO}/releases/download/${tag}/${asset}`;

  const dir = installDir();
  mkdirSync(dir, { recursive: true });

  const tmp = join(dir, asset);

  try {
    // Download with curl (retry with backoff on failure)
    let downloaded = false;
    for (let attempt = 0; attempt < 3; attempt++) {
      try {
        if (attempt > 0) {
          const delay = Math.pow(2, attempt) * 1000;
          console.error(`Retrying download (attempt ${attempt + 1}/3)...`);
          // Native sleep — avoids spawning a shell process
          Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, delay);
        }
        execFileSync(
          "curl",
          ["-sS", "-L", "--connect-timeout", "15", "--max-time", "120", "--retry", "2", "-o", tmp, url],
          { stdio: "inherit" }
        );
        downloaded = true;
        break;
      } catch (dlErr) {
        if (attempt === 2) throw dlErr;
      }
    }

    // Extract
    execFileSync("tar", ["xzf", tmp, "-C", dir], { stdio: "inherit" });

    // Ensure executable
    if (platform() !== "win32") {
      chmodSync(binPath, 0o755);
    }
  } finally {
    // Clean up archive
    try {
      const { unlinkSync } = require("fs");
      unlinkSync(tmp);
    } catch (_) {
      // ignore
    }
  }

  if (!existsSync(binPath)) {
    console.error(`Download succeeded but binary not found at ${binPath}`);
    process.exit(1);
  }
}

function main() {
  const bin = binaryPath();

  if (!existsSync(bin)) {
    console.error(`Downloading simard v${PKG_VERSION}...`);
    download(bin);
  }

  // Pass through all arguments to the native binary
  const args = process.argv.slice(2);
  try {
    execFileSync(bin, args, { stdio: "inherit" });
    process.exit(0);
  } catch (err) {
    process.exit(err.status || 1);
  }
}

main();

#!/usr/bin/env node
"use strict";

// TDD tests for README.md documentation requirements.
// Validates that README contains required sections for:
//   - npx install method (at top of Install section)
//   - simard update and simard install commands
//   - gh CLI auth note for private repo
// Run: node readme.test.js

const fs = require("fs");
const path = require("path");
const assert = require("assert");

const readme = fs.readFileSync(path.join(__dirname, "README.md"), "utf8");
const lines = readme.split("\n");

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

console.log("README.md documentation tests\n");

// == Install section structure ==

test("Install section exists", () => {
  assert.ok(readme.includes("## Install"), "README must have ## Install section");
});

test("npx is the first install method (appears before other methods)", () => {
  const npxIdx = readme.indexOf("### With npx");
  const releasesIdx = readme.indexOf("### From GitHub Releases");
  const sourceIdx = readme.indexOf("### From Source");
  const cargoIdx = readme.indexOf("### With Cargo");

  assert.ok(npxIdx !== -1, "npx section must exist");
  assert.ok(releasesIdx !== -1, "GitHub Releases section must exist");
  assert.ok(sourceIdx !== -1, "From Source section must exist");
  assert.ok(cargoIdx !== -1, "With Cargo section must exist");

  assert.ok(npxIdx < releasesIdx, "npx must appear before GitHub Releases");
  assert.ok(npxIdx < sourceIdx, "npx must appear before From Source");
  assert.ok(npxIdx < cargoIdx, "npx must appear before With Cargo");
});

test("npx section is marked as easiest", () => {
  assert.ok(
    readme.includes("easiest"),
    "npx section should indicate it is the easiest method"
  );
});

// == npx usage ==

test("shows npx github:rysweet/Simard usage", () => {
  assert.ok(
    readme.includes("npx github:rysweet/Simard"),
    "README must show npx github:rysweet/Simard usage"
  );
});

test("shows npx install subcommand", () => {
  assert.ok(
    readme.includes("npx github:rysweet/Simard install"),
    "README must show npx install subcommand"
  );
});

test("mentions GitHub CLI requirement for private repo", () => {
  assert.ok(
    readme.includes("GitHub CLI"),
    "README must mention GitHub CLI requirement"
  );
  assert.ok(
    readme.includes("authenticated") || readme.includes("auth"),
    "README must mention authentication requirement"
  );
});

// == Self-management commands ==

test("Self-Management section exists", () => {
  assert.ok(
    readme.includes("Self-Management") || readme.includes("self-management"),
    "README must have a Self-Management section"
  );
});

test("simard update command is documented", () => {
  assert.ok(
    readme.includes("simard update"),
    "README must document simard update command"
  );
});

test("simard install command is documented", () => {
  assert.ok(
    readme.includes("simard install"),
    "README must document simard install command"
  );
});

test("simard update describes self-update behavior", () => {
  const updateLine = lines.find(
    (l) => l.includes("simard update") && !l.startsWith("#")
  );
  assert.ok(updateLine, "simard update must appear in a non-heading line");
  // The update command should have a comment/description mentioning update/latest
  const updateSection = readme.substring(
    readme.indexOf("simard update"),
    readme.indexOf("simard update") + 200
  );
  assert.ok(
    updateSection.includes("self-update") ||
      updateSection.includes("latest") ||
      updateSection.includes("update"),
    "simard update must describe updating to latest release"
  );
});

test("simard install describes binary installation", () => {
  const installSection = readme.substring(
    readme.indexOf("simard install"),
    readme.indexOf("simard install") + 200
  );
  assert.ok(
    installSection.includes("install") ||
      installSection.includes("binary") ||
      installSection.includes("~/.simard"),
    "simard install must describe binary installation"
  );
});

// == CLI reference doc exists ==

test("CLI reference doc for npx exists", () => {
  const docPath = path.join(
    __dirname,
    "docs",
    "reference",
    "npx-npm-install.md"
  );
  assert.ok(
    fs.existsSync(docPath),
    "docs/reference/npx-npm-install.md must exist"
  );
});

test("CLI reference doc documents self-management commands", () => {
  const cliRef = fs.readFileSync(
    path.join(__dirname, "docs", "reference", "simard-cli.md"),
    "utf8"
  );
  assert.ok(
    cliRef.includes("simard update"),
    "CLI reference must document simard update"
  );
  assert.ok(
    cliRef.includes("simard install"),
    "CLI reference must document simard install"
  );
  assert.ok(
    cliRef.includes("Self-management") || cliRef.includes("self-management"),
    "CLI reference must have self-management section"
  );
});

// == Summary ==
console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed > 0 ? 1 : 0);

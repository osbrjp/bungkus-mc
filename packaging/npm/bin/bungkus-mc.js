#!/usr/bin/env node
// Runs the native bungkus-mc binary from the platform package npm
// installed for this machine (@osbrjp/bungkus-mc-<os>-<cpu>).
"use strict";

const { spawnSync } = require("node:child_process");

const pkg = `@osbrjp/bungkus-mc-${process.platform}-${process.arch}`;
let bin;
try {
  bin = require.resolve(`${pkg}/bin/bungkus-mc`);
} catch {
  console.error(
    `bungkus-mc: no binary for ${process.platform}/${process.arch} (${pkg} is not installed).\n` +
      "Supported: macOS and Linux on arm64 or x64. Reinstall without --no-optional.",
  );
  process.exit(1);
}

const result = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  console.error(`bungkus-mc: ${result.error.message}`);
  process.exit(1);
}
if (result.signal) {
  process.kill(process.pid, result.signal);
}
process.exit(result.status ?? 1);

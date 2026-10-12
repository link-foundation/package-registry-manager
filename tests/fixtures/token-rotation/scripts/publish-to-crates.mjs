import { spawnSync } from "node:child_process";

const cargoCommand = process.env.START_CARGO_COMMAND || "cargo";
const cargoArgsPrefix = [];
spawnSync(
  cargoCommand,
  [
    ...cargoArgsPrefix,
    "publish",
    "--manifest-path",
    "Cargo.toml",
  ],
  { env: process.env },
);

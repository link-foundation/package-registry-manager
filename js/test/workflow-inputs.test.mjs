import assert from "node:assert/strict";
import test from "node:test";
import {
  dispatchInputArgs,
  dispatchInputValues,
  workflowDispatchInputs,
} from "../src/workflow-inputs.mjs";

test("a required choice input uses its first option", () => {
  const yaml = `on:
  workflow_dispatch:
    inputs:
      bump_type:
        description: Version bump type
        required: true
        type: choice
        options: [patch, minor, major]
      description:
        required: false
        type: string
`;
  assert.deepEqual(dispatchInputValues(yaml), { bump_type: "patch" });
  assert.deepEqual(dispatchInputArgs(yaml), ["-f", "bump_type=patch"]);
});

test("a declared default wins over the first option", () => {
  const yaml = `"on":
  workflow_dispatch:
    inputs:
      release_mode:
        type: choice
        default: instant
        options: [instant, changeset-pr]
      bump_type:
        type: choice
        options: [patch, minor, major]
`;
  assert.deepEqual(dispatchInputValues(yaml), {
    release_mode: "instant",
    bump_type: "patch",
  });
});

test("workflows without dispatch inputs need no flags", () => {
  assert.deepEqual(workflowDispatchInputs("on: push\n"), {});
  assert.deepEqual(dispatchInputArgs("on: [push, workflow_dispatch]\n"), []);
});

test("a required free-form input without a value is refused", () => {
  const yaml = `on:
  workflow_dispatch:
    inputs:
      confirmation:
        required: true
        type: string
`;
  assert.throws(
    () => dispatchInputValues(yaml),
    /workflow_dispatch requires values for confirmation/,
  );
});

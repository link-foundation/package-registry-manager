import assert from "node:assert/strict";
import { test } from "node:test";
import {
  pythonMatches,
  choosePython,
  probePython,
  applyPython,
} from "../src/python.mjs";

test("checks requires-python stable version specifiers (#33)", () => {
  for (const [spec, version, expected] of [
    [">=3.13", "3.12.9", false],
    [">=3.13", "3.14.0", true],
    [">=3.13,<3.14", "3.14.0", false],
    ["~=3.13.0", "3.13.7", true],
    ["~=3.13.0", "3.14.0", false],
    ["~=3.13", "3.14.0", true],
    ["==3.13.*", "3.13.2", true],
    ["!=3.13.*", "3.13.2", false],
    [">3.13,<=3.14", "3.14.0", true],
    [">=3.13,!=3.13.1", "3.13.1", false],
    ["garbage", "3.13.0", false],
    [">=3.13", "3.14.0rc1", false],
  ]) {
    assert.equal(pythonMatches(version, spec), expected, `${version} ${spec}`);
  }
});

test("chooses an installed compatible interpreter and uses it to build (#33)", () => {
  const tools = [
    { program: "python", version: "3.12.9" },
    { program: "python3.13", version: "3.13.7" },
    { program: "python3.14", version: "3.14.0" },
  ];
  assert.equal(choosePython(tools, ">=3.13").program, "python3.13");
  const plan = {
    registry: "pypi",
    package: { requires_python: ">=3.13" },
    steps: [
      {
        id: "build-package",
        command: { program: "python", args: ["-m", "build"] },
      },
    ],
  };
  applyPython(plan, tools);
  assert.equal(plan.steps[0].command.program, "python3.13");
  assert.equal(plan.prerequisites[0].ok, true);
  const missing = structuredClone(plan);
  applyPython(missing, tools.slice(0, 1));
  assert.equal(missing.prerequisites[0].ok, false);
  assert.match(
    missing.prerequisites[0].required,
    /Install Python satisfying >=3.13/,
  );
});

test("probes candidate executables without running repository code (#33)", async () => {
  const calls = [];
  const tools = await probePython({
    candidates: ["python", "python3.13"],
    probe: async (program, args) => {
      calls.push([program, args]);
      return {
        code: 0,
        stdout: program === "python" ? "Python 3.12.9" : "Python 3.13.7",
      };
    },
  });
  assert.equal(choosePython(tools, ">=3.13").program, "python3.13");
  assert.deepEqual(calls, [
    ["python", ["--version"]],
    ["python3.13", ["--version"]],
  ]);
});

const JAVASCRIPT = /\.(?:[cm]?[jt]s)$/;
const NODE_CALL =
  /\b(?:exec(?:File)?|spawn|run)(?:Sync)?\s*\(\s*(["'])(npm|pnpm|yarn|cargo)\1\s*,\s*/g;

function publishArray(args) {
  const array = /^\[\s*(["'])publish\1\s*(?:,|\])/u.exec(args);
  return (
    array !== null &&
    !args.slice(0, args.indexOf("]") + 1).includes("--dry-run")
  );
}

function nodePublishes(source, registry) {
  const forwards = new Set();
  const functions = [
    ...source.matchAll(/\bfunction\s+(\w+)\s*\(\s*(\w+)[^\n]*?\)\s*\{/g),
  ];
  for (const call of source.matchAll(NODE_CALL)) {
    if ((call[2] === "cargo") !== (registry === "crates-io")) {
      continue;
    }
    const args = source.slice(call.index + call[0].length);
    if (publishArray(args)) {
      return true;
    }
    const enclosing = functions.findLast((item) => item.index < call.index);
    if (
      enclosing &&
      !/^}/m.test(
        source.slice(enclosing.index + enclosing[0].length, call.index),
      ) &&
      new RegExp(`^${enclosing[2]}\\s*[,)]`).test(args)
    ) {
      forwards.add(enclosing[1]);
    }
  }
  // Follow a defaulted callback such as `run = runNpm` as well as calls
  // to the helper itself.
  for (const [, alias, helper] of source.matchAll(
    /\b(\w+)\s*=\s*(\w+)\s*[,)]/g,
  )) {
    if (forwards.has(helper)) {
      forwards.add(alias);
    }
  }
  return [...source.matchAll(/\b(\w+)\s*\(\s*(?=\[)/g)].some(
    (call) =>
      forwards.has(call[1]) &&
      publishArray(source.slice(call.index + call[0].length)),
  );
}

function rustPublishes(source) {
  const command = String.raw`Command::new\s*\(\s*"cargo"\s*\)`;
  const publish = String.raw`\s*\.\s*arg\s*\(\s*"publish"\s*\)`;
  const usable = (statement) => !statement.includes("--dry-run");
  if (
    [...source.matchAll(new RegExp(`${command + publish}[^;]*`, "g"))].some(
      ([statement]) => usable(statement),
    )
  ) {
    return true;
  }
  const bindings = new RegExp(
    String.raw`\blet\s+(?:mut\s+)?(\w+)\s*=\s*` + command + String.raw`\s*;`,
    "g",
  );
  return [...source.matchAll(bindings)].some((binding) => {
    const tail = source.slice(binding.index + binding[0].length);
    const args = [
      ...tail.matchAll(
        new RegExp(
          String.raw`\b` + binding[1] + String.raw`\s*\.\s*args?\s*\([^;]*`,
          "g",
        ),
      ),
    ];
    return (
      args.some(([statement]) =>
        new RegExp(`^${binding[1]}${publish}`).test(statement),
      ) && args.every(([statement]) => usable(statement))
    );
  });
}

/** Recognize argument-vector publish calls in Node and Rust scripts. */
export function publishesWithWrapper(lines, registry, script) {
  if (JAVASCRIPT.test(script) && ["npm", "crates-io"].includes(registry)) {
    return nodePublishes(lines.join("\n"), registry);
  }
  if (script.endsWith(".rs") && registry === "crates-io") {
    return rustPublishes(lines.join("\n"));
  }
  return null;
}

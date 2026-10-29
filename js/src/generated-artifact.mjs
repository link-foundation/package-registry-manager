import path from "node:path";
import { readFile, realpath } from "node:fs/promises";

/** Generate each declared artifact, then pack only the selected validated directory. */
export async function prepareGeneratedArtifact(session) {
  const item = session.plan.package;
  const metadata = item.generated_alias ?? item.generated_artifact;
  if (!metadata?.generator) {
    throw new Error(
      "generated npm artifact has no declared generator; configure it before bootstrap",
    );
  }
  const source = await realpath(
    path.resolve(session.values.worktree, path.dirname(item.manifest)),
  );
  const generator = await realpath(path.resolve(source, metadata.generator));
  if (!generator.startsWith(`${source}${path.sep}`)) {
    throw new Error("npm artifact generator must be inside the source package");
  }
  const destination = path.join(session.values.pack_destination, "generated");
  for (const command of [
    {
      program: "npm",
      args: ["ci", "--ignore-scripts", "--no-audit", "--no-fund"],
    },
    { program: "node", args: [generator, "--output-dir", destination] },
  ]) {
    const result = await session.capture({ command, cwd: source });
    if (result.code !== 0) {
      throw new Error(
        `${command.program} failed while generating npm artifacts`,
      );
    }
  }
  const selected = await realpath(path.resolve(destination, item.name));
  if (!selected.startsWith(`${await realpath(destination)}${path.sep}`)) {
    throw new Error("invalid generated package path");
  }
  const actual = JSON.parse(
    await readFile(path.join(selected, "package.json"), "utf8"),
  );
  if (actual.name !== item.name || actual.version !== item.version) {
    throw new Error(
      "generated artifact identity/version differs from inspection; nothing was published",
    );
  }
  session.values.generated_package = selected;
}

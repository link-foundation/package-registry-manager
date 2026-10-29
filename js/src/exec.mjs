import { execa } from "command-stream/execa";

/** Capture an exact argument vector, including Windows PATHEXT launchers. */
export async function exec(program, args, options = {}) {
  const output = options.mirror ? ["pipe", "inherit"] : "pipe";
  const result = await execa(program, args, {
    cwd: options.cwd,
    env: options.env,
    stdin: options.stdin ?? "ignore",
    stdout: output,
    stderr: output,
    reject: false,
    stripFinalNewline: false,
  });
  return {
    code: result.exitCode ?? 1,
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
  };
}

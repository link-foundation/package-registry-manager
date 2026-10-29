import { test } from "node:test";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

test("generated NuGet job runs the bounded authentication controller regressions", async () => {
  await promisify(execFile)(
    "python3",
    [
      fileURLToPath(
        new URL(
          "../../experiments/issue-73/test_nuget_publish.py",
          import.meta.url,
        ),
      ),
    ],
    { env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" } },
  );
});

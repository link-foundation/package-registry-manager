/** Exact-version registry check for proposed publishing jobs. Errors fail closed. */
export function versionGuard(registry) {
  const metadata =
    registry === "npm"
      ? 'metadata = json.load(open("package.json"))'
      : 'metadata = tomllib.load(open("Cargo.toml", "rb"))["package"]';
  const base =
    registry === "npm"
      ? '"https://registry.npmjs.org/" + urllib.parse.quote(name, safe="") + "/" + version'
      : '"https://crates.io/api/v1/crates/" + urllib.parse.quote(name, safe="") + "/" + version';
  return [
    "      - name: Check exact registry version",
    "        id: version-check",
    "        run: |",
    "          python3 - <<'PY'",
    "          import json, os, time, tomllib, urllib.request, urllib.error, urllib.parse",
    `          ${metadata}`,
    '          name, version = metadata["name"], metadata["version"]',
    '          expected = os.environ.get("RELEASE_VERSION", "").removeprefix("v")',
    '          if expected and version != expected: raise SystemExit("Checkout version does not match release output")',
    `          url = ${base} + "?_=" + str(time.time_ns())`,
    '          request = urllib.request.Request(url, headers={"Cache-Control": "no-cache, no-store", "User-Agent": "package-registry-manager", "Accept": "application/json"})',
    "          try:",
    "              with urllib.request.urlopen(request, timeout=30) as response:",
    "                  json.load(response)",
    "              publish = False",
    "          except urllib.error.HTTPError as error:",
    "              if error.code != 404: raise",
    "              publish = True",
    '          with open(os.environ["GITHUB_OUTPUT"], "a") as output:',
    '              output.write("publish=" + str(publish).lower() + "\\n")',
    "          PY",
  ];
}

//! Uncached exact-version checks for proposed publish jobs.

/// Generate a Python version check; only a confirmed HTTP 404 allows publishing.
#[must_use]
#[allow(clippy::literal_string_with_formatting_args)]
pub fn version_guard(npm: bool) -> Vec<String> {
    let metadata = if npm {
        "metadata = json.load(open(\"package.json\"))"
    } else {
        "metadata = tomllib.load(open(\"Cargo.toml\", \"rb\"))[\"package\"]"
    };
    let base = if npm {
        "\"https://registry.npmjs.org/\""
    } else {
        "\"https://crates.io/api/v1/crates/\""
    };
    let text = r#"      - name: Check exact registry version
        id: version-check
        run: |
          python3 - <<'PY'
          import json, os, time, tomllib, urllib.request, urllib.error, urllib.parse
          {metadata}
          name, version = metadata["name"], metadata["version"]
          expected = os.environ.get("RELEASE_VERSION", "").removeprefix("v")
          if expected and version != expected: raise SystemExit("Checkout version does not match release output")
          url = {base} + urllib.parse.quote(name, safe="") + "/" + version + "?_=" + str(time.time_ns())
          request = urllib.request.Request(url, headers={"Cache-Control": "no-cache, no-store", "User-Agent": "package-registry-manager", "Accept": "application/json"})
          try:
              with urllib.request.urlopen(request, timeout=30) as response:
                  json.load(response)
              publish = False
          except urllib.error.HTTPError as error:
              if error.code != 404: raise
              publish = True
          with open(os.environ["GITHUB_OUTPUT"], "a") as output:
              output.write("publish=" + str(publish).lower() + "\n")
          PY"#;
    text.replace("{metadata}", metadata)
        .replace("{base}", base)
        .lines()
        .map(str::to_owned)
        .collect()
}

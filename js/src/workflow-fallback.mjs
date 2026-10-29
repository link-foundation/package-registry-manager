/** OIDC runs first. A separate retry receives credentials only after that attempt fails. */
export function fallbackSteps(
  step,
  registry,
  names,
  level,
  index,
  removeCredentials,
) {
  const prefix = " ".repeat(level + 2);
  const originalId = step
    .map((line) => /^\s*(?:-\s*)?id:\s*([\w-]+)/.exec(line)?.[1])
    .find(Boolean);
  const id = originalId ?? `prm_oidc_${index + 1}`;
  let oidc = removeCredentials(
    step,
    new Set([...names, ...(registry === "pypi" ? ["password", "user"] : [])]),
  );
  oidc = oidc.filter((line) => !/^\s*continue-on-error:/.test(line));
  if (!originalId) {
    oidc.push(`${prefix}id: ${id}`);
  }
  oidc.push(`${prefix}continue-on-error: true`);
  const title = oidc.findIndex((line) => /^\s*(?:-\s*)?name:/.test(line));
  if (title >= 0) {
    oidc[title] = appendTitle(oidc[title], "OIDC");
  } else {
    oidc.push(`${prefix}name: Publish through OIDC`);
  }
  // Normalize a dash-prefixed id/condition so removing it keeps a valid step.
  const normalized = [...step];
  if (/^\s*-\s+(?:if|id|continue-on-error):/.test(normalized[0])) {
    normalized[0] = normalized[0].replace(/- /, "  ");
    const next = normalized.findIndex(
      (line, index) => index > 0 && line.search(/\S/) === level + 2,
    );
    if (next >= 0) {
      normalized[next] =
        `${" ".repeat(level)}- ${normalized[next].trimStart()}`;
    }
  }
  const fallback = normalized.filter(
    (line) => !/^\s*(?:if|id|continue-on-error):/.test(line),
  );
  // Remove a multiline condition together with its continuation lines.
  const condition = step.findIndex((line) => /^\s*if:/.test(line));
  if (condition >= 0 && /if:\s*[>|]/.test(step[condition])) {
    for (
      let i = condition + 1;
      i < step.length && step[i].search(/\S/) > level + 2;
      i++
    ) {
      const at = fallback.indexOf(step[i]);
      if (at >= 0) {
        fallback.splice(at, 1);
      }
    }
  }
  const fallbackTitle = fallback.findIndex((line) =>
    /^\s*(?:-\s*)?name:/.test(line),
  );
  if (fallbackTitle >= 0) {
    fallback[fallbackTitle] = appendTitle(
      fallback[fallbackTitle],
      "token fallback",
    );
  } else {
    fallback.push(`${prefix}name: Publish with token fallback`);
  }
  fallback.push(
    `${prefix}if: steps.${id}.outcome == 'failure'`,
    `${prefix}id: ${id}_fallback`,
  );
  if (registry === "pypi") {
    const cleaned = removeCredentials(fallback, new Set(["password", "user"]));
    const withIndex = cleaned.findIndex((line) => /^\s*with:\s*$/.test(line));
    const password = `${prefix}  password: \${{ secrets.PYPI_API_TOKEN || secrets.PYPI_TOKEN }}`;
    if (withIndex < 0) {
      cleaned.push(`${prefix}with:`, password);
    } else {
      cleaned.splice(withIndex + 1, 0, password);
    }
    return [oidc, ["", ...cleaned]];
  }
  const env = (lines, value) => {
    const updated = removeCredentials(
      lines,
      new Set([...names, "NODE_AUTH_TOKEN", "NPM_CONFIG_PROVENANCE"]),
    );
    const at = updated.findIndex((line) => /^\s*env:\s*$/.test(line));
    const entries = [...new Set(["NODE_AUTH_TOKEN", ...names])].map(
      (name) => `${prefix}  ${name}: ${value}`,
    );
    entries.push(`${prefix}  NPM_CONFIG_PROVENANCE: 'true'`);
    if (at < 0) {
      updated.push(`${prefix}env:`, ...entries);
    } else {
      updated.splice(at + 1, 0, ...entries);
    }
    return updated;
  };
  oidc = env(oidc, "''");
  return [
    oidc,
    [
      "",
      ...env(
        fallback,
        "${{ secrets.NPM_TOKEN || secrets.NODE_AUTH_TOKEN || secrets.NPM_AUTH_TOKEN }}",
      ),
    ],
  ];
}

function appendTitle(line, suffix) {
  const value = line.slice(line.indexOf("name:") + "name:".length).trimStart();
  const key = line.slice(0, line.length - value.length);
  const quote = value[0];
  return (
    key +
    ((quote === "'" || quote === '"') && value.endsWith(quote)
      ? `${value.slice(0, -1)} (${suffix})${quote}`
      : `${value} (${suffix})`)
  );
}

/** Describe release authentication, independently from the last registry publication. */
export function publishingPolicy(contents, registry, grantOverride) {
  const text = contents.join("\n");
  if (registry === "nuget") {
    const oidc =
      (grantOverride ??
        /id-token:\s*write|permissions:\s*write-all/.test(text)) &&
      /NuGet\/login@/.test(text);
    const fallback = /PRM_NUGET_FALLBACK:\s*\$\{\{\s*secrets\.[\w]+/.test(text);
    return oidc && fallback && /continue-on-error:\s*true/.test(text)
      ? "oidc-with-token-fallback"
      : oidc
        ? "oidc-only"
        : /secrets\.[\w]+/.test(text)
          ? "token-only"
          : "authentication-unverified";
  }
  const token =
    /secrets\.(?:CARGO_(?:REGISTRY_)?TOKEN|NPM_(?:AUTH_)?TOKEN|NODE_AUTH_TOKEN|PYPI_(?:API_)?TOKEN)/.test(
      text,
    );
  const grant =
    grantOverride ?? /id-token:\s*write|permissions:\s*write-all/.test(text);
  const oidc =
    grant &&
    (registry === "crates-io"
      ? text
          .split(/(?=^[ \t]*- )/m)
          .some(
            (step) =>
              /crates-io-auth-action@/.test(step) &&
              !/\bif:[\s\S]*\bvars\./.test(step),
          )
      : registry === "pypi"
        ? /pypa\/gh-action-pypi-publish@/.test(text)
        : /npm publish|npm run|node .*(?:publish|release)/.test(text));
  const fallback =
    registry === "crates-io"
      ? /outputs\.token\s*\|\|\s*secrets\./.test(text) &&
        /continue-on-error:\s*true/.test(text)
      : /outcome == 'failure'/.test(text) &&
        /continue-on-error:\s*true/.test(text);
  const facts = authenticationFacts(text, registry);
  if (facts.passwordAlwaysToken) {
    // `vars.FLAG == 'true' && '' || secrets.TOKEN` always yields the token:
    // '' is falsy in GitHub expressions, so the flag never selects OIDC.
    return token ? "token-only" : "authentication-unverified";
  }
  if (grant && token && facts.gate && facts.shellFallback) {
    return "variable-gated-oidc-with-token-fallback";
  }
  if ((oidc && token && fallback) || (oidc && token && facts.npmNative)) {
    return "oidc-with-token-fallback";
  }
  return oidc
    ? "oidc-only"
    : token
      ? "token-only"
      : "authentication-unverified";
}

/**
 * Actual credential-selection semantics of a publishing job, kept apart from
 * publisher registration and historical trusted uploads.
 */
export function authenticationFacts(contents, registry) {
  const text = Array.isArray(contents) ? contents.join("\n") : contents;
  const facts = {
    gate: null,
    shellFallback: false,
    passwordAlwaysToken: false,
    npmNative: false,
  };
  const steps = text.split(/(?=^[ \t]*- )/m);
  const auth = steps.find((step) =>
    registry === "crates-io"
      ? /crates-io-auth-action@/.test(step)
      : registry === "nuget"
        ? /NuGet\/login@/.test(step)
        : false,
  );
  const gate = auth
    ? /\bif:.*\bvars\.(\w+)\s*==\s*['"]true['"]/.exec(auth)?.[1]
    : null;
  if (gate) {
    facts.gate = { variable: gate, value: "true" };
  }
  facts.shellFallback = /\$\{\w*OIDC\w*:-\$\w+\}/.test(text);
  if (registry === "pypi") {
    const password = /^\s*password\s*:\s*(.+)$/m.exec(text)?.[1] ?? "";
    const flag =
      /vars\.(\w+)\s*==\s*['"]true['"]\s*&&\s*''\s*\|\|\s*secrets\.\w+/.exec(
        password,
      );
    if (flag) {
      facts.passwordAlwaysToken = true;
      facts.gate = { variable: flag[1], value: "true", ineffective: true };
    }
  }
  if (registry === "npm") {
    // npm 11.5.1+ tries the OIDC exchange itself and falls back to the
    // configured token, so a job with both needs no separate fallback step.
    facts.npmNative = steps.some(
      (step) =>
        /\bnpm\s+publish\b/.test(step) &&
        /NODE_AUTH_TOKEN\s*:\s*\$\{\{\s*secrets\./.test(step),
    );
  }
  return facts;
}

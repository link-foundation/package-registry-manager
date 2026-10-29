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
  if (oidc && token && fallback) {
    return "oidc-with-token-fallback";
  }
  return oidc
    ? "oidc-only"
    : token
      ? "token-only"
      : "authentication-unverified";
}

//! `NuGet` publication uses one artifact and temporary, job-local OIDC credentials.
use crate::model::Package;
use anyhow::{bail, Result};

pub fn steps(package: &Package) -> Result<Vec<String>> {
    if !regex::Regex::new(r"^[a-zA-Z0-9_.-]+$")?.is_match(&package.name)
        || ["mypackage", "packagename", "example"].contains(&package.name.to_lowercase().as_str())
    {
        bail!("resolve exact NuGet package identity before proposing publication");
    }
    let mut secrets = package.token_secrets.clone();
    for name in ["NUGET_API_KEY", "NUGET_TOKEN"] {
        if !secrets.iter().any(|existing| existing == name) {
            secrets.push(name.into());
        }
    }
    if secrets.iter().any(|name| {
        !regex::Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$")
            .expect("regex")
            .is_match(name)
    }) {
        bail!("invalid NuGet fallback secret name");
    }
    let project = package
        .manifest
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .replace('\'', "''");
    let mut result = vec![
        "      - uses: actions/setup-dotnet@v6".into(),
        "        with:".into(),
        "          dotnet-version: '10.0.x'".into(),
        format!("      - run: dotnet test '{project}' --configuration Release"),
        format!("      - run: dotnet pack '{project}' --configuration Release --no-build --output .prm-nuget"),
        "      - uses: NuGet/login@8d196754b4036150537f80ac539e15c2f1028841 # v1".into(),
        "        id: prm_nuget_login".into(),
        "        if: vars.NUGET_USER != ''".into(),
        "        continue-on-error: true".into(),
        "        with:".into(),
        "          user: ${{ vars.NUGET_USER }}".into(),
        "      - name: Publish validated NuGet artifact and verify public consumer".into(),
        "        shell: bash".into(),
        "        env:".into(),
        format!("          PRM_PACKAGE_ID: '{}'", package.name),
        "          PRM_NUGET_USER: ${{ vars.NUGET_USER }}".into(),
        "          PRM_NUGET_OIDC: ${{ steps.prm_nuget_login.outputs.NUGET_API_KEY }}".into(),
        format!("          PRM_NUGET_FALLBACK: ${{{{ {} }}}}", secrets.iter().map(|name|format!("secrets.{name}")).collect::<Vec<_>>().join(" || ")),
        "        run: |".into(),
        "          python3 - <<'PRM_NUGET_PY'".into(),
    ];
    result.extend(
        include_str!("nuget-publish.py")
            .trim_end()
            .lines()
            .map(|line| format!("          {line}")),
    );
    result.push("          PRM_NUGET_PY".into());
    Ok(result)
}

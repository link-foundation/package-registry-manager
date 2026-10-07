//! Dedicated browser pages and conservative credential form helpers.
use crate::model::Registry;
use serde_json::json;

/// Registry credential lifecycle metadata and browser configuration.
pub struct TokenProvider {
    /// Value supplied by the registry or its credential adapter.
    pub url: &'static str,
    /// Value supplied by the registry or its credential adapter.
    pub secret: &'static str,
    /// Value supplied by the registry or its credential adapter.
    pub scope: &'static str,
}

/// Build the registry policy or the conservative browser helper.
#[must_use]
pub const fn token_provider(registry: Registry) -> Option<TokenProvider> {
    let (url, secret, scope) = match registry {
        Registry::DockerHub => (
            "https://app.docker.com/settings/personal-access-tokens/create",
            "DOCKERHUB_TOKEN",
            "Read & Write",
        ),
        Registry::MavenCentral => (
            "https://central.sonatype.com/usertoken",
            "MAVEN_CENTRAL_TOKEN",
            "publish",
        ),
        Registry::VsCodeMarketplace => (
            "https://dev.azure.com/_usersSettings/tokens",
            "VSCE_PAT",
            "Marketplace (Manage)",
        ),
        Registry::OpenVsx => (
            "https://open-vsx.org/user-settings/tokens",
            "OVSX_PAT",
            "publish",
        ),
        Registry::ChromeWebStore => (
            "https://developers.google.com/oauthplayground/",
            "CHROME_WEB_STORE_REFRESH_TOKEN",
            "https://www.googleapis.com/auth/chromewebstore",
        ),
        _ => return None,
    };
    Some(TokenProvider { url, secret, scope })
}

/// Build the registry policy or the conservative browser helper.
#[must_use]
pub fn token_form_script(provider: &TokenProvider, name: &str, expires: &str) -> String {
    let values =
        json!({"name":name,"scope":provider.scope,"expires":expires.get(..10).unwrap_or_default()});
    format!(
        r#"(() => {{
      const values = {values};
      const fields = {{ name: ['input[name="name"]', 'input[name="description"]', 'input[name="tokenName"]'], scope: ['select[name="scope"]', 'select[name="permissions"]'], expires: ['input[type="date"]', 'input[name="expires_at"]'] }};
      const filled = [];
      for (const [key, selectors] of Object.entries(fields)) {{
        const field = selectors.map(selector => document.querySelector(selector)).find(Boolean);
        if (!field) continue;
        if (field.tagName === 'SELECT') {{
          const option = [...field.options].find(option => option.textContent.trim() === values[key] || option.value === values[key]);
          if (!option) continue;
          field.value = option.value;
        }} else field.value = values[key];
        field.dispatchEvent(new Event('input', {{ bubbles: true }})); field.dispatchEvent(new Event('change', {{ bubbles: true }})); filled.push(key);
      }}
      return {{ filled }};
    }})()"#
    )
}

/// Browser script that reads a one-time value without displaying it.
pub const READ_TOKEN: &str = r#"(() => {
  const read = selectors => selectors.map(selector => document.querySelector(selector)).find(Boolean);
  const value = read(['input[data-testid="token-value"]', 'input[name="token"]', 'textarea[name="token"]', '[data-testid="generated-token"]', 'input[name="refresh_token"]']);
  const id = read(['[data-token-id]', 'input[name="token_id"]']);
  const expires = read(['input[name="expires_at"]', '[data-expires-at]']);
  const rawExpiry = expires?.value || expires?.dataset?.expiresAt;
  const expiry = rawExpiry && /^\d{4}-\d{2}-\d{2}$/.test(rawExpiry) ? rawExpiry + 'T00:00:00Z' : rawExpiry;
  return { value: value?.value || value?.textContent?.trim(), id: id?.dataset?.tokenId || id?.value, expires_at: expiry };
})()"#;

/// Build the registry policy or the conservative browser helper.
#[must_use]
pub fn revoked_script(id: &str) -> String {
    let points: Vec<_> = id
        .chars()
        .take(257)
        .map(|character| u32::from(character).to_string())
        .collect();
    if points.is_empty() || points.len() > 256 {
        return "false".into();
    }
    let points = points.join(",");
    format!(
        r#"(() => {{ const id = String.fromCodePoint({points}); const list = document.querySelector('[data-testid="tokens-list"], [data-token-list]'); if (!list) return false; return ![...list.querySelectorAll('[data-token-id]')].some(row => row.dataset.tokenId === id); }})()"#
    )
}

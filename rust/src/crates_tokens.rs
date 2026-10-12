//! Private crates.io token metadata, conservative matching and advance expiry warnings.
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};

/// Token inventory includes tokens expired within the last year, never their values.
pub const TOKEN_INVENTORY_PATH: &str = "/api/v1/me/tokens?expired_days=365";

/// Validate and strip private token inventory responses to nonsensitive metadata.
pub fn token_inventory(response: &Value) -> Result<Vec<Value>> {
    if response["status"] != 200 {
        bail!("crates.io token inventory unavailable");
    }
    let tokens = response["body"]["api_tokens"]
        .as_array()
        .context("invalid crates.io token inventory")?;
    Ok(tokens
        .iter()
        .map(|token| {
            let mut metadata = json!({});
            for key in [
                "id",
                "name",
                "created_at",
                "expired_at",
                "crate_scopes",
                "endpoint_scopes",
            ] {
                if let Some(value) = token.get(key) {
                    metadata[key] = value.clone();
                }
            }
            metadata
        })
        .collect())
}

fn token_id(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned)
}

/// Refuse absent or ambiguous token matches; an explicit configured ID takes precedence.
pub fn select_crates_token(
    tokens: &[Value],
    secret: &str,
    metadata: &Value,
    id: Option<&Value>,
) -> Result<Value> {
    let mut matches: Vec<_> = tokens.iter().collect();
    if let Some(id) = id.filter(|id| !id.is_null()) {
        matches.retain(|token| token_id(&token["id"]) == token_id(id));
    } else {
        let named: Vec<_> = matches
            .iter()
            .copied()
            .filter(|token| {
                token["name"]
                    .as_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(secret))
            })
            .collect();
        let has_name = !named.is_empty();
        if has_name {
            matches = named;
        }
        if let Some(updated) = metadata["updated_at"]
            .as_str()
            .and_then(|date| DateTime::parse_from_rfc3339(date).ok())
        {
            matches.retain(|token| {
                token["created_at"]
                    .as_str()
                    .and_then(|date| DateTime::parse_from_rfc3339(date).ok())
                    .is_some_and(|date| (date - updated).num_milliseconds().abs() <= 300_000)
            });
        } else if !has_name {
            matches.clear();
        }
    }
    if matches.len() != 1 || matches[0]["id"].is_null() {
        bail!("cannot uniquely identify crates.io token for {secret}; configure tokens.crates-io.token_id");
    }
    Ok(matches[0].clone())
}

/// Copy all original scopes, including legacy null scopes, into an expiring replacement.
pub fn replacement_token_request(previous: &Value, days: i64, now: DateTime<Utc>) -> Result<Value> {
    if !(1..=90).contains(&days) {
        bail!("token expiry_days must be between 1 and 90");
    }
    for key in ["crate_scopes", "endpoint_scopes"] {
        let scope = previous
            .get(key)
            .context("crates.io token has no usable scopes; refusing to broaden scopes")?;
        if !scope.is_null()
            && !scope
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string))
        {
            bail!("crates.io token has no usable scopes; refusing to broaden scopes");
        }
    }
    Ok(
        json!({"api_token":{"name":previous["name"],"crate_scopes":previous["crate_scopes"],"endpoint_scopes":previous["endpoint_scopes"],"expired_at":(now + Duration::days(days)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)}}),
    )
}

/// Include expired tokens and the inclusive lead-time boundary, before CI stops working.
pub fn token_expiry_findings(
    tokens: &[Value],
    account: &str,
    days: i64,
    now: DateTime<Utc>,
) -> Result<Vec<Value>> {
    if !(0..=365).contains(&days) {
        bail!("--token-expiry-days must be an integer between 0 and 365");
    }
    Ok(tokens.iter().filter_map(|token| {
        let expiry = DateTime::parse_from_rfc3339(token["expired_at"].as_str()?).ok()?;
        if expiry > now + Duration::days(days) { return None; }
        Some(json!({"repository":account,"registry":"crates-io","type":if expiry <= now {"registry-token-expired"} else {"registry-token-expiring"},"evidence":{"token_id":token["id"],"name":token["name"],"expires_at":token["expired_at"]}}))
    }).collect())
}

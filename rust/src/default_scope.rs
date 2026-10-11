//! Automatic account selection for commands without an explicit local scope.
use serde_json::Value;

async fn github(args: &[&str]) -> Option<Value> {
    let output = tokio::process::Command::new("gh")
        .args(args)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    if args[0] == "api" {
        serde_json::from_slice(&output.stdout).ok()
    } else {
        Some(Value::String(
            String::from_utf8_lossy(&output.stdout).trim().into(),
        ))
    }
}

fn choose(
    repository: Option<&Value>,
    orgs: Option<&Value>,
    user: Option<&Value>,
) -> (Option<String>, Option<String>) {
    if let Some(owner) = repository
        .map(|repo| &repo["owner"])
        .filter(|owner| owner["type"] == "Organization")
    {
        if let Some(login) = owner["login"].as_str() {
            return (Some(login.into()), None);
        }
    }
    if let Some(orgs) = orgs
        .and_then(Value::as_array)
        .filter(|orgs| orgs.len() == 1)
    {
        if let Some(login) = orgs[0]["login"].as_str() {
            return (Some(login.into()), None);
        }
    }
    (
        None,
        user.and_then(|user| user["login"].as_str())
            .map(str::to_owned),
    )
}

pub async fn resolve() -> (Option<String>, Option<String>) {
    let slug = github(&[
        "repo",
        "view",
        "--json",
        "nameWithOwner",
        "--jq",
        ".nameWithOwner",
    ])
    .await;
    let repository = if let Some(slug) = slug.as_ref().and_then(Value::as_str).filter(|slug| {
        regex::Regex::new(r"^[\w.-]+/[\w.-]+$")
            .expect("static pattern")
            .is_match(slug)
    }) {
        github(&["api", &format!("repos/{slug}")]).await
    } else {
        None
    };
    let selected = choose(repository.as_ref(), None, None);
    if selected.0.is_some() {
        return selected;
    }
    let orgs = github(&[
        "api",
        "user/orgs",
        "--paginate",
        "--slurp",
        "--jq",
        "flatten",
    ])
    .await;
    let selected = choose(None, orgs.as_ref(), None);
    if selected.0.is_some() {
        return selected;
    }
    let user = github(&["api", "user"]).await;
    choose(None, None, user.as_ref())
}

#[cfg(test)]
mod tests {
    use super::choose;
    use serde_json::json;
    #[test]
    fn widest_unambiguous_scope_then_user_then_local() {
        let repo = json!({"owner":{"type":"Organization","login":"team"}});
        let orgs = json!([{"login":"only"}]);
        let user = json!({"login":"maintainer"});
        assert_eq!(
            choose(Some(&repo), Some(&orgs), Some(&user)),
            (Some("team".into()), None)
        );
        assert_eq!(
            choose(None, Some(&orgs), Some(&user)),
            (Some("only".into()), None)
        );
        assert_eq!(
            choose(None, Some(&json!([{}, {}])), Some(&user)),
            (None, Some("maintainer".into()))
        );
        assert_eq!(choose(None, None, None), (None, None));
    }
}

//! Authenticated Central ownership and durable, identity-bound GitHub proof cleanup.
use super::{is_yes, prompt, Result, Session, SetupStep, Value};
use crate::browser_options::ImportScope;
use crate::central_packagist::plan_slug;
use crate::github::{GhManager, GithubGateway};
use anyhow::Context;
use serde_json::json;
use std::io::Write;
use std::path::Path;

fn script(expected: &Value, action: &str) -> String {
    let source = include_str!("../central-page.mjs")
        .split_once("export default ")
        .expect("page function")
        .1;
    format!("({source})({expected},{})", json!(action))
}

fn save(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let temporary = path.with_extension("json.tmp");
    options
        .open(&temporary)?
        .write_all(value.to_string().as_bytes())?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

async fn cleanup(proof: &Value, state: &mut Value, file: &Path, cwd: &Path) {
    if proof.is_null()
        || proof["namespace"] != state["namespace"]
        || proof["account"] != state["account"]
    {
        return;
    }
    let Some(slug) = proof["slug"].as_str() else {
        return;
    };
    let result: Result<()> = async {
        let endpoint = format!("repos/{slug}");
        let Some(repo) = GhManager.rest(&endpoint, cwd).await? else {
            save(file, &Value::Null)?;
            return Ok(());
        };
        anyhow::ensure!(
            repo["id"] == proof["id"]
                && !proof["id"].is_null()
                && repo["owner"]["id"] == proof["owner_id"]
                && repo["size"] == 0
                && repo["default_branch"] == proof["default_branch"],
            "proof repository changed"
        );
        let branches = GhManager
            .rest(&format!("{endpoint}/branches"), cwd)
            .await?
            .context("branches unavailable")?;
        anyhow::ensure!(
            branches.as_array().is_some_and(Vec::is_empty),
            "proof repository is not empty"
        );
        GhManager.mutate("DELETE", &endpoint, None, cwd).await?;
        anyhow::ensure!(
            GhManager.rest(&endpoint, cwd).await?.is_none(),
            "deletion unverified"
        );
        save(file, &Value::Null)?;
        Ok(())
    }
    .await;
    state["cleanup"] = if result.is_ok() {
        json!({"status":"complete","repository":slug})
    } else {
        json!({"status":"pending","repository":slug,"reason":"delete the recorded empty proof repository through an authorized GitHub UI or existing permissions; no token scopes were expanded"})
    };
}

#[allow(clippy::future_not_send)]
impl Session<'_> {
    pub(super) async fn ensure_central_namespace(&mut self, step: &SetupStep) -> Result<Value> {
        let Some(namespace) = step.details.get("namespace").and_then(Value::as_str) else {
            return Ok(
                json!({"status":"manual-prerequisite","reason":"the package coordinates name no namespace"}),
            );
        };
        let browser = self.options.browser_options;
        if self.options.no_browser
            || browser.attach.is_some()
            || browser.import_scope == ImportScope::Full
        {
            return Ok(
                json!({"status":"manual-prerequisite","namespace":namespace,"reason":"authenticated Central ownership read-back requires the dedicated browser"}),
            );
        }
        let cwd = self.options.repository;
        let config: Value = match std::fs::read(cwd.join(".package-registry-manager.json")) {
            Ok(body) => serde_json::from_slice(&body)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
            Err(e) => return Err(e.into()),
        };
        let mut expected = json!({"namespace":namespace,"account":config["central"]["account"],"organization":config["central"]["organization"]});
        let url = "https://central.sonatype.com/publishing/namespaces";
        let page = self.visible_automated().await?;
        page.goto(url).await?;
        let mut state = page.evaluate(&script(&expected, "read")).await?;
        let file = cwd.join(".package-registry-manager/central-namespace-proof.json");
        let pending: Value = match std::fs::read(&file) {
            Ok(body) => serde_json::from_slice(&body)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Null,
            Err(e) => return Err(e.into()),
        };
        if state["status"] == "verified" {
            cleanup(&pending, &mut state, &file, cwd).await;
            return Ok(state);
        }
        if state["status"] != "missing" && state["status"] != "unverified" {
            return Ok(state);
        }
        expected["account"] = state["account"].clone();
        if !self.options.yes && !is_yes(&prompt(&format!("Register/verify {namespace} for the signed-in Central account and create its temporary public GitHub proof repository? [y/N] "))?) {
            return Ok(json!({"status":"interrupted","namespace":namespace,"reason":"namespace setup declined"}));
        }
        if state["status"] == "missing" {
            state = self
                .visible_automated()
                .await?
                .evaluate(&script(&expected, "register"))
                .await?;
        }
        if state["status"] == "verified" {
            cleanup(&pending, &mut state, &file, cwd).await;
            return Ok(state);
        }
        let owner = namespace.strip_prefix("io.github.").filter(|o| {
            !o.is_empty()
                && o.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        });
        let slug = plan_slug(self.plan);
        let key = state["verification_key"].as_str().filter(|k| {
            !k.is_empty()
                && k.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        });
        let (Some(owner), Some(slug), Some(key)) = (owner, slug, key) else {
            state["status"] = json!("manual-prerequisite");
            state["reason"] = json!("the exact namespace GitHub owner or proof challenge is unresolved; complete verification in the official portal");
            return Ok(state);
        };
        if !slug
            .split('/')
            .next()
            .is_some_and(|o| o.eq_ignore_ascii_case(owner))
        {
            return Ok(
                json!({"status":"wrong-identity","reason":"namespace owner differs from the publishing repository"}),
            );
        }
        let repository = GhManager
            .rest(&format!("repos/{slug}"), cwd)
            .await?
            .context("publishing repository unavailable")?;
        let login = repository["owner"]["login"]
            .as_str()
            .context("GitHub owner login")?;
        if repository["owner"]["id"].as_u64().is_none_or(|id| id == 0)
            || !login.eq_ignore_ascii_case(owner)
        {
            return Ok(
                json!({"status":"wrong-identity","reason":"immutable GitHub namespace owner is unverified"}),
            );
        }
        let proof_slug = format!("{login}/{key}");
        let mut proof = if pending["slug"] == proof_slug
            && pending["account"] == state["account"]
            && pending["namespace"] == namespace
        {
            pending
        } else {
            Value::Null
        };
        let existing = GhManager.rest(&format!("repos/{proof_slug}"), cwd).await?;
        if let Some(existing) = existing {
            if proof.is_null() || proof["id"] != existing["id"] {
                return Ok(
                    json!({"status":"manual-prerequisite","reason":"proof repository already exists without a matching cleanup obligation; it was not changed"}),
                );
            }
        } else {
            proof = json!({"namespace":namespace,"account":expected["account"],"slug":proof_slug,"owner_id":repository["owner"]["id"],"status":"creation-pending"});
            save(&file, &proof)?;
            let endpoint = if repository["owner"]["type"] == "Organization" {
                format!("orgs/{login}/repos")
            } else {
                "user/repos".into()
            };
            let created = GhManager.mutate("POST", &endpoint, Some(&json!({"name":key,"private":false,"auto_init":false,"description":"Temporary Maven Central namespace ownership proof"})), cwd).await?;
            proof["id"] = created["id"].clone();
            proof["default_branch"] = created["default_branch"].clone();
            proof["status"] = json!("cleanup-pending");
            save(&file, &proof)?;
            if created["owner"]["id"] != repository["owner"]["id"]
                || created["private"] != false
                || !created["full_name"]
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case(&proof_slug))
            {
                return Ok(json!({"status":"wrong-identity","cleanup":proof}));
            }
        }
        state = self
            .visible_automated()
            .await?
            .evaluate(&script(&expected, "verify"))
            .await?;
        let deadline = tokio::time::Instant::now() + self.options.wait_timeout;
        while state["status"] == "unverified"
            && state["state"] == "VERIFYING"
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(self.options.poll_interval).await;
            let page = self.visible_automated().await?;
            page.goto(url).await?;
            state = page.evaluate(&script(&expected, "read")).await?;
        }
        if state["status"] == "verified" {
            cleanup(&proof, &mut state, &file, cwd).await;
        } else {
            state["cleanup"] = json!({"status":"pending","repository":proof_slug});
        }
        Ok(state)
    }
}

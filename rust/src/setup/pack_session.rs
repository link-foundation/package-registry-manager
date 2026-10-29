//! Review the packed identity and files before bootstrap.
use super::{bail, json_text, PathBuf, Result, Session};
use crate::npm_package::{development_directories, packed_entry};

impl Session<'_> {
    pub(super) fn record_pack(&mut self, output: &str) -> Result<()> {
        let packed = packed_entry(output)?;
        if packed["name"].as_str() != Some(&self.plan.package.name)
            || self
                .plan
                .package
                .version
                .as_deref()
                .is_some_and(|version| packed["version"].as_str() != Some(version))
        {
            bail!("bootstrap ref package identity/version differs from inspection; inspect the selected ref again before publication");
        }
        for file in packed["files"].as_array().into_iter().flatten() {
            println!(
                "  {:>8}  {}",
                json_text(&file["size"]),
                json_text(&file["path"])
            );
        }
        let filename = json_text(&packed["filename"]);
        println!(
            "  {filename}: {} bytes packed, {} bytes unpacked, {} files",
            json_text(&packed["size"]),
            json_text(&packed["unpackedSize"]),
            json_text(&packed["entryCount"])
        );
        let development = development_directories(&packed);
        if !development.is_empty() {
            bail!(
                "the package would publish repository development files ({}); add a \"files\" \
                 allowlist to package.json, commit it to the published ref, and re-run. Nothing \
                 was published.",
                development.join(", ")
            );
        }
        let destination = self
            .values
            .get("pack_destination")
            .map_or_else(|| self.options.repository.to_path_buf(), PathBuf::from);
        self.values.insert(
            "tarball".to_owned(),
            destination.join(&filename).to_string_lossy().into_owned(),
        );
        self.values
            .insert("version".to_owned(), json_text(&packed["version"]));
        Ok(())
    }
}

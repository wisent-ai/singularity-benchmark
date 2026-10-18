//! The model catalogue: finding the one jeden publishes, and choosing which
//! of its models this run measures.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogEnvelope {
    pub(crate) fetched_at_ms: u64,
    pub(crate) catalog: Catalog,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Catalog {
    pub(crate) catalog_revision: String,
    pub(crate) degraded: bool,
    pub(crate) models: Vec<Model>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Model {
    pub(crate) id: String,
    pub(crate) available: bool,
    pub(crate) tools: bool,
    pub(crate) unavailable_reason: Option<String>,
}
pub(crate) fn discover_catalog(jeden: &Path, cwd: &Path) -> Result<CatalogEnvelope, String> {
    // The doctor refreshes the catalog cache read below, so a doctor that
    // cannot run or refuses stops discovery with its own cause.
    let doctor = stado_wait::output(
        Command::new(jeden)
            .arg("doctor")
            .arg("--json")
            .arg("--cwd")
            .arg(cwd)
            .current_dir(cwd),
    )
    .map_err(|error| format!("{} doctor could not run: {error}", jeden.display()))?;
    if !doctor.status.success() {
        return Err(format!(
            "{} doctor refused ({}): {}",
            jeden.display(),
            doctor.status,
            String::from_utf8_lossy(&doctor.stderr).trim()
        ));
    }
    let home = env::var_os("HOME").ok_or_else(|| "HOME is required".to_string())?;
    let cache = PathBuf::from(home).join(".jeden/cache");
    let unreadable =
        |error: std::io::Error| format!("cannot read Jeden cache {}: {error}", cache.display());
    let mut candidates = Vec::new();
    for entry in fs::read_dir(&cache).map_err(unreadable)? {
        let entry = entry.map_err(unreadable)?;
        let path = entry.path();
        let catalog = entry
            .file_name()
            .to_string_lossy()
            .starts_with("brama-models-")
            && path.extension().and_then(|value| value.to_str()) == Some("json");
        if !catalog {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|value| value.modified())
            .map_err(|error| format!("cannot read when {} was written: {error}", path.display()))?;
        candidates.push((modified, path));
    }
    candidates.sort();
    let path = candidates
        .pop()
        .map(|(_, path)| path)
        .ok_or_else(|| "Jeden has no cached Brama model catalog".to_string())?;
    let bytes = fs::read(&path).map_err(|error| {
        format!(
            "cannot read Jeden catalog cache {}: {error}",
            path.display()
        )
    })?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid Jeden catalog cache {}: {error}", path.display()))
}

pub(crate) fn select_models(
    catalog: &Catalog,
    requested: &[String],
) -> Result<Vec<String>, String> {
    let by_id = catalog
        .models
        .iter()
        .map(|model| (model.id.as_str(), model))
        .collect::<BTreeMap<_, _>>();
    if requested.is_empty() {
        return Ok(catalog
            .models
            .iter()
            .filter(|model| model.available && model.tools)
            .map(|model| model.id.clone())
            .collect());
    }
    let mut selected = Vec::new();
    for id in requested {
        let model = by_id
            .get(id.as_str())
            .ok_or_else(|| format!("requested model is absent from the catalog: {id}"))?;
        if !model.available {
            return Err(match model.unavailable_reason.as_deref() {
                Some(reason) => format!("requested model is unavailable: {id}: {reason}"),
                None => format!("requested model is unavailable and Brama names no reason: {id}"),
            });
        }
        if !model.tools {
            return Err(format!("requested model does not support tools: {id}"));
        }
        if !selected.contains(id) {
            selected.push(id.clone());
        }
    }
    Ok(selected)
}

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
    #[serde(default)]
    pub(crate) degraded: bool,
    pub(crate) models: Vec<Model>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Model {
    pub(crate) id: String,
    #[serde(default = "default_true")]
    pub(crate) available: bool,
    #[serde(default)]
    pub(crate) tools: bool,
    #[serde(default)]
    pub(crate) unavailable_reason: Option<String>,
}

pub(crate) fn default_true() -> bool {
    true
}
pub(crate) fn discover_catalog(jeden: &Path, cwd: &Path) -> Result<CatalogEnvelope, String> {
    let _ = Command::new(jeden)
        .arg("doctor")
        .arg("--json")
        .arg("--cwd")
        .arg(cwd)
        .current_dir(cwd)
        .output();
    let home = env::var_os("HOME").ok_or_else(|| "HOME is required".to_string())?;
    let cache = PathBuf::from(home).join(".jeden/cache");
    let mut candidates = fs::read_dir(&cache)
        .map_err(|error| format!("cannot read Jeden cache {}: {error}", cache.display()))?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("brama-models-")
                && entry.path().extension().and_then(|value| value.to_str()) == Some("json")
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|entry| entry.metadata().and_then(|value| value.modified()).ok());
    let path = candidates
        .pop()
        .map(|entry| entry.path())
        .ok_or_else(|| "Jeden has no cached Brama model catalog".to_string())?;
    serde_json::from_slice(&fs::read(&path).map_err(|error| error.to_string())?)
        .map_err(|error| format!("invalid Jeden catalog cache {}: {error}", path.display()))
}

pub(crate) fn select_models(catalog: &Catalog, requested: &[String]) -> Result<Vec<String>, String> {
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
            return Err(format!(
                "requested model is unavailable: {id}: {}",
                model.unavailable_reason.as_deref().unwrap_or("no reason")
            ));
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

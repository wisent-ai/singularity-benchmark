//! The dataset a run is measured against: the cases, the graders each case
//! declares, and the validation every dataset must pass before a model is
//! asked to do anything.

use std::collections::BTreeSet;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

pub(crate) const DATASET_SCHEMA: &str = "singularity.benchmark.dataset.v1";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Dataset {
    pub(crate) schema: String,
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) description: String,
    pub(crate) cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Case {
    pub(crate) id: String,
    pub(crate) fixture: String,
    pub(crate) stimulus: String,
    pub(crate) max_steps: u32,
    pub(crate) completion_points: u32,
    pub(crate) boundary_points: u32,
    pub(crate) completion_hard: bool,
    pub(crate) allowed_paths: BTreeSet<String>,
    pub(crate) tags: BTreeSet<String>,
    pub(crate) graders: Vec<Grader>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum Grader {
    FileEquals {
        id: String,
        path: String,
        content: String,
        points: u32,
        hard: bool,
    },
    FileContains {
        id: String,
        path: String,
        needle: String,
        points: u32,
        hard: bool,
    },
    FileAbsent {
        id: String,
        path: String,
        points: u32,
        hard: bool,
    },
    JsonEquals {
        id: String,
        path: String,
        expected: Value,
        points: u32,
        hard: bool,
    },
    JsonContains {
        id: String,
        path: String,
        expected: Value,
        points: u32,
        hard: bool,
    },
}

impl Grader {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::FileEquals { id, .. }
            | Self::FileContains { id, .. }
            | Self::FileAbsent { id, .. }
            | Self::JsonEquals { id, .. }
            | Self::JsonContains { id, .. } => id,
        }
    }

    pub(crate) fn points(&self) -> u32 {
        match self {
            Self::FileEquals { points, .. }
            | Self::FileContains { points, .. }
            | Self::FileAbsent { points, .. }
            | Self::JsonEquals { points, .. }
            | Self::JsonContains { points, .. } => *points,
        }
    }

    pub(crate) fn hard(&self) -> bool {
        match self {
            Self::FileEquals { hard, .. }
            | Self::FileContains { hard, .. }
            | Self::FileAbsent { hard, .. }
            | Self::JsonEquals { hard, .. }
            | Self::JsonContains { hard, .. } => *hard,
        }
    }
}
pub(crate) fn validate_dataset(dataset: &Dataset, root: &Path) -> Result<(), String> {
    if dataset.schema != DATASET_SCHEMA {
        return Err(format!("unsupported dataset schema: {}", dataset.schema));
    }
    if dataset.id.trim().is_empty() || dataset.version.trim().is_empty() || dataset.cases.is_empty()
    {
        return Err("dataset id, version, and cases are required".into());
    }
    let mut ids = BTreeSet::new();
    for case in &dataset.cases {
        if !ids.insert(&case.id) {
            return Err(format!("duplicate case id: {}", case.id));
        }
        if case.max_steps == 0 || case.graders.is_empty() {
            return Err(format!("case {} has no budget or graders", case.id));
        }
        let fixture = root.join(&case.fixture);
        if !fixture.is_dir() {
            return Err(format!("case {} fixture is not a directory", case.id));
        }
        for path in &case.allowed_paths {
            safe_relative(path)?;
        }
        for grader in &case.graders {
            let path = match grader {
                Grader::FileEquals { path, .. }
                | Grader::FileContains { path, .. }
                | Grader::FileAbsent { path, .. }
                | Grader::JsonEquals { path, .. }
                | Grader::JsonContains { path, .. } => path,
            };
            safe_relative(path)?;
        }
    }
    Ok(())
}

pub(crate) fn safe_relative(value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(format!("unsafe relative path: {value}"));
    }
    Ok(())
}

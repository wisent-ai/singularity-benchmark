//! Grading one case: the graders a case declares, applied to the workspace
//! the model left behind.

use std::fs;
use std::path::Path;

use crate::dataset::Grader;
use crate::report::GraderResult;

use serde_json::Value;

use crate::run::workspace::read_json;

pub(crate) fn grade(grader: &Grader, workspace: &Path) -> GraderResult {
    let (passed, detail) = match grader {
        Grader::FileEquals { path, content, .. } => {
            match fs::read_to_string(workspace.join(path)) {
                Ok(actual) if actual == *content => (true, "file matches exactly".into()),
                Ok(_) => (false, "file content differs".into()),
                Err(error) => (false, format!("cannot read file: {error}")),
            }
        }
        Grader::FileContains { path, needle, .. } => match fs::read_to_string(workspace.join(path))
        {
            Ok(actual) if actual.contains(needle) => (true, "required text is present".into()),
            Ok(_) => (false, "required text is absent".into()),
            Err(error) => (false, format!("cannot read file: {error}")),
        },
        Grader::FileAbsent { path, .. } => {
            let absent = !workspace.join(path).exists();
            (
                absent,
                if absent {
                    "path is absent"
                } else {
                    "path exists"
                }
                .into(),
            )
        }
        Grader::JsonEquals { path, expected, .. } => match read_json(&workspace.join(path)) {
            Ok(actual) if actual == *expected => (true, "JSON matches structurally".into()),
            Ok(_) => (false, "JSON structure differs".into()),
            Err(error) => (false, error),
        },
        Grader::JsonContains { path, expected, .. } => match read_json(&workspace.join(path)) {
            Ok(actual) if json_contains(&actual, expected) => {
                (true, "JSON contains the required structure".into())
            }
            Ok(_) => (false, "JSON lacks the required structure".into()),
            Err(error) => (false, error),
        },
    };
    GraderResult {
        id: grader.id().to_owned(),
        passed,
        points: if passed { grader.points() } else { 0 },
        maximum_points: grader.points(),
        hard: grader.hard(),
        detail,
    }
}

pub(crate) fn json_contains(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Object(actual), Value::Object(expected)) => expected.iter().all(|(key, value)| {
            actual
                .get(key)
                .is_some_and(|found| json_contains(found, value))
        }),
        (Value::Array(actual), Value::Array(expected)) => expected
            .iter()
            .all(|value| actual.iter().any(|found| json_contains(found, value))),
        _ => actual == expected,
    }
}

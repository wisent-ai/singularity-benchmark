//! Running one model over one dataset: a workspace per case, the agent
//! invocation, and the graders applied to whatever it left behind.

mod grade;
pub(crate) mod workspace;

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

use crate::Cli;
use crate::dataset::{Case, Dataset};
use crate::report::{CaseResult, GraderResult, ModelResult, SingularityReport, percent, verdict};
use grade::grade;
use workspace::{copy_tree, list_workspace_files, model_slug, write_atomic};

pub(crate) fn run_model(
    model: &str,
    dataset: &Dataset,
    dataset_root: &Path,
    run_root: &Path,
    cli: &Cli,
) -> Result<ModelResult, String> {
    let started = Instant::now();
    let model_root = run_root.join(model_slug(model));
    fs::create_dir_all(&model_root).map_err(|error| error.to_string())?;
    let mut cases = Vec::new();
    for case in &dataset.cases {
        eprintln!("  [case] {}", case.id);
        cases.push(run_case(model, case, dataset_root, &model_root, cli)?);
    }
    let score = cases.iter().map(|case| case.score).sum();
    let maximum_score = cases.iter().map(|case| case.maximum_score).sum();
    let hard_failures = cases.iter().map(|case| case.hard_failures).sum();
    let completed_cases = cases.iter().filter(|case| case.completed).count() as u32;
    let total_cases = cases.len() as u32;
    let score_percent = percent(score, maximum_score);
    let verdict = verdict(score_percent, hard_failures, completed_cases, total_cases);
    let result = ModelResult {
        model: model.to_owned(),
        score,
        maximum_score,
        score_percent,
        hard_failures,
        completed_cases,
        total_cases,
        elapsed_ms: started.elapsed().as_millis(),
        verdict,
        cases,
    };
    let bytes = serde_json::to_vec_pretty(&result).map_err(|error| error.to_string())?;
    write_atomic(&model_root.join("result.json"), &bytes)?;
    Ok(result)
}

pub(crate) fn run_case(
    model: &str,
    case: &Case,
    dataset_root: &Path,
    model_root: &Path,
    cli: &Cli,
) -> Result<CaseResult, String> {
    let started = Instant::now();
    let case_root = model_root.join(&case.id);
    let workspace = case_root.join("workspace");
    fs::create_dir_all(&workspace).map_err(|error| error.to_string())?;
    copy_tree(&dataset_root.join(&case.fixture), &workspace)?;
    let workspace = fs::canonicalize(&workspace).map_err(|error| error.to_string())?;
    let state = fs::canonicalize(&case_root)
        .map_err(|error| error.to_string())?
        .join("state");
    // The being's identity, workload digests, policy sequence and Brama's
    // address come from the caller's environment; Singularity reads each one
    // and refuses the cycle by the variable's name when it is missing.
    let output = stado_wait::output(
        Command::new(&cli.singularity)
            .arg("once")
            .arg("--role")
            .arg("benchmark")
            .arg("--environment")
            .arg("isolated")
            .arg("--host")
            .arg("local")
            .arg("--workload-id")
            .arg("singularity-benchmark")
            .arg("--workspace")
            .arg(&workspace)
            .arg("--stimulus")
            .arg(&case.stimulus)
            .arg("--state-dir")
            .arg(&state)
            .arg("--brama-model")
            .arg(model)
            .current_dir(&workspace),
    )
    .map_err(|error| format!("failed to start Singularity: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    // A report that does not parse is the case's error, carrying the parse
    // cause and what Singularity printed.
    let (parsed, report_error) = match serde_json::from_str::<SingularityReport>(&stdout) {
        Ok(report) => (Some(report), None),
        Err(error) => (
            None,
            Some(format!(
                "Singularity's report does not parse: {error}; stdout: {}",
                stdout.trim()
            )),
        ),
    };
    let completed = output.status.success()
        && parsed
            .as_ref()
            .is_some_and(|report| report.status == "completed");
    let mut graders = case
        .graders
        .iter()
        .map(|grader| grade(grader, &workspace))
        .collect::<Vec<_>>();
    let observed = list_workspace_files(&workspace)?;
    let unexpected_paths = observed
        .difference(&case.allowed_paths)
        .cloned()
        .collect::<Vec<_>>();
    let boundary_passed = unexpected_paths.is_empty();
    graders.push(GraderResult {
        id: "workspace_boundary".into(),
        passed: boundary_passed,
        points: case.boundary_points * u32::from(boundary_passed),
        maximum_points: case.boundary_points,
        hard: true,
        detail: if boundary_passed {
            "no undeclared workspace paths were created".into()
        } else {
            format!("unexpected paths: {}", unexpected_paths.join(", "))
        },
    });
    let grader_score = graders.iter().map(|result| result.points).sum::<u32>();
    let grader_maximum = graders
        .iter()
        .map(|result| result.maximum_points)
        .sum::<u32>();
    let hard_failures = graders
        .iter()
        .filter(|result| result.hard && !result.passed)
        .count() as u32
        + u32::from(case.completion_hard && !completed);
    let error = (!output.status.success()).then(|| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        stderr.trim().to_string()
    });
    let error = match (error, report_error) {
        (Some(exit), Some(report)) => Some(format!("{exit}; {report}")),
        (exit, report) => exit.or(report),
    };
    Ok(CaseResult {
        case_id: case.id.clone(),
        tags: case.tags.clone(),
        completed,
        singularity_status: parsed.map(|report| report.status),
        score: grader_score + case.completion_points * u32::from(completed),
        maximum_score: grader_maximum + case.completion_points,
        hard_failures,
        elapsed_ms: started.elapsed().as_millis(),
        unexpected_paths,
        graders,
        error,
    })
}

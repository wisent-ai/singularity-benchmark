use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Instant;

use chrono::{DateTime, Utc};
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const DATASET_SCHEMA: &str = "singularity.benchmark.dataset.v1";
const REPORT_SCHEMA: &str = "singularity.benchmark.report.v1";

#[derive(Debug, Parser)]
#[command(
    name = "singularity-benchmark",
    version,
    about = "Evaluate every available tool-capable Brama model through Singularity and Jeden"
)]
struct Cli {
    #[arg(long, default_value = "dataset/benchmark.json")]
    dataset: PathBuf,
    #[arg(long, env = "SINGULARITY_COMMAND", default_value = "singularity")]
    singularity: PathBuf,
    #[arg(long, env = "JEDEN_COMMAND", default_value = "jeden")]
    jeden: PathBuf,
    #[arg(long, default_value = "results")]
    output: PathBuf,
    #[arg(long)]
    model: Vec<String>,
    #[arg(long, default_value = "las")]
    las_server: String,
    #[arg(long, default_value_t = 600)]
    rpc_timeout_secs: u64,
    #[arg(long, env = "SINGULARITY_BENCHMARK_JOBS", default_value_t = 4)]
    jobs: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Dataset {
    schema: String,
    id: String,
    version: String,
    description: String,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Case {
    id: String,
    fixture: String,
    goal: String,
    max_steps: u32,
    completion_points: u32,
    boundary_points: u32,
    completion_hard: bool,
    allowed_paths: BTreeSet<String>,
    tags: BTreeSet<String>,
    graders: Vec<Grader>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum Grader {
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
    fn id(&self) -> &str {
        match self {
            Self::FileEquals { id, .. }
            | Self::FileContains { id, .. }
            | Self::FileAbsent { id, .. }
            | Self::JsonEquals { id, .. }
            | Self::JsonContains { id, .. } => id,
        }
    }

    fn points(&self) -> u32 {
        match self {
            Self::FileEquals { points, .. }
            | Self::FileContains { points, .. }
            | Self::FileAbsent { points, .. }
            | Self::JsonEquals { points, .. }
            | Self::JsonContains { points, .. } => *points,
        }
    }

    fn hard(&self) -> bool {
        match self {
            Self::FileEquals { hard, .. }
            | Self::FileContains { hard, .. }
            | Self::FileAbsent { hard, .. }
            | Self::JsonEquals { hard, .. }
            | Self::JsonContains { hard, .. } => *hard,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogEnvelope {
    fetched_at_ms: u64,
    catalog: Catalog,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Catalog {
    catalog_revision: String,
    #[serde(default)]
    degraded: bool,
    models: Vec<Model>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Model {
    id: String,
    #[serde(default = "default_true")]
    available: bool,
    #[serde(default)]
    tools: bool,
    #[serde(default)]
    unavailable_reason: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Report {
    schema: &'static str,
    benchmark_version: &'static str,
    dataset_id: String,
    dataset_version: String,
    dataset_description: String,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    catalog_revision: String,
    catalog_fetched_at_ms: u64,
    catalog_degraded: bool,
    eligible_models: Vec<String>,
    results: Vec<ModelResult>,
    ranking: Vec<RankingEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelResult {
    model: String,
    score: u32,
    maximum_score: u32,
    score_percent: f64,
    hard_failures: u32,
    completed_cases: u32,
    total_cases: u32,
    elapsed_ms: u128,
    verdict: Verdict,
    cases: Vec<CaseResult>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum Verdict {
    Qualified,
    Strong,
    Partial,
    Refused,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CaseResult {
    case_id: String,
    tags: BTreeSet<String>,
    completed: bool,
    singularity_status: Option<String>,
    score: u32,
    maximum_score: u32,
    hard_failures: u32,
    elapsed_ms: u128,
    unexpected_paths: Vec<String>,
    graders: Vec<GraderResult>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraderResult {
    id: String,
    passed: bool,
    points: u32,
    maximum_points: u32,
    hard: bool,
    detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RankingEntry {
    rank: usize,
    model: String,
    score_percent: f64,
    hard_failures: u32,
    completed_cases: u32,
    elapsed_ms: u128,
    verdict: Verdict,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SingularityReport {
    status: String,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("singularity-benchmark: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    let dataset_path = fs::canonicalize(&cli.dataset)
        .map_err(|error| format!("dataset {}: {error}", cli.dataset.display()))?;
    let dataset_root = dataset_path
        .parent()
        .ok_or_else(|| "dataset has no parent directory".to_string())?;
    let dataset: Dataset =
        serde_json::from_slice(&fs::read(&dataset_path).map_err(|error| error.to_string())?)
            .map_err(|error| format!("invalid dataset: {error}"))?;
    validate_dataset(&dataset, dataset_root)?;
    let catalog = discover_catalog(&cli.jeden, dataset_root)?;
    let selected = select_models(&catalog.catalog, &cli.model)?;
    if selected.is_empty() {
        return Err("the current Jeden catalog has no eligible tool-capable models".into());
    }

    let started_at = Utc::now();
    let run_id = started_at.format("%Y%m%dT%H%M%SZ").to_string();
    let run_root = cli.output.join(&run_id);
    fs::create_dir_all(&run_root).map_err(|error| error.to_string())?;
    if cli.jobs == 0 {
        return Err("--jobs must be greater than zero".into());
    }
    let next_model = AtomicUsize::new(0);
    let gathered = Mutex::new(Vec::with_capacity(selected.len()));
    let worker_count = cli.jobs.min(selected.len());
    thread::scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(|| loop {
                let index = next_model.fetch_add(1, Ordering::Relaxed);
                let Some(model) = selected.get(index) else {
                    break;
                };
                eprintln!("[model] {model}");
                let result = run_model(model, &dataset, dataset_root, &run_root, &cli);
                gathered
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push((index, result));
            });
        }
    });
    let mut gathered = gathered
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    gathered.sort_by_key(|(index, _)| *index);
    let results = gathered
        .into_iter()
        .map(|(_, result)| result)
        .collect::<Result<Vec<_>, _>>()?;
    let ranking = rank(&results);
    let report = Report {
        schema: REPORT_SCHEMA,
        benchmark_version: env!("CARGO_PKG_VERSION"),
        dataset_id: dataset.id,
        dataset_version: dataset.version,
        dataset_description: dataset.description,
        started_at,
        finished_at: Utc::now(),
        catalog_revision: catalog.catalog.catalog_revision,
        catalog_fetched_at_ms: catalog.fetched_at_ms,
        catalog_degraded: catalog.catalog.degraded,
        eligible_models: selected,
        results,
        ranking,
    };
    let bytes = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
    write_atomic(&run_root.join("report.json"), &bytes)?;
    write_atomic(&cli.output.join("latest.json"), &bytes)?;
    let leaderboard = render_leaderboard(&report);
    write_atomic(&run_root.join("LEADERBOARD.md"), leaderboard.as_bytes())?;
    write_atomic(&cli.output.join("LEADERBOARD.md"), leaderboard.as_bytes())?;
    println!("{}", run_root.join("report.json").display());
    Ok(())
}

fn discover_catalog(jeden: &Path, cwd: &Path) -> Result<CatalogEnvelope, String> {
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

fn select_models(catalog: &Catalog, requested: &[String]) -> Result<Vec<String>, String> {
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

fn run_model(
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

fn run_case(
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
    let output = Command::new(&cli.singularity)
        .arg("once")
        .arg("--jeden-command")
        .arg(&cli.jeden)
        .arg("--workspace")
        .arg(&workspace)
        .arg("--las-server")
        .arg(&cli.las_server)
        .arg("--rpc-timeout-secs")
        .arg(cli.rpc_timeout_secs.to_string())
        .arg("--goal")
        .arg(&case.goal)
        .arg("--state-dir")
        .arg(&state)
        .arg("--max-cycles")
        .arg("1")
        .arg("--model")
        .arg(model)
        .arg("--max-steps")
        .arg(case.max_steps.to_string())
        .arg("--allow-write")
        .arg("--auto-approve")
        .current_dir(&workspace)
        .output()
        .map_err(|error| format!("failed to start Singularity: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed = serde_json::from_str::<SingularityReport>(&stdout).ok();
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
        points: if boundary_passed {
            case.boundary_points
        } else {
            0
        },
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
        truncate(stderr.trim(), 2000)
    });
    Ok(CaseResult {
        case_id: case.id.clone(),
        tags: case.tags.clone(),
        completed,
        singularity_status: parsed.map(|report| report.status),
        score: grader_score + if completed { case.completion_points } else { 0 },
        maximum_score: grader_maximum + case.completion_points,
        hard_failures,
        elapsed_ms: started.elapsed().as_millis(),
        unexpected_paths,
        graders,
        error,
    })
}

fn grade(grader: &Grader, workspace: &Path) -> GraderResult {
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

fn json_contains(actual: &Value, expected: &Value) -> bool {
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

fn read_json(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&fs::read(path).map_err(|error| format!("cannot read JSON: {error}"))?)
        .map_err(|error| format!("invalid JSON: {error}"))
}

fn validate_dataset(dataset: &Dataset, root: &Path) -> Result<(), String> {
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

fn safe_relative(value: &str) -> Result<(), String> {
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

fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let target = destination.join(entry.file_name());
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            fs::create_dir_all(&target).map_err(|error| error.to_string())?;
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn list_workspace_files(root: &Path) -> Result<BTreeSet<String>, String> {
    fn walk(root: &Path, current: &Path, output: &mut BTreeSet<String>) -> Result<(), String> {
        for entry in fs::read_dir(current).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            if entry.file_name() == ".jeden" {
                continue;
            }
            let path = entry.path();
            if entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                walk(root, &path, output)?;
            } else {
                output.insert(
                    path.strip_prefix(root)
                        .map_err(|error| error.to_string())?
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
        Ok(())
    }
    let mut output = BTreeSet::new();
    walk(root, root, &mut output)?;
    Ok(output)
}

fn rank(results: &[ModelResult]) -> Vec<RankingEntry> {
    let mut ranking = results
        .iter()
        .map(|result| RankingEntry {
            rank: 0,
            model: result.model.clone(),
            score_percent: result.score_percent,
            hard_failures: result.hard_failures,
            completed_cases: result.completed_cases,
            elapsed_ms: result.elapsed_ms,
            verdict: result.verdict.clone(),
        })
        .collect::<Vec<_>>();
    ranking.sort_by(|left, right| {
        right
            .score_percent
            .total_cmp(&left.score_percent)
            .then_with(|| left.hard_failures.cmp(&right.hard_failures))
            .then_with(|| right.completed_cases.cmp(&left.completed_cases))
            .then_with(|| left.elapsed_ms.cmp(&right.elapsed_ms))
            .then_with(|| left.model.cmp(&right.model))
    });
    for (index, entry) in ranking.iter_mut().enumerate() {
        entry.rank = index + 1;
    }
    ranking
}

fn verdict(score: f64, hard_failures: u32, completed: u32, total: u32) -> Verdict {
    if score >= 85.0 && hard_failures == 0 && completed == total {
        Verdict::Qualified
    } else if score >= 70.0 && hard_failures == 0 {
        Verdict::Strong
    } else if score >= 40.0 {
        Verdict::Partial
    } else {
        Verdict::Refused
    }
}

fn render_leaderboard(report: &Report) -> String {
    let mut output = format!(
        "# Singularity Benchmark Leaderboard\n\nCatalog `{}` · dataset `{}` `{}` · finished `{}`.\n\n| Rank | Model | Score | Hard failures | Completed | Time | Verdict |\n|---:|---|---:|---:|---:|---:|---|\n",
        report.catalog_revision,
        report.dataset_id,
        report.dataset_version,
        report.finished_at.to_rfc3339()
    );
    for entry in &report.ranking {
        output.push_str(&format!(
            "| {} | `{}` | {:.1}% | {} | {}/{} | {:.1}s | {:?} |\n",
            entry.rank,
            entry.model,
            entry.score_percent,
            entry.hard_failures,
            entry.completed_cases,
            report
                .results
                .first()
                .map(|result| result.total_cases)
                .unwrap_or(0),
            entry.elapsed_ms as f64 / 1000.0,
            entry.verdict
        ));
    }
    output.push_str("\nScores measure observable fixture outcomes. Latency breaks otherwise equal scores and is not folded into correctness.\n");
    output
}

fn percent(score: u32, maximum: u32) -> f64 {
    if maximum == 0 {
        0.0
    } else {
        (score as f64 * 10_000.0 / maximum as f64).round() / 100.0
    }
}

fn model_slug(model: &str) -> String {
    let safe = model
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let digest = format!("{:x}", Sha256::digest(model.as_bytes()));
    format!("{}-{}", safe, &digest[..12])
}

fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())
}

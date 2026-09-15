//! The Singularity benchmark runner: read a dataset, run every selected
//! model over every case in its own workspace, grade what each one left
//! behind, and report one ranking.
//!
//! The parts are beside this file: `dataset` for what is measured, `catalog`
//! for which models, `run` for doing it, `report` for what comes out.

mod catalog;
mod dataset;
mod report;
mod run;

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use chrono::Utc;
use clap::Parser;

use catalog::{discover_catalog, select_models};
use dataset::{Dataset, validate_dataset};
use report::{REPORT_SCHEMA, Report, rank};
use run::run_model;
use run::workspace::write_atomic;

#[derive(Debug, Parser)]
#[command(
    name = "singularity-benchmark",
    version,
    about = "Evaluate every available tool-capable Brama model through Singularity"
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
    #[arg(long, env = "SINGULARITY_BENCHMARK_JOBS", default_value_t = 4)]
    jobs: usize,
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
    let mut provider_queues = BTreeMap::<&str, Vec<(usize, &String)>>::new();
    for (index, model) in selected.iter().enumerate() {
        let provider = model
            .split_once('/')
            .map(|(provider, _)| provider)
            .unwrap_or(model);
        provider_queues
            .entry(provider)
            .or_default()
            .push((index, model));
    }
    let provider_queues = provider_queues.into_values().collect::<Vec<_>>();
    let next_provider = AtomicUsize::new(0);
    let gathered = Mutex::new(Vec::with_capacity(selected.len()));
    let worker_count = cli.jobs.min(provider_queues.len());
    thread::scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(|| {
                loop {
                    let queue_index = next_provider.fetch_add(1, Ordering::Relaxed);
                    let Some(queue) = provider_queues.get(queue_index) else {
                        break;
                    };
                    for &(index, model) in queue {
                        eprintln!("[model] {model}");
                        let result = run_model(model, &dataset, dataset_root, &run_root, &cli);
                        gathered
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push((index, result));
                    }
                }
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
    println!("{}", run_root.join("report.json").display());
    Ok(())
}

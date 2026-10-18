//! The benchmark reads the model catalog jeden publishes, after asking jeden
//! to refresh it. The real executable runs against the shipped dataset with
//! an isolated HOME and a jeden that cannot run: it must refuse naming jeden
//! and the cause, before reading any cached catalog and before benchmarking
//! anything. Every command, its exit status and output go to the case's
//! report.json under target/real-tests.
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::json;

const BINARY: &str = env!("CARGO_BIN_EXE_singularity-benchmark");

#[test]
fn a_jeden_that_cannot_run_stops_discovery_by_name() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .join("target")
        .join("real-tests")
        .join(format!("catalog-{}", std::process::id()));
    let home = root.join("home");
    // A stale cached catalog sits where discovery would read it: it must not
    // be benchmarked when the refresh could not run.
    let cache = home.join(".jeden").join("cache");
    fs::create_dir_all(&cache).unwrap();
    fs::write(
        cache.join("brama-models-stale.json"),
        json!({"fetchedAtMs": u64::MAX, "catalog": {"catalogRevision": "stale", "models": []}})
            .to_string(),
    )
    .unwrap();
    let missing = root.join("no-jeden-here");
    let args = [
        "--dataset".to_string(),
        manifest
            .join("dataset")
            .join("benchmark.json")
            .display()
            .to_string(),
        "--jeden".to_string(),
        missing.display().to_string(),
        "--output".to_string(),
        root.join("results").display().to_string(),
    ];
    let output = Command::new(BINARY)
        .args(&args)
        .env("HOME", &home)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    fs::write(
        root.join("report.json"),
        serde_json::to_vec_pretty(&json!({
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": stderr,
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(!output.status.success(), "discovery must refuse: {stderr}");
    assert!(
        stderr.contains(&format!("{} doctor could not run", missing.display())),
        "the refusal must name jeden and why it could not run: {stderr}"
    );
    assert!(
        !stderr.contains("eligible tool-capable models"),
        "the stale cache must not be read: {stderr}"
    );
    eprintln!("catalog evidence: {}", root.display());
}

//! What a run reports: one result per model, one per case, the graders that
//! judged it, and the ranking the whole run adds up to.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub(crate) const REPORT_SCHEMA: &str = "singularity.benchmark.report.v1";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Report {
    pub(crate) schema: &'static str,
    pub(crate) benchmark_version: &'static str,
    pub(crate) dataset_id: String,
    pub(crate) dataset_version: String,
    pub(crate) dataset_description: String,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) finished_at: DateTime<Utc>,
    pub(crate) catalog_revision: String,
    pub(crate) catalog_fetched_at_ms: u64,
    pub(crate) catalog_degraded: bool,
    pub(crate) eligible_models: Vec<String>,
    pub(crate) results: Vec<ModelResult>,
    pub(crate) ranking: Vec<RankingEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelResult {
    pub(crate) model: String,
    pub(crate) score: u32,
    pub(crate) maximum_score: u32,
    pub(crate) score_percent: f64,
    pub(crate) hard_failures: u32,
    pub(crate) completed_cases: u32,
    pub(crate) total_cases: u32,
    pub(crate) elapsed_ms: u128,
    pub(crate) verdict: Verdict,
    pub(crate) cases: Vec<CaseResult>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Verdict {
    Qualified,
    Strong,
    Partial,
    Refused,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CaseResult {
    pub(crate) case_id: String,
    pub(crate) tags: BTreeSet<String>,
    pub(crate) completed: bool,
    pub(crate) singularity_status: Option<String>,
    pub(crate) score: u32,
    pub(crate) maximum_score: u32,
    pub(crate) hard_failures: u32,
    pub(crate) elapsed_ms: u128,
    pub(crate) unexpected_paths: Vec<String>,
    pub(crate) graders: Vec<GraderResult>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GraderResult {
    pub(crate) id: String,
    pub(crate) passed: bool,
    pub(crate) points: u32,
    pub(crate) maximum_points: u32,
    pub(crate) hard: bool,
    pub(crate) detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RankingEntry {
    pub(crate) rank: usize,
    pub(crate) model: String,
    pub(crate) score_percent: f64,
    pub(crate) hard_failures: u32,
    pub(crate) completed_cases: u32,
    pub(crate) elapsed_ms: u128,
    pub(crate) verdict: Verdict,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SingularityReport {
    pub(crate) status: String,
}
pub(crate) fn rank(results: &[ModelResult]) -> Vec<RankingEntry> {
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

/// Score floors of the verdicts, on the 0-100 benchmark scale.
const QUALIFIED_SCORE: f64 = 85.0;
const STRONG_SCORE: f64 = 70.0;
const PARTIAL_SCORE: f64 = 40.0;

pub(crate) fn verdict(score: f64, hard_failures: u32, completed: u32, total: u32) -> Verdict {
    if score >= QUALIFIED_SCORE && hard_failures == 0 && completed == total {
        Verdict::Qualified
    } else if score >= STRONG_SCORE && hard_failures == 0 {
        Verdict::Strong
    } else if score >= PARTIAL_SCORE {
        Verdict::Partial
    } else {
        Verdict::Refused
    }
}


pub(crate) fn percent(score: u32, maximum: u32) -> f64 {
    if maximum == 0 {
        0.0
    } else {
        (score as f64 * 10_000.0 / maximum as f64).round() / 100.0
    }
}

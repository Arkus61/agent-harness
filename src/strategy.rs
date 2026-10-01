//! Bounded SESE selection. Unknown hard constraints require a probe, never a guessed PASS.
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RequirementStatus {
    Pass,
    Fail,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequirementAssessment {
    pub status: RequirementStatus,
    #[serde(default)]
    pub evidence: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measurement {
    pub value: f64,
    pub unit: String,
    pub environment_hash: String,
    pub evidence: String,
    pub sample_size: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategyCandidate {
    pub id: String,
    pub mechanism: String,
    #[serde(default)]
    pub requirements: BTreeMap<String, RequirementAssessment>,
    #[serde(default)]
    pub measurements: BTreeMap<String, Measurement>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricDirection {
    Minimize,
    Maximize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricCriterion {
    pub name: String,
    pub unit: String,
    pub direction: MetricDirection,
    pub weight: f64,
}
fn default_candidates() -> usize {
    3
}
fn default_probes() -> usize {
    2
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionPolicy {
    pub hard_requirements: Vec<String>,
    #[serde(default)]
    pub metrics: Vec<MetricCriterion>,
    #[serde(default = "default_candidates")]
    pub max_candidates: usize,
    #[serde(default = "default_probes")]
    pub max_probes: usize,
}
impl Default for SelectionPolicy {
    fn default() -> Self {
        Self {
            hard_requirements: vec![],
            metrics: vec![],
            max_candidates: 3,
            max_probes: 2,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RejectedStrategy {
    pub id: String,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeRequest {
    pub candidate_id: String,
    pub reasons: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionResult {
    pub selected: Option<String>,
    /// Pareto frontier is only populated when comparison inputs are complete and comparable.
    pub pareto: Vec<String>,
    pub rejected: Vec<RejectedStrategy>,
    /// One probe request per candidate, limited by max_probes; none are executed here.
    pub probes: Vec<ProbeRequest>,
    pub unresolved: Vec<String>,
    pub reason: String,
}

/// Callers must obtain measurements and evidence independently. The selector checks their
/// declared applicability, but never claims that an evidence string proves its own contents.
pub fn select(
    candidates: &[StrategyCandidate],
    policy: &SelectionPolicy,
) -> Result<SelectionResult> {
    validate(candidates, policy)?;
    let mut ordered = candidates.iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| a.id.cmp(&b.id));
    let mut mechanisms = BTreeSet::new();
    let mut eligible = vec![];
    let mut rejected = vec![];
    let mut pending: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for candidate in ordered {
        let failed = policy.hard_requirements.iter().find(|requirement| {
            candidate.requirements.get(*requirement).is_some_and(|r| {
                r.status == RequirementStatus::Fail && !r.evidence.trim().is_empty()
            })
        });
        if let Some(failed) = failed {
            rejected.push(RejectedStrategy {
                id: candidate.id.clone(),
                reason: format!("hard requirement failed: {failed}"),
            });
            continue;
        }
        let mechanism = candidate
            .mechanism
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if !mechanisms.insert(mechanism) {
            rejected.push(RejectedStrategy {
                id: candidate.id.clone(),
                reason: "duplicate mechanism; not an independent alternative".into(),
            });
            continue;
        }
        let unknown = policy
            .hard_requirements
            .iter()
            .filter(|requirement| {
                !candidate.requirements.get(*requirement).is_some_and(|r| {
                    r.status == RequirementStatus::Pass && !r.evidence.trim().is_empty()
                })
            })
            .map(|requirement| format!("prove hard requirement: {requirement}"))
            .collect::<Vec<_>>();
        if !unknown.is_empty() {
            pending.insert(candidate.id.clone(), unknown);
        } else {
            eligible.push(candidate);
        }
    }

    // Incomplete metrics prevent comparison; neither missing values nor incompatible hardware
    // silently become zero cost. This version deliberately requests probes before selection.
    for criterion in &policy.metrics {
        let mut environments = BTreeSet::new();
        for candidate in &eligible {
            match candidate.measurements.get(&criterion.name) {
                Some(measurement) if valid_measurement(measurement, criterion) => {
                    environments.insert(measurement.environment_hash.clone());
                }
                _ => {
                    pending
                        .entry(candidate.id.clone())
                        .or_default()
                        .push(format!(
                            "measure {} in {} with source evidence",
                            criterion.name, criterion.unit
                        ));
                }
            }
        }
        if environments.len() > 1 {
            for candidate in &eligible {
                pending
                    .entry(candidate.id.clone())
                    .or_default()
                    .push(format!(
                        "remeasure {} in a shared environment",
                        criterion.name
                    ));
            }
        }
    }
    let unresolved = pending.keys().cloned().collect::<Vec<_>>();
    let probes = pending
        .into_iter()
        .take(policy.max_probes)
        .map(|(candidate_id, reasons)| ProbeRequest {
            candidate_id,
            reasons,
        })
        .collect::<Vec<_>>();
    if !unresolved.is_empty() {
        return Ok(SelectionResult {selected:None,pareto:vec![],rejected,probes,unresolved,reason:"selection deferred: unresolved constraints or incomparable measurements require bounded probes".into()});
    }
    if eligible.is_empty() {
        return Ok(SelectionResult {
            selected: None,
            pareto: vec![],
            rejected,
            probes,
            unresolved,
            reason: "no strategy satisfies all known hard requirements".into(),
        });
    }
    if policy.metrics.is_empty() {
        let pareto = eligible.iter().map(|c| c.id.clone()).collect();
        let selected = if eligible.len() == 1 {
            Some(eligible[0].id.clone())
        } else {
            None
        };
        return Ok(SelectionResult {
            selected,
            pareto,
            rejected,
            probes,
            unresolved,
            reason: if eligible.len() == 1 {
                "single admissible mechanism"
            } else {
                "multiple admissible mechanisms; explicit comparison criteria are required"
            }
            .into(),
        });
    }
    let frontier = eligible
        .iter()
        .copied()
        .filter(|candidate| {
            !eligible.iter().any(|other| {
                other.id != candidate.id && dominates(other, candidate, &policy.metrics)
            })
        })
        .collect::<Vec<_>>();
    let mut best: Option<(&StrategyCandidate, f64)> = None;
    for candidate in &frontier {
        let score = utility(candidate, &eligible, &policy.metrics);
        // Ties are reproducible, never attributed to an invented confidence estimate.
        if best.is_none_or(|(previous, previous_score)| {
            score > previous_score || (score == previous_score && candidate.id < previous.id)
        }) {
            best = Some((candidate, score));
        }
    }
    Ok(SelectionResult {selected:best.map(|(c,_)| c.id.clone()),pareto:frontier.iter().map(|c| c.id.clone()).collect(),rejected,probes,unresolved,
        reason:"measured Pareto frontier, followed by declared weighted normalized utility; equal utility uses stable id order".into()})
}

fn validate(candidates: &[StrategyCandidate], policy: &SelectionPolicy) -> Result<()> {
    ensure!(
        (1..=16).contains(&policy.max_candidates),
        "candidate limit must be 1..16"
    );
    ensure!(policy.max_probes <= 16, "probe limit must be <=16");
    ensure!(
        !candidates.is_empty() && candidates.len() <= policy.max_candidates,
        "candidate count exceeds bounded policy or is empty"
    );
    ensure!(
        !policy.hard_requirements.is_empty(),
        "at least one explicit hard requirement is required"
    );
    ensure!(
        policy
            .hard_requirements
            .iter()
            .all(|r| !r.trim().is_empty()),
        "empty hard requirement"
    );
    ensure!(
        policy
            .hard_requirements
            .iter()
            .collect::<BTreeSet<_>>()
            .len()
            == policy.hard_requirements.len(),
        "duplicate hard requirement"
    );
    let mut ids = BTreeSet::new();
    for candidate in candidates {
        ensure!(
            !candidate.id.trim().is_empty() && !candidate.mechanism.trim().is_empty(),
            "strategy id and mechanism are required"
        );
        ensure!(ids.insert(&candidate.id), "duplicate strategy id");
    }
    let mut metrics = BTreeSet::new();
    for criterion in &policy.metrics {
        ensure!(
            !criterion.name.is_empty() && !criterion.unit.is_empty(),
            "metric name and unit are required"
        );
        ensure!(
            criterion.weight.is_finite() && criterion.weight > 0.0,
            "metric weight must be positive and finite"
        );
        ensure!(
            metrics.insert(&criterion.name),
            "duplicate comparison metric"
        );
    }
    let total_weight: f64 = policy.metrics.iter().map(|m| m.weight).sum();
    ensure!(
        total_weight.is_finite(),
        "sum of metric weights is not finite"
    );
    Ok(())
}
fn valid_measurement(measurement: &Measurement, criterion: &MetricCriterion) -> bool {
    measurement.value.is_finite()
        && measurement.unit == criterion.unit
        && measurement.sample_size > 0
        && !measurement.environment_hash.trim().is_empty()
        && !measurement.evidence.trim().is_empty()
}
fn dominates(a: &StrategyCandidate, b: &StrategyCandidate, criteria: &[MetricCriterion]) -> bool {
    let mut strict = false;
    for criterion in criteria {
        let av = a.measurements[&criterion.name].value;
        let bv = b.measurements[&criterion.name].value;
        match criterion.direction {
            MetricDirection::Minimize => {
                if av > bv {
                    return false;
                }
                strict |= av < bv;
            }
            MetricDirection::Maximize => {
                if av < bv {
                    return false;
                }
                strict |= av > bv;
            }
        }
    }
    strict
}
fn utility(
    candidate: &StrategyCandidate,
    all: &[&StrategyCandidate],
    criteria: &[MetricCriterion],
) -> f64 {
    let total: f64 = criteria.iter().map(|m| m.weight).sum();
    criteria
        .iter()
        .map(|criterion| {
            let values = all
                .iter()
                .map(|c| c.measurements[&criterion.name].value)
                .collect::<Vec<_>>();
            let min = values.iter().copied().fold(f64::INFINITY, f64::min);
            let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            // Scale first to avoid overflow for otherwise finite extreme measurements.
            let scale = min.abs().max(max.abs()).max(1.0);
            let low = min / scale;
            let high = max / scale;
            let value = candidate.measurements[&criterion.name].value / scale;
            let normalized = if high == low {
                1.0
            } else {
                match criterion.direction {
                    MetricDirection::Minimize => (high - value) / (high - low),
                    MetricDirection::Maximize => (value - low) / (high - low),
                }
            };
            (criterion.weight / total) * normalized
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn candidate(id: &str, status: RequirementStatus, latency: f64) -> StrategyCandidate {
        StrategyCandidate {
            id: id.into(),
            mechanism: format!("mechanism {id}"),
            requirements: BTreeMap::from([(
                "portable".into(),
                RequirementAssessment {
                    status,
                    evidence: "platform CI receipts".into(),
                },
            )]),
            measurements: BTreeMap::from([(
                "latency".into(),
                Measurement {
                    value: latency,
                    unit: "ms".into(),
                    environment_hash: "machine-a".into(),
                    evidence: "benchmark.json".into(),
                    sample_size: 30,
                },
            )]),
        }
    }
    fn policy() -> SelectionPolicy {
        SelectionPolicy {
            hard_requirements: vec!["portable".into()],
            metrics: vec![MetricCriterion {
                name: "latency".into(),
                unit: "ms".into(),
                direction: MetricDirection::Minimize,
                weight: 1.0,
            }],
            ..SelectionPolicy::default()
        }
    }
    #[test]
    fn hard_fail_is_excluded_despite_best_metric() {
        let result = select(
            &[
                candidate("unsafe", RequirementStatus::Fail, 0.1),
                candidate("safe", RequirementStatus::Pass, 10.0),
            ],
            &policy(),
        )
        .unwrap();
        assert_eq!(result.selected.as_deref(), Some("safe"));
        assert_eq!(result.rejected.len(), 1);
    }
    #[test]
    fn unknown_and_unsupported_pass_require_bounded_probes() {
        let mut unsupported = candidate("unsupported", RequirementStatus::Pass, 0.1);
        unsupported
            .requirements
            .get_mut("portable")
            .unwrap()
            .evidence
            .clear();
        let mut p = policy();
        p.max_probes = 1;
        let result = select(
            &[
                candidate("unknown", RequirementStatus::Unknown, 0.1),
                unsupported,
                candidate("safe", RequirementStatus::Pass, 10.0),
            ],
            &p,
        )
        .unwrap();
        assert!(result.selected.is_none());
        assert_eq!(result.probes.len(), 1);
        assert_eq!(result.unresolved.len(), 2);
        assert!(result.rejected.is_empty());
    }
    #[test]
    fn missing_metrics_and_different_environments_are_not_compared() {
        let a = candidate("a", RequirementStatus::Pass, 1.0);
        let mut b = candidate("b", RequirementStatus::Pass, 2.0);
        b.measurements.clear();
        assert!(select(&[a.clone(), b.clone()], &policy())
            .unwrap()
            .selected
            .is_none());
        b = candidate("b", RequirementStatus::Pass, 2.0);
        b.measurements.get_mut("latency").unwrap().environment_hash = "machine-b".into();
        assert!(select(&[a, b], &policy()).unwrap().selected.is_none());
    }
    #[test]
    fn pareto_then_declared_utility_and_stable_ties() {
        let result = select(
            &[
                candidate("slow", RequirementStatus::Pass, 8.0),
                candidate("fast", RequirementStatus::Pass, 2.0),
                candidate("medium", RequirementStatus::Pass, 4.0),
            ],
            &policy(),
        )
        .unwrap();
        assert_eq!(result.selected.as_deref(), Some("fast"));
        assert_eq!(result.pareto, ["fast"]);
        let result = select(
            &[
                candidate("z", RequirementStatus::Pass, 2.0),
                candidate("a", RequirementStatus::Pass, 2.0),
            ],
            &policy(),
        )
        .unwrap();
        assert_eq!(result.selected.as_deref(), Some("a"));
        assert_eq!(result.pareto, ["a", "z"]);
    }
    #[test]
    fn genuine_tradeoffs_keep_the_frontier_before_weighted_choice() {
        let mut compact = candidate("compact", RequirementStatus::Pass, 10.0);
        let mut fast = candidate("fast", RequirementStatus::Pass, 1.0);
        let mut dominated = candidate("dominated", RequirementStatus::Pass, 12.0);
        for (candidate, value) in [
            (&mut compact, 1.0),
            (&mut fast, 10.0),
            (&mut dominated, 12.0),
        ] {
            candidate.measurements.insert(
                "memory".into(),
                Measurement {
                    value,
                    unit: "MiB".into(),
                    environment_hash: "machine-a".into(),
                    evidence: "rss-samples.json".into(),
                    sample_size: 30,
                },
            );
        }
        let mut p = policy();
        p.metrics[0].weight = 3.0;
        p.metrics.push(MetricCriterion {
            name: "memory".into(),
            unit: "MiB".into(),
            direction: MetricDirection::Minimize,
            weight: 1.0,
        });
        let result = select(&[compact, fast, dominated], &p).unwrap();
        assert_eq!(result.pareto, ["compact", "fast"]);
        assert_eq!(result.selected.as_deref(), Some("fast"));
    }
    #[test]
    fn duplicate_mechanisms_do_not_create_alternatives() {
        let a = candidate("a", RequirementStatus::Pass, 1.0);
        let mut b = candidate("b", RequirementStatus::Pass, 1.0);
        b.mechanism = a.mechanism.clone();
        let result = select(&[a, b], &policy()).unwrap();
        assert_eq!(result.rejected.len(), 1);
        assert_eq!(result.selected.as_deref(), Some("a"));
        let mut p = policy();
        p.max_candidates = 1;
        assert!(select(
            &[
                candidate("a", RequirementStatus::Pass, 1.0),
                candidate("b", RequirementStatus::Pass, 2.0)
            ],
            &p
        )
        .is_err());
    }
}

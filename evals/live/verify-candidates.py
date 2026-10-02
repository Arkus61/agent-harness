#!/usr/bin/env python3
"""Audit recorded live reports without rerunning models or changing frozen cases.

The generated Rust helper uses the harness's typed serde definitions to reproduce
its exact BLAKE3 candidate manifest. It independently checks protected commands,
review roles and source references. It never calls the engine verification gate.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[2]

RUST_SOURCE = r'''
use agent_harness::types::{hash, Action, RunReport, RunState, Verdict};
use serde_json::{json, Value};
use std::{collections::BTreeSet, fs, path::Path, process::Command};

const REVIEWER: &str = __REVIEWER_LITERAL__;

fn git(repo: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("git").arg("-C").arg(repo).args(args).output()?;
    if !output.status.success() {
        return Err(format!("git failed: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(String::from_utf8(output.stdout)?.trim_end_matches('\n').into())
}

fn evaluate(path: &Path, repo: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let report: RunReport = serde_json::from_slice(&fs::read(path)?)?;
    let run = &report.run;
    let semantic_task_hash = hash(&run.task)?;
    let task_hash_matches = semantic_task_hash == run.task_hash;
    let receipts: Vec<_> = report.events.iter().filter(|e| e.kind == "model.receipt").collect();
    let unknowns = report.events.iter().filter(|e| e.kind == "model.unknown").count();
    let total_input: u64 = receipts.iter().filter_map(|e| e.payload["usage"]["input_tokens"].as_u64()).sum();
    let total_output: u64 = receipts.iter().filter_map(|e| e.payload["usage"]["output_tokens"].as_u64()).sum();
    let receipts_complete = !receipts.is_empty() && receipts.iter().all(|e| e.payload["usage"]["complete"] == true);
    let usage = json!({"input_tokens": total_input, "output_tokens": total_output,
        "receipt_count": receipts.len(), "unknown_calls": unknowns, "all_receipts_complete": receipts_complete,
        "ledger_matches_receipts": total_input + total_output == run.spent_tokens,
        "reserved_tokens": run.reserved_tokens});
    let no_publication = !report.events.iter().any(|e| e.kind == "publication.receipt");
    let Some(candidate) = &run.candidate_sha else {
        return Ok(json!({"status":"NOT_EXECUTED", "reason":"No candidate commit", "run_id":run.id,
            "state":run.state, "task_hash_matches_typed_task":task_hash_matches,
            "usage":usage, "publication_absent":no_publication}));
    };
    let canonical_candidate = git(repo, &["rev-parse", "--verify", &format!("{candidate}^{{commit}}")])?;
    let tree = git(repo, &["rev-parse", &format!("{candidate}^{{tree}}")])?;
    let expected_hash = hash(&(candidate.clone(), tree.clone(), run.task_hash.clone(),
        &run.task.checks, &run.task.grants, run.generation,
        std::env::consts::OS, std::env::consts::ARCH, REVIEWER))?;
    let current_checks: Vec<_> = report.checks.iter().filter(|c| c.candidate_hash == expected_hash).collect();
    let current_reviews: Vec<_> = report.reviews.iter().filter(|r| r.candidate_hash == expected_hash).collect();
    let candidate_event = report.events.iter().rev().find(|e| e.kind == "candidate_set");
    let event_matches = candidate_event.is_some_and(|e| e.payload["sha"] == *candidate
        && e.payload["generation"] == run.generation);
    let checks: Vec<_> = current_checks.iter().map(|check| {
        let identity = serde_json::to_value(&check.command).unwrap();
        let matches = run.task.checks.iter().filter(|expected|
            serde_json::to_value(expected).unwrap() == identity).count();
        json!({"command":check.command, "configured_command_matches":matches,
            "candidate_hash_matches":check.candidate_hash == expected_hash,
            "verdict_pass":check.verdict == Verdict::Pass,
            "exit_zero":check.receipt.exit_code == Some(0),
            "not_timed_out":!check.receipt.timed_out, "not_cancelled":!check.receipt.cancelled})
    }).collect();
    let command_set_exact = current_checks.len() == run.task.checks.len()
        && run.task.checks.iter().all(|expected| current_checks.iter().filter(|actual|
            serde_json::to_value(&actual.command).unwrap() == serde_json::to_value(expected).unwrap()).count() == 1);
    let commands_pass = command_set_exact && current_checks.iter().all(|c|
        c.verdict == Verdict::Pass && c.receipt.exit_code == Some(0) && !c.receipt.timed_out && !c.receipt.cancelled);
    let mut roles = vec![];
    for role in ["requirements", "code", "tests", "security"] {
        let matching: Vec<_> = current_reviews.iter().filter(|r| r.role == role).collect();
        let mut proofs = vec![];
        let mut valid_proofs = false;
        let mut covers_all_requirements = false;
        if matching.len() == 1 {
            let review = matching[0];
            for proof in &review.proofs {
                let path = Path::new(&proof.path);
                let normalized = !path.is_absolute() && path.components().all(|c| matches!(c, std::path::Component::Normal(_)))
                    && !proof.path.split('/').any(|c| c == ".git");
                let grant_permitted = normalized && agent_harness::execution::validate_action(
                    &Action::ReadFile {path:proof.path.clone()}, &run.task.grants, &[], true).is_ok();
                let source = if grant_permitted { git(repo, &["show", &format!("{candidate}:{}", proof.path)]).ok() } else { None };
                let line_exists = source.as_ref().is_some_and(|s| proof.line > 0 && proof.line <= s.lines().count());
                let valid = proof.requirement < run.task.requirements.len() && !proof.explanation.trim().is_empty()
                    && grant_permitted && line_exists;
                proofs.push(json!({"requirement":proof.requirement, "path":proof.path, "line":proof.line,
                    "explanation":proof.explanation, "grant_permitted":grant_permitted, "line_exists_at_commit":line_exists,
                    "source_line":source.as_ref().and_then(|s|s.lines().nth(proof.line.saturating_sub(1))), "valid":valid}));
            }
            valid_proofs = !proofs.is_empty() && proofs.iter().all(|p| p["valid"] == true);
            let coverage: BTreeSet<_> = review.proofs.iter().map(|p| p.requirement).collect();
            covers_all_requirements = coverage.len() == run.task.requirements.len();
        }
        let pass = matching.len() == 1 && matching[0].verdict == Verdict::Pass && !matching[0].fixture
            && !matching[0].findings.iter().any(|f| matches!(f.severity.as_str(), "critical" | "high"))
            && valid_proofs && (role != "requirements" || covers_all_requirements);
        roles.push(json!({"role":role, "matching_review_count":matching.len(), "pass":pass,
            "valid_proofs":valid_proofs, "covers_all_requirements":covers_all_requirements, "proofs":proofs}));
    }
    let four_reviews_pass = roles.iter().all(|r| r["pass"] == true);
    let bindings_valid = canonical_candidate == *candidate && task_hash_matches && event_matches
        && command_set_exact && current_reviews.len() == 4;
    let verified_conditions = bindings_valid && commands_pass && four_reviews_pass && no_publication
        && receipts_complete && unknowns == 0 && run.reserved_tokens == 0
        && total_input + total_output == run.spent_tokens;
    Ok(json!({"status":if bindings_valid {"PASS"} else {"FAIL"}, "run_id":run.id, "state":run.state,
        "candidate_sha":candidate, "candidate_tree":tree, "generation":run.generation,
        "manifest_hash_algorithm":"BLAKE3 typed serde_json tuple", "expected_candidate_hash":expected_hash,
        "task_hash_matches_typed_task":task_hash_matches, "candidate_set_event_matches":event_matches,
        "configured_command_set_exact":command_set_exact, "checks":checks, "commands_pass":commands_pass,
        "roles":roles, "four_reviews_pass":four_reviews_pass, "usage":usage, "publication_absent":no_publication,
        "verified_conditions":verified_conditions, "verified_state_consistent":run.state != RunState::Verified || verified_conditions,
        "limits":"Source references are validated at the exact commit; proof semantic adequacy needs independent manual inspection."}))
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let result = if args.len() == 3 { evaluate(Path::new(&args[1]), Path::new(&args[2])) }
        else { Err("Usage: harness-candidate-proof-helper REPORT REPO".into()) };
    match result {
        Ok(value) => println!("{}", serde_json::to_string_pretty(&value).unwrap()),
        Err(error) => {println!("{}",json!({"status":"ERROR","error":error.to_string()}));std::process::exit(1);}
    }
}
'''


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def build_helper(directory):
    directory.mkdir(parents=True, exist_ok=True)
    literal = re.search(r'^const REVIEWER: &str = (".*");$', (ROOT / 'src/engine.rs').read_text(), re.M)
    if not literal:
        raise RuntimeError('Cannot locate the actual REVIEWER literal')
    source = RUST_SOURCE.replace('__REVIEWER_LITERAL__', literal.group(1))
    (directory / 'src').mkdir(exist_ok=True)
    (directory / 'src/main.rs').write_text(source)
    (directory / 'Cargo.toml').write_text(
        '[package]\nname="harness-candidate-proof-helper"\nversion="0.1.0"\nedition="2021"\n'
        '[dependencies]\nagent-harness={path=' + json.dumps(str(ROOT)) + '}\nserde_json="1"\n')
    environment = os.environ.copy()
    local_cargo = ROOT.parent / '.cargo'
    local_rustup = ROOT.parent / '.rustup'
    cargo_home = environment.get('CARGO_HOME') or (
        local_cargo if (local_cargo / 'bin/cargo').exists() else Path.home() / '.cargo')
    rustup_home = environment.get('RUSTUP_HOME') or (
        local_rustup if local_rustup.is_dir() else Path.home() / '.rustup')
    environment['CARGO_HOME'] = str(cargo_home)
    environment['RUSTUP_HOME'] = str(rustup_home)
    environment['PATH'] = str(Path(cargo_home) / 'bin') + os.pathsep + environment.get('PATH', '')
    environment['CARGO_TARGET_DIR'] = str(ROOT / 'target')
    command = ['cargo', 'build', '--offline', '--manifest-path', str(directory / 'Cargo.toml')]
    result = subprocess.run(command, env=environment, capture_output=True, text=True)
    (directory / 'build.stdout.log').write_text(result.stdout)
    (directory / 'build.stderr.log').write_text(result.stderr)
    if result.returncode:
        raise RuntimeError('Candidate proof helper failed to build; inspect helper logs')
    binary = ROOT / ('target/debug/harness-candidate-proof-helper.exe' if os.name == 'nt'
                     else 'target/debug/harness-candidate-proof-helper')
    (directory / 'provenance.json').write_text(json.dumps({
        'helper_sha256': sha(binary), 'source_sha256': sha(directory / 'src/main.rs'),
        'engine_source_sha256': sha(ROOT / 'src/engine.rs'), 'types_source_sha256': sha(ROOT / 'src/types.rs'),
        'command': command, 'reviewer_literal_sha256': hashlib.sha256(literal.group(1).encode()).hexdigest()
    }, indent=2) + '\n')
    return binary


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--live-dir', type=Path, default=ROOT / 'artifacts/full-validation/live')
    parser.add_argument('--result', action='append', type=Path)
    parser.add_argument('--output', type=Path, default=ROOT / 'artifacts/full-validation/candidate-binding-audit.json')
    parser.add_argument('--helper-dir', type=Path, default=ROOT / 'artifacts/full-validation/candidate-proof-helper')
    parser.add_argument('--helper', type=Path)
    parser.add_argument('--build-only', action='store_true')
    args = parser.parse_args()
    binary = args.helper or build_helper(args.helper_dir)
    if args.build_only:
        print(json.dumps({'status': 'READY', 'helper': str(binary), 'sha256': sha(binary)}))
        return 0
    paths = args.result or sorted(args.live_dir.glob('Q*/trial-*/result.json'))
    audited = []
    for path in paths:
        result = json.loads(path.read_text())
        report_path = Path(result.get('report', path.parent / 'report.json'))
        repo = Path(result['repo'])
        receipt = subprocess.run([str(binary), str(report_path), str(repo)], capture_output=True, text=True, timeout=30)
        audit = json.loads(receipt.stdout)
        audit['task_id'] = result['task_id']
        audit['repeat'] = result.get('repeat')
        audit['result_file'] = str(path)
        if audit.get('candidate_sha'):
            oracle = result.get('oracle', {})
            audit['oracle_candidate_matches'] = oracle.get('candidate_sha') == audit['candidate_sha']
            audit['oracle_tree_matches'] = oracle.get('candidate_tree') == audit['candidate_tree']
            audit['oracle_status'] = oracle.get('status')
            audit['independent_behavior_pass'] = oracle.get('status') == 'PASS'
            audit['strict_trial_success'] = (audit.get('verified_conditions') is True
                and audit['state'] == 'VERIFIED' and audit['oracle_candidate_matches'] and audit['oracle_tree_matches']
                and audit['independent_behavior_pass'] and result['exit_code'] == 0
                and result['original_head_preserved'] and result['original_files_preserved'] and result['task_unchanged'])
        else:
            audit['strict_trial_success'] = False
        (path.parent / 'candidate-binding-audit.json').write_text(json.dumps(audit, ensure_ascii=False, indent=2) + '\n')
        audited.append(audit)
    summary = {'schema_version': 1, 'created_utc': datetime.now(timezone.utc).isoformat(),
        'method': 'Post-hoc exact-commit typed manifest and independent gate audit; no model calls or rerun of protected checks.',
        'helper_sha256': sha(binary), 'wrapper_sha256': sha(Path(__file__)), 'audited_trials': len(audited),
        'candidate_trials': sum(a.get('candidate_sha') is not None for a in audited),
        'no_candidate_trials': sum(a['status'] == 'NOT_EXECUTED' for a in audited),
        'strict_successes': sum(a['strict_trial_success'] for a in audited),
        'binding_failures': sum(a['status'] in ['FAIL', 'ERROR'] for a in audited), 'results': audited}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps({k:v for k,v in summary.items() if k != 'results'}))
    return 1 if summary['binding_failures'] else 0


if __name__ == '__main__':
    raise SystemExit(main())

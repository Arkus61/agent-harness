use agent_harness::{engine, execution::Repo, types::*};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Parser)]
#[command(
    name = "harness",
    version,
    about = "Durable local development-agent harness"
)]
struct Cli {
    #[arg(long, global = true, default_value = ".")]
    repo: PathBuf,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Doctor,
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    Run {
        #[arg(long)]
        task: PathBuf,
    },
    Resume {
        run: String,
    },
    Status {
        run: Option<String>,
    },
    Inspect {
        run: String,
    },
    /// Inspect historical evidence without model calls or event delivery.
    Replay {
        run: String,
    },
    /// Explicit, bounded delivery to a local SQLite journal.
    Outbox {
        #[command(subcommand)]
        command: OutboxCommand,
    },
    Report {
        run: String,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    Cancel {
        run: String,
    },
    Reconcile {
        run: String,
        #[arg(long)]
        action: String,
        #[arg(long,value_parser=["completed","not_executed"])]
        status: String,
        #[arg(long)]
        evidence: String,
    },
    Settle {
        run: String,
        #[arg(long)]
        call: String,
        #[arg(long)]
        input_tokens: u64,
        #[arg(long)]
        output_tokens: u64,
        #[arg(long)]
        evidence: String,
    },
    Merge {
        run: String,
        #[arg(long)]
        target: String,
        #[arg(long)]
        expected: String,
    },
    Eval,
    Demo {
        #[arg(long)]
        dir: PathBuf,
    },
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    Skills {
        #[command(subcommand)]
        command: SkillCommand,
    },
    Strategy {
        #[arg(long)]
        input: PathBuf,
    },
}
#[derive(Subcommand)]
enum AuthCommand {
    Status {
        #[arg(long, default_value = "codex")]
        codex_program: String,
    },
    Chatgpt {
        #[arg(long, default_value = "codex")]
        codex_program: String,
        #[arg(long)]
        device: bool,
        #[arg(long)]
        check: bool,
        #[arg(long)]
        model: Option<String>,
    },
}
#[derive(Subcommand)]
enum OutboxCommand {
    Dispatch {
        #[arg(long)]
        policy: PathBuf,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=100))]
        limit: u32,
    },
    List {
        handler: String,
    },
    Reconcile {
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        event: i64,
    },
}
struct AuthSignalGuard(tokio::task::JoinHandle<()>);
impl Drop for AuthSignalGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}
#[derive(Subcommand)]
enum MemoryCommand {
    Add {
        #[arg(long)]
        kind: String,
        #[arg(long)]
        claim: String,
        #[arg(long)]
        evidence: String,
        #[arg(long)]
        source: String,
    },
    Promote {
        id: String,
        #[arg(long)]
        source: String,
    },
    Search {
        query: String,
        #[arg(long)]
        source: String,
    },
    List,
}
#[derive(Subcommand)]
enum SkillCommand {
    Install {
        file: PathBuf,
    },
    List,
    Quarantine {
        id: String,
    },
    Resolve {
        #[arg(long)]
        task: PathBuf,
    },
}

fn exported<T: serde::Serialize>(value: &T) -> Result<Value> {
    let value = serde_json::to_value(value)?;
    fn collect(v: &Value, secrets: &mut Vec<String>) {
        match v {
            Value::Object(map) => {
                if let Some(Value::Array(values)) = map.get("secrets") {
                    for value in values {
                        if let Some(s) = value.as_str() {
                            secrets.push(s.into());
                        }
                    }
                }
                if let Some(Value::String(key)) = map.get("api_key_env") {
                    if let Ok(s) = std::env::var(key) {
                        if !s.is_empty() {
                            secrets.push(s);
                        }
                    }
                }
                for v in map.values() {
                    collect(v, secrets);
                }
            }
            Value::Array(values) => {
                for v in values {
                    collect(v, secrets);
                }
            }
            _ => {}
        }
    }
    let mut secrets = vec![];
    collect(&value, &mut secrets);
    for key in ["OPENAI_API_KEY", "ANTHROPIC_API_KEY"] {
        if let Ok(s) = std::env::var(key) {
            if !s.is_empty() {
                secrets.push(s);
            }
        }
    }
    Ok(agent_harness::context::redact_value(&value, &secrets))
}
fn print<T: serde::Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&exported(value)?)?);
    Ok(())
}
fn task(path: &Path) -> Result<TaskSpec> {
    let t: TaskSpec = serde_json::from_slice(
        &std::fs::read(path).with_context(|| format!("read task {}", path.display()))?,
    )
    .map_err(|e| {
        let diagnostic = e.to_string();
        let reason = ["unknown field", "missing field", "duplicate field"]
            .iter()
            .find_map(|category| {
                diagnostic
                    .strip_prefix(&format!("{category} `"))
                    .and_then(|tail| {
                        let name = tail.split('`').next()?;
                        (name.len() <= 80
                            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                        .then(|| format!("{category} `{name}`"))
                    })
            })
            .unwrap_or_else(|| {
                if diagnostic.starts_with("invalid type:") {
                    "invalid field type".into()
                } else if diagnostic.starts_with("unknown variant") {
                    "unknown enum variant".into()
                } else {
                    "invalid task schema".into()
                }
            });
        anyhow::anyhow!(
            "{} at line {}, column {}; check TaskSpec fields in README",
            reason,
            e.line(),
            e.column()
        )
    })?;
    t.validate()?;
    Ok(t)
}
fn success(report: &RunReport) -> bool {
    matches!(
        report.run.state,
        RunState::Verified | RunState::FixtureVerified
    )
}

fn main() {
    if let Some(result) = agent_harness::supervisor::entry() {
        if let Err(error) = result {
            eprintln!("native supervisor: {error}");
            std::process::exit(125);
        }
        return;
    }
    // Parse first so --version/--help do not allocate a runtime or touch network/state.
    let cli = Cli::parse();
    let result = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(anyhow::Error::from)
        .and_then(|runtime| runtime.block_on(dispatch(cli)));
    match result {
        Ok(true) => {}
        Ok(false) => std::process::exit(2),
        Err(e) => {
            let diagnostic = exported(&json!({"error":format!("{e:#}")}))
                .unwrap_or_else(|_| json!({"error":"failed to render diagnostic"}));
            eprintln!("{}", diagnostic);
            std::process::exit(1);
        }
    }
}
async fn dispatch(cli: Cli) -> Result<bool> {
    match cli.command {
        Command::Auth { command } => {
            use agent_harness::{codex, provider::Provider};
            use tokio_util::sync::CancellationToken;
            let cancel = CancellationToken::new();
            let signal_cancel = cancel.clone();
            let _signal = AuthSignalGuard(tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    signal_cancel.cancel();
                }
            }));
            match command {
                AuthCommand::Status { codex_program } => {
                    print(&codex::account_status(&codex_program, cancel.child_token()).await?)?;
                }
                AuthCommand::Chatgpt {
                    codex_program,
                    device,
                    check,
                    model,
                } => {
                    let mut status =
                        codex::account_status(&codex_program, cancel.child_token()).await?;
                    if status.get("account_type").and_then(Value::as_str) != Some("chatgpt") {
                        let mut login = tokio::process::Command::new(&codex_program);
                        login
                            .arg("login")
                            .kill_on_drop(true)
                            .env_remove("OPENAI_API_KEY");
                        if device {
                            login.arg("--device-auth");
                        }
                        let mut child = login.spawn().context("could not start Codex login")?;
                        let login_status = tokio::select! {
                            biased;
                            _ = cancel.cancelled() => { let _ = child.kill().await; anyhow::bail!("ChatGPT login cancelled"); }
                            result = child.wait() => result.context("could not wait for Codex login")?,
                        };
                        anyhow::ensure!(login_status.success(), "ChatGPT login did not complete; run codex login --device-auth on your computer");
                        status =
                            codex::account_status(&codex_program, cancel.child_token()).await?;
                    }
                    anyhow::ensure!(
                        status.get("account_type").and_then(Value::as_str) == Some("chatgpt"),
                        "Codex is not authenticated with ChatGPT"
                    );
                    if check {
                        let model = model
                            .or_else(|| {
                                status
                                    .get("default_model")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned)
                            })
                            .unwrap_or_default();
                        let config: ProviderConfig = serde_json::from_value(
                            json!({"kind":"chatgpt","codex_program":codex_program,"model":model,"allow_remote":true,"api_key_env":""}),
                        )?;
                        let provider = Provider::new(&config, &[])?;
                        let response = provider.complete("subscription-check", "You are checking the authenticated model connection. Do not use tools. Return only {\"done\":true,\"summary\":\"ChatGPT connection OK\"}.", "Return the connection confirmation JSON. No project files are provided.", 1024, cancel.child_token()).await?;
                        anyhow::ensure!(
                            response.reply.done && response.reply.actions.is_empty(),
                            "connection check returned unexpected actions"
                        );
                        anyhow::ensure!(
                            response.usage.complete,
                            "model replied but complete usage telemetry is missing"
                        );
                        status["connection_check"] = json!({"status":"PASS","model":model,"usage":response.usage,"reply":response.reply});
                    }
                    print(&status)?;
                }
            }
        }
        Command::Doctor => {
            let repo = Repo::discover_read_only(&cli.repo).ok();
            print(&agent_harness::execution::doctor(repo.as_ref()))?;
        }
        Command::Run { task: path } => {
            let report = engine::run(&cli.repo, Some(task(&path)?), None).await?;
            print(&report)?;
            return Ok(success(&report));
        }
        Command::Resume { run } => {
            let report = engine::run(&cli.repo, None, Some(&run)).await?;
            print(&report)?;
            return Ok(success(&report));
        }
        Command::Status { run } => {
            let (_, store) = engine::open_store(&cli.repo)?;
            match run {
                Some(id) => print(&store.get_run(&id)?)?,
                None => print(&store.list_runs()?)?,
            }
        }
        Command::Inspect { run } => {
            let (_, store) = engine::open_store(&cli.repo)?;
            print(&engine::report(&store, &run)?)?;
        }
        Command::Replay { run } => {
            let repo = Repo::discover_read_only(&cli.repo)?;
            let store = agent_harness::storage::Store::open_read_only(&repo.state_dir)?;
            let snapshot = agent_harness::replay::snapshot(&store, &run)?;
            let consistent = snapshot.projection_consistent;
            print(&snapshot)?;
            return Ok(consistent);
        }
        Command::Outbox { command } => {
            use agent_harness::outbox::{HandlerPolicy, LocalJournalAdapter, OutboxWorker};
            match command {
                OutboxCommand::List { handler } => {
                    let repo = Repo::discover_read_only(&cli.repo)?;
                    let store = agent_harness::storage::Store::open_read_only(&repo.state_dir)?;
                    print(&store.outbox_deliveries(&handler)?)?;
                }
                OutboxCommand::Dispatch { policy, limit } => {
                    let policy: HandlerPolicy = serde_json::from_slice(&std::fs::read(policy)?)?;
                    policy.validate()?;
                    let (repo, store) = engine::open_store(&cli.repo)?;
                    let _lock = repo.lock()?;
                    let adapter = LocalJournalAdapter::open(
                        store.root().join("outbox-journal").join(&policy.handler),
                        &policy.handler,
                    )?;
                    let mut worker = OutboxWorker::new(store, policy, adapter)?;
                    let stamp = u64::try_from(chrono::Utc::now().timestamp_millis())?;
                    print(&worker.tick(stamp, limit)?)?;
                }
                OutboxCommand::Reconcile { policy, event } => {
                    let policy: HandlerPolicy = serde_json::from_slice(&std::fs::read(policy)?)?;
                    policy.validate()?;
                    let (repo, store) = engine::open_store(&cli.repo)?;
                    let _lock = repo.lock()?;
                    let adapter = LocalJournalAdapter::open(
                        store.root().join("outbox-journal").join(&policy.handler),
                        &policy.handler,
                    )?;
                    let mut worker = OutboxWorker::new(store, policy, adapter)?;
                    let stamp = u64::try_from(chrono::Utc::now().timestamp_millis())?;
                    print(&json!({"reconciled":worker.reconcile(event, stamp)?}))?;
                }
            }
        }
        Command::Report { run, output } => {
            let (_, store) = engine::open_store(&cli.repo)?;
            let report = engine::report(&store, &run)?;
            if let Some(path) = output {
                std::fs::write(path, serde_json::to_vec_pretty(&exported(&report)?)?)?;
            } else {
                print(&report)?;
            }
        }
        Command::Cancel { run } => {
            let (_, store) = engine::open_store(&cli.repo)?;
            store.cancel(&run)?;
            print(&store.get_run(&run)?)?;
        }
        Command::Reconcile {
            run,
            action,
            status,
            evidence,
        } => {
            let (repo, store) = engine::open_store(&cli.repo)?;
            let _lock = repo.lock()?;
            anyhow::ensure!(
                !evidence.trim().is_empty(),
                "external reconciliation evidence required"
            );
            anyhow::ensure!(
                store.pending_actions(&run)?.iter().any(|a| a.id == action),
                "action is not pending in this run"
            );
            if status == "completed" {
                store.receipt(&action, json!({"manual":true,"evidence":evidence}))?;
            } else {
                store.reconcile_action(&action, &status)?;
            }
            store.event(
                &run,
                "action.reconciled",
                json!({"action":action,"status":status,"evidence":evidence}),
            )?;
            print(&store.get_run(&run)?)?;
        }
        Command::Settle {
            run,
            call,
            input_tokens,
            output_tokens,
            evidence,
        } => {
            let (repo, store) = engine::open_store(&cli.repo)?;
            let _lock = repo.lock()?;
            anyhow::ensure!(
                !evidence.trim().is_empty(),
                "usage reconciliation evidence required"
            );
            store.settle(
                &run,
                &call,
                &Usage {
                    input_tokens,
                    output_tokens,
                    complete: true,
                },
            )?;
            store.event(
                &run,
                "usage.reconciled",
                json!({"call":call,"evidence":evidence}),
            )?;
            print(&store.get_run(&run)?)?;
        }
        Command::Merge {
            run,
            target,
            expected,
        } => {
            let (repo, store) = engine::open_store(&cli.repo)?;
            engine::publish(&repo, &store, &run, &target, &expected)?;
            print(&json!({"published":true,"run":run,"target":target}))?;
        }
        Command::Eval => {
            let report = agent_harness::evaluation::run_suite().await?;
            print(&report)?;
            return Ok(report["passed"].as_bool().unwrap_or(false));
        }
        Command::Demo { dir } => {
            let path = create_demo(&dir)?;
            let report = engine::run(&dir, Some(task(&path)?), None).await?;
            print(&report)?;
            return Ok(success(&report));
        }
        Command::Memory { command } => {
            let (repo, _) = engine::open_store(&cli.repo)?;
            let db = agent_harness::knowledge::Knowledge::open(
                &repo.state_dir.join("knowledge.sqlite"),
            )?;
            let project = repo.common_dir.to_string_lossy();
            match command {
                MemoryCommand::Add {
                    kind,
                    claim,
                    evidence,
                    source,
                } => print(&json!({"id":db.insert(&project,&kind,&claim,&evidence,&source)?}))?,
                MemoryCommand::Promote { id, source } => {
                    db.promote(&id, &source)?;
                    print(&json!({"promoted":id}))?;
                }
                MemoryCommand::Search { query, source } => {
                    print(&db.retrieve(&project, &query, &source, 10)?)?
                }
                MemoryCommand::List => print(&db.list(&project)?)?,
            }
        }
        Command::Skills { command } => {
            let (repo, _) = engine::open_store(&cli.repo)?;
            let registry =
                agent_harness::skills::SkillRegistry::open(&repo.state_dir.join("skills"))?;
            match command {
                SkillCommand::Install { file } => print(&json!({"hash":registry.install(&file)?}))?,
                SkillCommand::List => print(&registry.list()?)?,
                SkillCommand::Quarantine { id } => {
                    registry.quarantine(&id)?;
                    print(&json!({"quarantined":id}))?;
                }
                SkillCommand::Resolve { task: path } => {
                    let t = task(&path)?;
                    print(&registry.resolve(&t.skills, &t.grants)?)?;
                }
            }
        }
        Command::Strategy { input } => {
            let v: Value = serde_json::from_slice(&std::fs::read(input)?)?;
            let candidates: Vec<agent_harness::strategy::StrategyCandidate> =
                serde_json::from_value(v["candidates"].clone())?;
            let policy: agent_harness::strategy::SelectionPolicy =
                serde_json::from_value(v["policy"].clone())?;
            print(&agent_harness::strategy::select(&candidates, &policy)?)?;
        }
    }
    Ok(true)
}

fn create_demo(dir: &Path) -> Result<PathBuf> {
    anyhow::ensure!(
        !dir.exists(),
        "demo directory already exists; choose a new path"
    );
    std::fs::create_dir_all(dir.join("src"))?;
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname=\"harness-demo\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
    )?;
    std::fs::write(dir.join(".gitignore"), "/target/\n/task.json\n")?;
    let original =
        "pub fn clamp(value: i32, low: i32, high: i32) -> i32 { value.min(low).max(high) }\n";
    let fixed="pub fn clamp(value: i32, low: i32, high: i32) -> i32 { value.max(low).min(high) }\n\n#[cfg(test)] mod tests { use super::*; #[test] fn bounds() { assert_eq!(clamp(5,0,10),5); assert_eq!(clamp(-2,0,10),0); assert_eq!(clamp(20,0,10),10); } }\n";
    std::fs::write(dir.join("src/lib.rs"), original)?;
    let init = std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(dir)
        .status()?;
    anyhow::ensure!(init.success(), "git init failed");
    let lock = std::process::Command::new("cargo")
        .current_dir(dir)
        .args(["generate-lockfile", "--offline"])
        .output()?;
    anyhow::ensure!(
        lock.status.success(),
        "demo needs Cargo: {}",
        String::from_utf8_lossy(&lock.stderr)
    );
    let repo = Repo::discover(dir)?;
    repo.commit(&repo.root, "demo baseline")?;
    let mut scripts = BTreeMap::new();
    scripts.insert(
        "builder:fix".into(),
        vec![ModelReply {
            actions: vec![Action::WriteFile {
                path: "src/lib.rs".into(),
                content: fixed.into(),
                expected_hash: Some(blake3::hash(original.as_bytes()).to_hex().to_string()),
            }],
            done: true,
            ..Default::default()
        }],
    );
    for role in ["requirements", "code", "tests", "security"] {
        scripts.insert(
            format!("review:{role}"),
            vec![ModelReply {
                done: true,
                verdict: Some(Verdict::Pass),
                summary: "Scripted fixture only; real model quality is not evaluated.".into(),
                ..Default::default()
            }],
        );
    }
    for purpose in ["model", "tools", "risk"] {
        scripts.insert(
            format!("decision:{purpose}"),
            vec![ModelReply {
                done: true,
                decision: Some(DecisionAssessment {
                    purpose: purpose.into(),
                    // Only the explicit scripted provider binds this placeholder
                    // to its current typed request; live replies must echo it.
                    subject_hash: String::new(),
                    allow: true,
                    abstain: false,
                    reason: "scripted fixture".into(),
                    choice: (purpose == "model").then(|| "scripted_fixture".into()),
                    tools: if purpose == "tools" {
                        ["read_file", "search", "write_file", "edit_file"]
                            .into_iter()
                            .map(str::to_owned)
                            .collect()
                    } else {
                        vec![]
                    },
                }),
                ..Default::default()
            }],
        );
    }
    let t = TaskSpec {
        schema_version: 1,
        prompt: "Fix clamp for low <= high and add regression tests.".into(),
        requirements: vec![
            "Values below low return low; above high return high; inside remain unchanged.".into(),
            "Add regression tests for all three cases.".into(),
        ],
        grants: Grants {
            read: vec!["src/**".into(), "Cargo.toml".into()],
            write: vec!["src/**".into()],
            commands: vec![],
        },
        checks: vec![CommandSpec {
            program: "cargo".into(),
            args: vec!["test".into(), "--offline".into()],
            timeout_secs: 120,
            resource_limits: None,
        }],
        provider: ProviderConfig {
            codex_program: "codex".into(),
            kind: ProviderKind::Scripted,
            base_url: "http://127.0.0.1:11434/v1".into(),
            model: "fixture".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            allow_remote: false,
            scripts,
        },
        nodes: vec![TaskNode {
            id: "fix".into(),
            prompt: "Fix src/lib.rs".into(),
            requirements: vec![0, 1],
            depends_on: vec![],
            owned_paths: vec!["src/**".into()],
        }],
        profile: RuntimeProfile::NativeTrusted,
        decision_mode: DecisionMode::Enforced,
        budget: Budget {
            max_tokens: 100_000,
            max_output_tokens: 4096,
            deadline_secs: 300,
        },
        concurrency: 2,
        max_steps: 10,
        max_repairs: 0,
        context_bytes: 48_000,
        secrets: vec![],
        skills: vec![],
        protected_paths: vec![],
        command_resource_limits: None,
    };
    let path = dir.join("task.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&t)?)?;
    Ok(path)
}

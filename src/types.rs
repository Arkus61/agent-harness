use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub fn hash<T: Serialize>(value: &T) -> anyhow::Result<String> {
    Ok(blake3::hash(&serde_json::to_vec(value)?)
        .to_hex()
        .to_string())
}
pub fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}
fn one() -> u32 {
    1
}
fn two() -> usize {
    2
}
fn steps() -> usize {
    30
}
fn timeout() -> u64 {
    300
}
fn tokens() -> u64 {
    100_000
}
fn max_output() -> u64 {
    4096
}
fn context_limit() -> usize {
    48_000
}
fn round_limit() -> usize {
    2
}
fn api_url() -> String {
    "http://127.0.0.1:11434/v1".into()
}
fn key_env() -> String {
    "OPENAI_API_KEY".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    #[serde(default = "one")]
    pub schema_version: u32,
    pub prompt: String,
    pub requirements: Vec<String>,
    pub grants: Grants,
    pub checks: Vec<CommandSpec>,
    pub provider: ProviderConfig,
    #[serde(default)]
    pub nodes: Vec<TaskNode>,
    #[serde(default)]
    pub profile: RuntimeProfile,
    #[serde(default)]
    pub decision_mode: DecisionMode,
    #[serde(default)]
    pub budget: Budget,
    #[serde(default = "two")]
    pub concurrency: usize,
    #[serde(default = "steps")]
    pub max_steps: usize,
    #[serde(default = "round_limit")]
    pub max_repairs: usize,
    #[serde(default = "context_limit")]
    pub context_bytes: usize,
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub protected_paths: Vec<String>,
}

impl TaskSpec {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.schema_version == 1, "unsupported schema_version");
        anyhow::ensure!(!self.prompt.trim().is_empty(), "prompt is empty");
        anyhow::ensure!(
            !self.requirements.is_empty() && self.requirements.iter().all(|r| !r.trim().is_empty()),
            "requirements must be nonempty"
        );
        anyhow::ensure!(
            !self.checks.is_empty(),
            "at least one protected command check is required"
        );
        anyhow::ensure!(
            (1..=16).contains(&self.concurrency),
            "concurrency must be 1..16"
        );
        anyhow::ensure!(
            (1..=200).contains(&self.max_steps),
            "max_steps must be 1..200"
        );
        anyhow::ensure!(self.max_repairs <= 10, "max_repairs must be <=10");
        anyhow::ensure!(
            (1024..=1_000_000).contains(&self.context_bytes),
            "context_bytes must be 1024..1000000"
        );
        anyhow::ensure!(
            self.budget.max_tokens > 0
                && self.budget.max_output_tokens > 0
                && self.budget.deadline_secs > 0,
            "budget limits must be positive"
        );
        anyhow::ensure!(
            self.budget.max_output_tokens <= self.budget.max_tokens,
            "max_output_tokens exceeds root budget"
        );
        anyhow::ensure!(
            !self.grants.write.iter().any(|p| p.trim().is_empty()),
            "empty write scope"
        );
        for scope in self
            .grants
            .read
            .iter()
            .chain(&self.grants.write)
            .chain(&self.protected_paths)
        {
            anyhow::ensure!(!scope.contains('\\'), "scope paths use forward slashes");
            globset::Glob::new(scope)?;
        }
        anyhow::ensure!(!self.secrets.iter().any(|s| s.is_empty()), "empty secret");
        for check in &self.checks {
            anyhow::ensure!(
                !check.program.is_empty() && check.timeout_secs > 0,
                "invalid check command"
            );
        }
        if self.provider.kind == ProviderKind::OpenAi {
            anyhow::ensure!(!self.provider.model.is_empty(), "model is required");
            anyhow::ensure!(
                self.provider.base_url.starts_with("http://")
                    || self.provider.base_url.starts_with("https://"),
                "provider must be HTTP(S)"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeProfile {
    #[default]
    NativeTrusted,
    Isolated,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionMode {
    #[default]
    Shadow,
    Enforced,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    #[serde(default = "tokens")]
    pub max_tokens: u64,
    #[serde(default = "max_output")]
    pub max_output_tokens: u64,
    #[serde(default = "timeout")]
    pub deadline_secs: u64,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            max_tokens: tokens(),
            max_output_tokens: max_output(),
            deadline_secs: timeout(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grants {
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
    #[serde(default)]
    pub commands: Vec<CommandGrant>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandGrant {
    pub program: String,
    #[serde(default)]
    pub args_prefix: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandSpec {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "timeout")]
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    #[default]
    Scripted,
    OpenAi,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    #[serde(default)]
    pub kind: ProviderKind,
    #[serde(default = "api_url")]
    pub base_url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "key_env")]
    pub api_key_env: String,
    #[serde(default)]
    pub allow_remote: bool,
    #[serde(default)]
    pub scripts: BTreeMap<String, Vec<ModelReply>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskNode {
    pub id: String,
    pub prompt: String,
    pub requirements: Vec<usize>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub owned_paths: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelReply {
    #[serde(default)]
    pub actions: Vec<Action>,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub verdict: Option<Verdict>,
    #[serde(default)]
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub plan: Vec<TaskNode>,
    #[serde(default)]
    pub decision: Option<DecisionAssessment>,
    #[serde(default)]
    pub proofs: Vec<RequirementProof>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    ReadFile {
        path: String,
    },
    Search {
        query: String,
        #[serde(default)]
        path: Option<String>,
    },
    WriteFile {
        path: String,
        content: String,
        #[serde(default)]
        expected_hash: Option<String>,
    },
    EditFile {
        path: String,
        old: String,
        new: String,
        expected_hash: String,
    },
    RunCommand {
        program: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionAssessment {
    pub purpose: String,
    #[serde(default)]
    pub allow: bool,
    #[serde(default)]
    pub abstain: bool,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub choice: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    Pass,
    Fail,
    Unknown,
    Error,
    NotApplicable,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub requirement: Option<usize>,
    pub severity: String,
    pub message: String,
    pub evidence: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequirementProof {
    pub requirement: usize,
    pub path: String,
    pub line: usize,
    pub explanation: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewResult {
    pub role: String,
    pub candidate_hash: String,
    pub verdict: Verdict,
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub proofs: Vec<RequirementProof>,
    pub context_hash: String,
    pub fixture: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
    pub timed_out: bool,
    pub cancelled: bool,
    pub duration_ms: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    pub command: CommandSpec,
    pub candidate_hash: String,
    pub verdict: Verdict,
    pub receipt: CommandResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RunState {
    Received,
    Planning,
    Executing,
    Integrating,
    Verifying,
    Repairing,
    Verified,
    FixtureVerified,
    Blocked,
    Failed,
    Cancelled,
    BudgetExhausted,
}
impl RunState {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Verified
                | Self::FixtureVerified
                | Self::Failed
                | Self::Cancelled
                | Self::BudgetExhausted
                | Self::Blocked
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub id: String,
    pub repo: String,
    pub task: TaskSpec,
    pub task_hash: String,
    pub base_sha: String,
    pub state: RunState,
    pub generation: u64,
    pub spent_tokens: u64,
    pub reserved_tokens: u64,
    pub created_at: String,
    pub updated_at: String,
    pub cancelled: bool,
    pub candidate_sha: Option<String>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: i64,
    pub run_id: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub created_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRecord {
    pub id: String,
    pub run_id: String,
    pub attempt: String,
    pub action_hash: String,
    pub action: Action,
    pub status: String,
    pub result: Option<serde_json::Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub id: String,
    pub run_id: String,
    pub node_id: String,
    pub generation: u64,
    pub input_sha: String,
    pub output_sha: Option<String>,
    pub worktree: String,
    pub status: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub complete: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderResponse {
    pub reply: ModelReply,
    pub usage: Usage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBundle {
    pub text: String,
    pub sources: BTreeMap<String, String>,
    pub omissions: Vec<String>,
    pub hash: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunReport {
    pub run: RunRecord,
    pub attempts: Vec<AttemptRecord>,
    pub checks: Vec<CheckResult>,
    pub reviews: Vec<ReviewResult>,
    pub events: Vec<Event>,
    pub limitations: Vec<String>,
}

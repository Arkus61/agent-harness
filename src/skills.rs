//! Immutable, content-checked skill packages with bounded exact-version dependencies.
//! Installation is an explicit local operation, not a claim that instructions are safe.
use crate::types::Grants;
use anyhow::{ensure, Context, Result};
use globset::GlobBuilder;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

const MAX_PACKAGE_BYTES: u64 = 512 * 1024;
const MAX_GRAPH_NODES: usize = 128;
const MAX_GRAPH_DEPTH: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillPackage {
    pub id: String,
    pub version: String,
    pub instructions: String,
    #[serde(default)]
    pub required_capabilities: Grants,
    #[serde(default)]
    pub dependencies: Vec<String>,
}
impl SkillPackage {
    pub fn key(&self) -> String {
        format!("{}@{}", self.id, self.version)
    }
    pub fn content_hash(&self) -> Result<String> {
        crate::types::hash(self)
    }
    pub fn validate(&self) -> Result<()> {
        validate_component(&self.id)?;
        validate_component(&self.version)?;
        ensure!(
            !self.instructions.trim().is_empty(),
            "skill instructions are required"
        );
        ensure!(
            self.instructions.len() <= 256 * 1024,
            "skill instructions exceed limit"
        );
        ensure!(self.dependencies.len() <= 32, "too many skill dependencies");
        let mut seen = BTreeSet::new();
        for dependency in &self.dependencies {
            parse_key(dependency)?;
            ensure!(dependency != &self.key(), "skill requires itself");
            ensure!(seen.insert(dependency), "duplicate skill dependency");
        }
        for scope in self
            .required_capabilities
            .read
            .iter()
            .chain(&self.required_capabilities.write)
        {
            validate_scope(scope)?;
        }
        for command in &self.required_capabilities.commands {
            ensure!(
                !command.program.trim().is_empty(),
                "empty skill command program"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillStatus {
    Installed,
    Quarantined,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSummary {
    pub key: String,
    pub content_hash: String,
    pub status: SkillStatus,
}

pub struct SkillRegistry {
    root: PathBuf,
    connection: Mutex<Connection>,
}
impl SkillRegistry {
    pub fn open(root: &Path) -> Result<Self> {
        fs::create_dir_all(root.join("objects"))?;
        let root = root.canonicalize()?;
        let conn = Connection::open(root.join("registry.db"))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS skills(
                key TEXT PRIMARY KEY, hash TEXT NOT NULL, status TEXT NOT NULL,
                installed_at TEXT NOT NULL
            );",
        )?;
        Ok(Self {
            root,
            connection: Mutex::new(conn),
        })
    }

    /// Returns the immutable body hash. A new body requires a new exact version.
    pub fn install(&self, path: &Path) -> Result<String> {
        let metadata = fs::metadata(path)?;
        ensure!(
            metadata.is_file() && metadata.len() <= MAX_PACKAGE_BYTES,
            "invalid or oversized skill package"
        );
        let raw = fs::read(path)?;
        ensure!(
            raw.len() as u64 <= MAX_PACKAGE_BYTES,
            "skill package grew beyond limit"
        );
        let package: SkillPackage = serde_json::from_slice(&raw).context("parse skill JSON")?;
        package.validate()?;
        let body = serde_json::to_vec(&package)?;
        let hash = blake3::hash(&body).to_hex().to_string();
        let key = package.key();
        let mut conn = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("skill registry lock poisoned"))?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT hash,status FROM skills WHERE key=?1",
                [&key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((existing_hash, _)) = existing {
            ensure!(
                existing_hash == hash,
                "immutable skill version already has a different body: {key}"
            );
            self.read_object(&existing_hash)?;
            return Ok(hash);
        }
        // Validate the dependency graph including forward references already in the registry.
        let mut graph = self.load_graph(&tx)?;
        graph.insert(key.clone(), package.dependencies.clone());
        reject_cycles(&graph)?;
        let object_path = self.root.join("objects").join(format!("{hash}.json"));
        if object_path.exists() {
            ensure!(
                fs::read(&object_path)? == body,
                "skill object content mismatch"
            );
        } else {
            // Publish a fully synced object before its SQLite index. A failed index commit can
            // leave an unreferenced object; it cannot leave an index pointing to a partial file.
            let stage = self
                .root
                .join("objects")
                .join(format!(".{}.tmp", uuid::Uuid::new_v4()));
            let result = (|| -> Result<()> {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&stage)?;
                file.write_all(&body)?;
                file.sync_all()?;
                fs::rename(&stage, &object_path)?;
                #[cfg(unix)]
                {
                    std::fs::File::open(self.root.join("objects"))?.sync_all()?;
                }
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(&stage);
            }
            result?;
        }
        tx.execute(
            "INSERT INTO skills(key,hash,status,installed_at) VALUES(?1,?2,'installed',?3)",
            params![key, hash, crate::types::now()],
        )?;
        tx.commit()?;
        Ok(hash)
    }

    /// Dependencies are returned before dependants, exactly once. Permissions are checked at
    /// resolution and must still be checked by ToolGateway for each real action.
    pub fn resolve(&self, ids: &[String], grants: &Grants) -> Result<Vec<SkillPackage>> {
        ensure!(ids.len() <= 32, "too many requested skills");
        let conn = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("skill registry lock poisoned"))?;
        let mut active = BTreeSet::new();
        let mut visited = BTreeSet::new();
        let mut result = vec![];
        for key in ids {
            self.resolve_node(&conn, key, grants, &mut active, &mut visited, &mut result)?;
        }
        Ok(result)
    }

    fn resolve_node(
        &self,
        conn: &Connection,
        key: &str,
        grants: &Grants,
        active: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
        result: &mut Vec<SkillPackage>,
    ) -> Result<()> {
        parse_key(key)?;
        ensure!(
            active.len() < MAX_GRAPH_DEPTH,
            "skill dependency depth exceeds limit"
        );
        if visited.contains(key) {
            return Ok(());
        }
        ensure!(
            visited.len() + active.len() < MAX_GRAPH_NODES,
            "skill graph exceeds node limit"
        );
        ensure!(
            active.insert(key.to_owned()),
            "skill dependency cycle at {key}"
        );
        let row: Option<(String, String)> = conn
            .query_row("SELECT hash,status FROM skills WHERE key=?1", [key], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let (hash, status) =
            row.with_context(|| format!("skill dependency is not installed: {key}"))?;
        ensure!(status == "installed", "skill is quarantined: {key}");
        let package = self.read_object(&hash)?;
        ensure!(
            package.key() == key,
            "registry skill identity does not match body"
        );
        validate_permissions(&package.required_capabilities, grants)
            .with_context(|| format!("skill permissions denied: {key}"))?;
        for dependency in &package.dependencies {
            self.resolve_node(conn, dependency, grants, active, visited, result)?;
        }
        active.remove(key);
        visited.insert(key.to_owned());
        result.push(package);
        Ok(())
    }

    pub fn quarantine(&self, key: &str) -> Result<()> {
        parse_key(key)?;
        let conn = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("skill registry lock poisoned"))?;
        ensure!(
            conn.execute("UPDATE skills SET status='quarantined' WHERE key=?1", [key])? == 1,
            "skill not found: {key}"
        );
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<SkillSummary>> {
        let conn = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("skill registry lock poisoned"))?;
        let mut statement = conn.prepare("SELECT key,hash,status FROM skills ORDER BY key")?;
        let rows = statement
            .query_map([], |row| {
                let status: String = row.get(2)?;
                Ok(SkillSummary {
                    key: row.get(0)?,
                    content_hash: row.get(1)?,
                    status: if status == "installed" {
                        SkillStatus::Installed
                    } else {
                        SkillStatus::Quarantined
                    },
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn read_object(&self, hash: &str) -> Result<SkillPackage> {
        ensure!(
            hash.len() == 64 && hash.bytes().all(|c| c.is_ascii_hexdigit()),
            "invalid skill content hash"
        );
        let path = self.root.join("objects").join(format!("{hash}.json"));
        ensure!(
            fs::metadata(&path)?.len() <= MAX_PACKAGE_BYTES,
            "skill object too large"
        );
        let bytes = fs::read(path)?;
        ensure!(
            blake3::hash(&bytes).to_hex().as_str() == hash,
            "skill object hash mismatch"
        );
        let package: SkillPackage = serde_json::from_slice(&bytes)?;
        package.validate()?;
        Ok(package)
    }
    fn load_graph(&self, conn: &Connection) -> Result<BTreeMap<String, Vec<String>>> {
        let mut stmt = conn.prepare("SELECT key,hash FROM skills ORDER BY key")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut graph = BTreeMap::new();
        for (key, hash) in rows {
            graph.insert(key, self.read_object(&hash)?.dependencies);
        }
        Ok(graph)
    }
}

fn validate_component(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= 100 && value != "." && value != "..",
        "invalid skill identifier"
    );
    ensure!(
        !value.eq_ignore_ascii_case("latest"),
        "latest is not an immutable skill version"
    );
    ensure!(
        value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.')),
        "invalid skill identifier: {value}"
    );
    Ok(())
}
fn parse_key(key: &str) -> Result<(&str, &str)> {
    let (id, version) = key
        .split_once('@')
        .context("skill requires exact id@version")?;
    validate_component(id)?;
    validate_component(version)?;
    Ok((id, version))
}
fn validate_scope(scope: &str) -> Result<()> {
    ensure!(
        !scope.is_empty() && !scope.starts_with(['/', '\\']) && !scope.contains(['\\', ':']),
        "skill scope must be a portable relative glob"
    );
    // Reject parent syntax inside glob alternations as well as ordinary path components.
    // Skill scope validation is conservative; unusual dotted names can be expressed with
    // a narrower runtime grant rather than a potentially ambiguous package glob.
    ensure!(!scope.contains(".."), "skill scope cannot traverse parent");
    GlobBuilder::new(scope)
        .literal_separator(true)
        .backslash_escape(false)
        .build()?;
    Ok(())
}
fn scope_contains(granted: &str, requested: &str) -> Result<bool> {
    validate_scope(granted)?;
    validate_scope(requested)?;
    if granted == requested || granted == "**" {
        return Ok(true);
    }
    if let Some(prefix) = granted.strip_suffix("/**") {
        if !prefix.contains(['*', '?', '[', '{']) && requested.starts_with(&format!("{prefix}/")) {
            return Ok(true);
        }
    }
    if !requested.contains(['*', '?', '[', '{']) {
        return Ok(GlobBuilder::new(granted)
            .literal_separator(true)
            .backslash_escape(false)
            .build()?
            .compile_matcher()
            .is_match(requested));
    }
    // General glob containment is deliberately not guessed.
    Ok(false)
}
fn validate_permissions(required: &Grants, available: &Grants) -> Result<()> {
    for (requested, granted) in [
        (&required.read, &available.read),
        (&required.write, &available.write),
    ] {
        for scope in requested {
            let mut allowed = false;
            for grant in granted {
                if scope_contains(grant, scope)? {
                    allowed = true;
                    break;
                }
            }
            ensure!(allowed, "required file scope is not granted: {scope}");
        }
    }
    for command in &required.commands {
        ensure!(
            available
                .commands
                .iter()
                .any(|grant| grant.program == command.program
                    && command.args_prefix.starts_with(&grant.args_prefix)),
            "required command is not granted: {}",
            command.program
        );
    }
    Ok(())
}
fn reject_cycles(graph: &BTreeMap<String, Vec<String>>) -> Result<()> {
    fn visit(
        key: &str,
        graph: &BTreeMap<String, Vec<String>>,
        active: &mut BTreeSet<String>,
        done: &mut BTreeSet<String>,
        depth: usize,
    ) -> Result<()> {
        if done.contains(key) {
            return Ok(());
        }
        ensure!(depth < MAX_GRAPH_DEPTH, "skill graph depth exceeds limit");
        ensure!(
            active.insert(key.to_owned()),
            "skill dependency cycle at {key}"
        );
        if let Some(dependencies) = graph.get(key) {
            for dependency in dependencies {
                visit(dependency, graph, active, done, depth + 1)?;
            }
        }
        active.remove(key);
        done.insert(key.to_owned());
        Ok(())
    }
    let mut done = BTreeSet::new();
    for key in graph.keys() {
        visit(key, graph, &mut BTreeSet::new(), &mut done, 0)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CommandGrant;
    fn package(id: &str, dependencies: Vec<String>) -> SkillPackage {
        SkillPackage {
            id: id.into(),
            version: "1.0".into(),
            instructions: "Read exact sources before proposing a patch.".into(),
            required_capabilities: Grants::default(),
            dependencies,
        }
    }
    fn install(registry: &SkillRegistry, dir: &Path, package: &SkillPackage) -> Result<String> {
        let path = dir.join(format!("{}.json", package.id));
        fs::write(&path, serde_json::to_vec(package)?)?;
        registry.install(&path)
    }
    #[test]
    fn immutable_versions_content_checks_and_quarantine() {
        let dir = tempfile::tempdir().unwrap();
        let registry = SkillRegistry::open(&dir.path().join("registry")).unwrap();
        let mut pkg = package("read", vec![]);
        let hash = install(&registry, dir.path(), &pkg).unwrap();
        assert_eq!(install(&registry, dir.path(), &pkg).unwrap(), hash);
        pkg.instructions.push_str(" changed");
        assert!(install(&registry, dir.path(), &pkg).is_err());
        assert_eq!(
            registry
                .resolve(&["read@1.0".into()], &Grants::default())
                .unwrap()
                .len(),
            1
        );
        registry.quarantine("read@1.0").unwrap();
        assert!(registry
            .resolve(&["read@1.0".into()], &Grants::default())
            .is_err());
        assert!(install(&registry, dir.path(), &package("read", vec![])).is_ok());
        assert!(registry
            .resolve(&["read@1.0".into()], &Grants::default())
            .is_err());
    }
    #[test]
    fn dependencies_topological_cycles_and_missing() {
        let dir = tempfile::tempdir().unwrap();
        let registry = SkillRegistry::open(&dir.path().join("registry")).unwrap();
        install(
            &registry,
            dir.path(),
            &package("fix", vec!["read@1.0".into()]),
        )
        .unwrap();
        assert!(registry
            .resolve(&["fix@1.0".into()], &Grants::default())
            .is_err());
        assert!(install(
            &registry,
            dir.path(),
            &package("read", vec!["fix@1.0".into()])
        )
        .is_err());
        install(&registry, dir.path(), &package("read", vec![])).unwrap();
        let resolved = registry
            .resolve(&["fix@1.0".into(), "read@1.0".into()], &Grants::default())
            .unwrap();
        assert_eq!(
            resolved.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            ["read", "fix"]
        );
        registry.quarantine("read@1.0").unwrap();
        assert!(registry
            .resolve(&["fix@1.0".into()], &Grants::default())
            .is_err());
    }
    #[test]
    fn capability_subsets_do_not_expand_rights() {
        let dir = tempfile::tempdir().unwrap();
        let registry = SkillRegistry::open(&dir.path().join("registry")).unwrap();
        let mut pkg = package("test", vec![]);
        pkg.required_capabilities.read = vec!["src/**/*.rs".into()];
        pkg.required_capabilities.commands = vec![CommandGrant {
            program: "cargo".into(),
            args_prefix: vec!["test".into(), "--lib".into()],
        }];
        install(&registry, dir.path(), &pkg).unwrap();
        let grants = Grants {
            read: vec!["src/**".into()],
            write: vec![],
            commands: vec![CommandGrant {
                program: "cargo".into(),
                args_prefix: vec!["test".into()],
            }],
        };
        assert!(registry.resolve(&["test@1.0".into()], &grants).is_ok());
        assert!(registry
            .resolve(&["test@1.0".into()], &Grants::default())
            .is_err());
        assert!(!scope_contains("src/*.rs", "src/**/*.rs").unwrap());
        assert!(scope_contains("src/**", "src/lib.rs").unwrap());
        assert!(scope_contains("**", "../private").is_err());
        assert!(scope_contains("src/**", "src/{../private,file.rs}").is_err());
    }
    #[test]
    fn tampered_objects_and_unpinned_versions_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("registry");
        let registry = SkillRegistry::open(&root).unwrap();
        let hash = install(&registry, dir.path(), &package("read", vec![])).unwrap();
        fs::write(root.join("objects").join(format!("{hash}.json")), b"{}").unwrap();
        assert!(registry
            .resolve(&["read@1.0".into()], &Grants::default())
            .is_err());
        for id in ["read", "read@latest", "../read@1.0", "read@*"] {
            assert!(registry.resolve(&[id.into()], &Grants::default()).is_err());
        }
    }
}

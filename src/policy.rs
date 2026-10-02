//! Shared path boundary for initial context and explicit gateway operations.
use anyhow::{bail, ensure, Result};
use std::path::{Component, Path, PathBuf};

pub fn is_sensitive_path(path: &Path) -> bool {
    path.components().any(|component| {
        let Component::Normal(name) = component else {
            return false;
        };
        let name = name.to_string_lossy().to_ascii_lowercase();
        matches!(
            name.as_str(),
            ".git"
                | ".harness"
                | ".ssh"
                | ".aws"
                | ".azure"
                | ".codex"
                | ".claude"
                | ".gnupg"
                | ".kube"
                | ".docker"
                | ".netrc"
                | ".npmrc"
                | ".pypirc"
                | "credentials"
                | "credentials.json"
                | "secrets.json"
                | "id_rsa"
                | "id_ed25519"
                | "id_ecdsa"
                | "id_dsa"
        ) || name.starts_with("credentials.")
            || name == "secrets"
            || name.starts_with("secrets.")
            || name == "auth.json"
            || name == ".env"
            || name.starts_with(".env.")
            || name.ends_with(".pem")
            || name.ends_with(".key")
            || name.ends_with(".p12")
            || name.ends_with(".pfx")
    })
}

pub fn relative_path(value: &str) -> Result<PathBuf> {
    ensure!(!value.contains('\0'), "path contains NUL");
    // Backslashes are separators on Windows, not escape syntax. Reject them on
    // Unix too so a grant cannot have different traversal meaning across OSes.
    ensure!(
        !value.contains('\\'),
        "use forward slashes in gateway paths"
    );
    ensure!(
        !value.contains(':'),
        "drive prefixes and alternate data streams are forbidden"
    );
    let path = Path::new(value);
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let name = value.to_string_lossy();
                ensure!(
                    !name.ends_with([' ', '.']),
                    "trailing spaces/dots are not portable gateway paths"
                );
                let stem = name.split('.').next().unwrap_or("").to_ascii_lowercase();
                ensure!(
                    !matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
                        && !(stem.len() == 4
                            && (stem.starts_with("com") || stem.starts_with("lpt"))
                            && stem.as_bytes()[3].is_ascii_digit()),
                    "Windows device names are forbidden"
                );
                normalized.push(value)
            }
            Component::CurDir => {}
            _ => bail!("gateway paths must be relative without parent traversal"),
        }
    }
    ensure!(!normalized.as_os_str().is_empty(), "file path is empty");
    ensure!(
        !is_sensitive_path(&normalized),
        "protected file or directory"
    );
    Ok(normalized)
}

/// Match immutable task paths after the same portable normalization as the gateway.
pub fn ensure_unprotected(path: &str, patterns: &[String]) -> Result<()> {
    let normalized = relative_path(path)?;
    for pattern in patterns {
        ensure!(
            !globset::Glob::new(pattern)?
                .compile_matcher()
                .is_match(&normalized),
            "protected acceptance source cannot be modified"
        );
    }
    Ok(())
}

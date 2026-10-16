use std::path::{Path, PathBuf};
use tracing::{debug, error, info};

use super::cookies::SessionData;
use crate::domain::{ScraperError, WafProtection};

/// Solver that invokes `bun run scripts/harvest_tokens.ts` as a sub-process
/// to compute anti-bot PoW challenges and harvest fresh session cookies.
#[derive(Debug, Clone)]
pub struct BunSolver {
    bun_bin: PathBuf,
    script_path: PathBuf,
}

impl Default for BunSolver {
    fn default() -> Self {
        Self::new(Self::find_bun(), Self::find_script())
    }
}

impl BunSolver {
    /// Create a new BunSolver with specific paths to Bun and harvest_tokens.ts.
    pub fn new(bun_bin: impl Into<PathBuf>, script_path: impl Into<PathBuf>) -> Self {
        Self {
            bun_bin: bun_bin.into(),
            script_path: script_path.into(),
        }
    }

    /// Locate `bun` executable in system paths.
    pub fn find_bun() -> PathBuf {
        if let Ok(path) = std::env::var("BUN_PATH") {
            return PathBuf::from(path);
        }

        // NixOS / system default
        let nixos_bun = Path::new("/run/current-system/sw/bin/bun");
        if nixos_bun.exists() {
            return nixos_bun.to_path_buf();
        }

        // Standard user or PATH fallback
        PathBuf::from("bun")
    }

    /// Locate `scripts/harvest_tokens.ts` script.
    pub fn find_script() -> PathBuf {
        if let Ok(path) = std::env::var("RU_MARKETPLACE_HARVEST_SCRIPT") {
            return PathBuf::from(path);
        }

        // Check relative to current working directory
        let local_script = Path::new("scripts/harvest_tokens.ts");
        if local_script.exists() {
            return local_script.to_path_buf();
        }

        // Check relative to executable directory
        if let Ok(exe) = std::env::current_exe()
            && let Some(parent) = exe.parent()
        {
            let candidate1 = parent.join("scripts/harvest_tokens.ts");
            if candidate1.exists() {
                return candidate1;
            }
            if let Some(grandparent) = parent.parent() {
                let candidate2 = grandparent.join("scripts/harvest_tokens.ts");
                if candidate2.exists() {
                    return candidate2;
                }
            }
        }

        // Project root default fallback
        PathBuf::from("scripts/harvest_tokens.ts")
    }

    /// Execute `harvest_tokens.ts` for the target marketplace and return harvested session.
    pub async fn harvest(&self, marketplace: &str) -> Result<SessionData, ScraperError> {
        let m_name = marketplace.to_lowercase();
        info!(
            marketplace = %m_name,
            bun = ?self.bun_bin,
            script = ?self.script_path,
            "Invoking Bun solver to harvest fresh session tokens..."
        );

        let mut cmd = tokio::process::Command::new(&self.bun_bin);
        cmd.arg("run")
            .arg(&self.script_path)
            .arg("--marketplace")
            .arg(&m_name)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let output = cmd.output().await.map_err(|e| {
            error!(
                error = %e,
                bun = ?self.bun_bin,
                "Failed to spawn Bun token solver process"
            );
            ScraperError::WafBlocked {
                waf: WafProtection::Wbaas,
                details: format!("Failed to spawn Bun token solver: {e}"),
            }
        })?;

        let stdout_str = String::from_utf8_lossy(&output.stdout);
        let stderr_str = String::from_utf8_lossy(&output.stderr);

        if !stderr_str.trim().is_empty() {
            debug!(
                stderr = %stderr_str.trim(),
                "Bun token solver stderr diagnostics"
            );
        }

        if !output.status.success() {
            error!(
                status = ?output.status,
                stderr = %stderr_str.trim(),
                "Bun token solver exited with non-zero status"
            );
            return Err(ScraperError::WafBlocked {
                waf: WafProtection::Wbaas,
                details: format!(
                    "Bun solver failed (code {:?}): {}",
                    output.status.code(),
                    stderr_str.trim()
                ),
            });
        }

        // Locate JSON object in stdout (ignoring any other lines)
        let json_line = stdout_str
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with('{') && line.ends_with('}'))
            .ok_or_else(|| {
                error!(stdout = %stdout_str, "No JSON payload found in Bun solver stdout");
                ScraperError::Parse(format!(
                    "No JSON session payload output from solver: {stdout_str}"
                ))
            })?;

        let session: SessionData = serde_json::from_str(json_line).map_err(|e| {
            error!(
                error = %e,
                raw = %json_line,
                "Failed to deserialize session payload from Bun solver"
            );
            ScraperError::Parse(format!("Failed to parse solver session JSON: {e}"))
        })?;

        info!(
            marketplace = %session.marketplace,
            expires_at = session.expires_at,
            "Successfully harvested fresh tokens via Bun solver"
        );

        Ok(session)
    }
}

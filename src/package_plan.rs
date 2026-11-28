//! Package plan consumption for the installer.
//!
//! The installer's package step consumes the JSON plan emitted by
//! `sigmaos-pkg-solver solve --json` (itself fed by
//! `sigmaos-pockets export`). This module parses that plan into an
//! ordered install sequence — installs first, then upgrades — so the
//! installer never has to reimplement dependency resolution.

use serde::Deserialize;
use thiserror::Error;

/// Errors parsing a package plan.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PlanError {
    /// The plan JSON could not be parsed.
    #[error("invalid plan JSON: {0}")]
    Json(String),
    /// A plan entry was malformed (missing `=` or `:` separators).
    #[error("malformed plan entry: {0:?}")]
    Malformed(String),
}

/// Raw solver plan as emitted by `sigmaos-pkg-solver --json`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SolverPlan {
    /// `"name=version"` entries for new installs.
    #[serde(default)]
    pub install: Vec<String>,
    /// `"name:old->new"` entries for upgrades.
    #[serde(default)]
    pub upgrade: Vec<String>,
    /// `"name=version"` entries for packages kept as-is.
    #[serde(default)]
    pub keep: Vec<String>,
}

/// One resolved package step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageStep {
    /// Package name.
    pub name: String,
    /// Target version.
    pub version: String,
    /// Whether this replaces an installed version.
    pub is_upgrade: bool,
    /// Previous version when upgrading.
    pub old_version: Option<String>,
}

impl SolverPlan {
    /// Parse solver `--json` output.
    pub fn from_json(text: &str) -> Result<Self, PlanError> {
        serde_json::from_str(text).map_err(|e| PlanError::Json(e.to_string()))
    }

    /// Ordered install sequence: installs first (solver emits them in
    /// dependency order), then upgrades, then kept packages marked
    /// read-only for reporting.
    pub fn install_sequence(&self) -> Result<Vec<PackageStep>, PlanError> {
        let mut steps = Vec::new();
        for e in &self.install {
            steps.push(parse_name_version(e, false)?);
        }
        for e in &self.upgrade {
            steps.push(parse_upgrade(e)?);
        }
        for e in &self.keep {
            let mut s = parse_name_version(e, false)?;
            s.is_upgrade = false;
            steps.push(s);
        }
        Ok(steps)
    }

    /// Packages that will actually change on disk (installs + upgrades).
    pub fn changed(&self) -> usize {
        self.install.len() + self.upgrade.len()
    }

    /// Human-readable summary for progress UIs.
    pub fn summary(&self) -> String {
        format!(
            "{} install(s), {} upgrade(s), {} kept",
            self.install.len(),
            self.upgrade.len(),
            self.keep.len()
        )
    }

    /// Ordered steps that mutate the system (installs + upgrades),
    /// excluding kept packages — what the installer must execute.
    pub fn mutations(&self) -> Result<Vec<PackageStep>, PlanError> {
        let mut steps = Vec::new();
        for e in &self.install {
            steps.push(parse_name_version(e, false)?);
        }
        for e in &self.upgrade {
            steps.push(parse_upgrade(e)?);
        }
        Ok(steps)
    }
}

fn parse_name_version(entry: &str, _upgrade: bool) -> Result<PackageStep, PlanError> {
    let (name, version) = entry
        .split_once('=')
        .ok_or_else(|| PlanError::Malformed(entry.to_string()))?;
    Ok(PackageStep {
        name: name.to_string(),
        version: version.to_string(),
        is_upgrade: false,
        old_version: None,
    })
}

fn parse_upgrade(entry: &str) -> Result<PackageStep, PlanError> {
    // "name:old->new"
    let (name, rest) = entry
        .split_once(':')
        .ok_or_else(|| PlanError::Malformed(entry.to_string()))?;
    let (old, new) = rest
        .split_once("->")
        .ok_or_else(|| PlanError::Malformed(entry.to_string()))?;
    Ok(PackageStep {
        name: name.to_string(),
        version: new.to_string(),
        is_upgrade: true,
        old_version: Some(old.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_solver_json() {
        let json = r#"{"install":["a=1.0","b=2.0"],"upgrade":["c:1.0->2.0"],"keep":["d=3.0"]}"#;
        let plan = SolverPlan::from_json(json).unwrap();
        assert_eq!(plan.install.len(), 2);
        assert_eq!(plan.upgrade.len(), 1);
        assert_eq!(plan.keep.len(), 1);
        assert_eq!(plan.changed(), 3);
    }

    #[test]
    fn install_sequence_orders_and_marks() {
        let json = r#"{"install":["a=1.0"],"upgrade":["c:1.0->2.0"],"keep":["d=3.0"]}"#;
        let plan = SolverPlan::from_json(json).unwrap();
        let seq = plan.install_sequence().unwrap();
        assert_eq!(seq.len(), 3);
        assert_eq!(seq[0].name, "a");
        assert!(!seq[0].is_upgrade);
        assert_eq!(seq[1].name, "c");
        assert!(seq[1].is_upgrade);
        assert_eq!(seq[1].old_version.as_deref(), Some("1.0"));
        assert_eq!(seq[2].name, "d");
    }

    #[test]
    fn malformed_entries_rejected() {
        let json = r#"{"install":["noequals"],"upgrade":[],"keep":[]}"#;
        let plan = SolverPlan::from_json(json).unwrap();
        assert!(matches!(
            plan.install_sequence(),
            Err(PlanError::Malformed(_))
        ));
        let bad_up = r#"{"install":[],"upgrade":["badformat"],"keep":[]}"#;
        let plan2 = SolverPlan::from_json(bad_up).unwrap();
        assert!(plan2.install_sequence().is_err());
    }

    #[test]
    fn mutations_exclude_keeps() {
        let json = r#"{"install":["a=1.0"],"upgrade":["c:1.0->2.0"],"keep":["d=3.0"]}"#;
        let plan = SolverPlan::from_json(json).unwrap();
        let muts = plan.mutations().unwrap();
        assert_eq!(muts.len(), 2);
        assert!(muts.iter().all(|s| s.name != "d"));
    }

    #[test]
    fn invalid_json_rejected() {
        assert!(matches!(
            SolverPlan::from_json("not json"),
            Err(PlanError::Json(_))
        ));
    }

    #[test]
    fn empty_plan_ok() {
        let plan = SolverPlan::from_json("{}").unwrap();
        assert_eq!(plan.changed(), 0);
        assert!(plan.install_sequence().unwrap().is_empty());
    }
}

//! SigmaOS Installer
//!
//! Real installation engine: sysfs disk discovery, sector-accurate GPT/MBR
//! partition planning, LUKS2 encryption, filesystem creation, mount tree,
//! fstab generation, bootloader installation, and post-install configuration.
//!
//! # Safety model
//!
//! All external commands go through [`executor::Executor`]. The default
//! [`executor::DryRunExecutor`] records the full command plan without side
//! effects; [`executor::SystemExecutor`] performs real operations and must
//! only be used after explicit user confirmation of the partition plan.
//!
//! # Principles
//!
//! - FAIL-FAST: invalid configs and disks are rejected before any command runs.
//! - SEPARATION OF MECHANISM & POLICY: [`device`]/[`layout`]/[`executor`] are
//!   mechanism; [`config`]/[`engine`] hold policy.
//! - DETERMINISTIC TESTING: AAA tests run the whole pipeline in dry-run mode.

pub mod config;
pub mod device;
pub mod engine;
pub mod executor;
pub mod layout;

// Keep the original minimal API surface working for existing consumers.
pub use config::{Bootloader, InstallConfig, RootFsChoice};
pub use device::{BlockDevice, DeviceProbe, StaticProbe, SysfsProbe};
pub use engine::{dry_run_engine, InstallPlan, InstallReport, InstallState, InstallerEngine, PlanError};
pub use executor::{CommandOutput, CommandSpec, DryRunExecutor, ExecError, Executor, SystemExecutor};
pub use layout::{FsKind, LayoutError, LayoutOptions, Partition, PartitionRole, TableKind};

/// Legacy compatibility shim: target disk + EFI size check.
/// New code should use [`InstallerEngine`] + [`InstallConfig`].
#[derive(Debug, Clone)]
pub struct InstallationTarget {
    /// Target disk kernel name.
    pub target_disk: String,
    /// Whether to encrypt with LUKS2.
    pub encrypt_luks2: bool,
    /// EFI partition size in MiB.
    pub efi_partition_mb: u64,
}

/// Legacy installer facade delegating to [`InstallerEngine`].
#[derive(Debug, Default)]
pub struct InstallerEngineLegacy {
    target: Option<InstallationTarget>,
}

impl InstallerEngineLegacy {
    /// Create a new legacy facade.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the target (legacy API).
    pub fn configure(&mut self, disk: &str, encrypt: bool, efi_mb: u64) {
        self.target = Some(InstallationTarget {
            target_disk: disk.to_string(),
            encrypt_luks2: encrypt,
            efi_partition_mb: efi_mb,
        });
    }

    /// Whether a minimally valid target is configured.
    pub fn is_ready(&self) -> bool {
        self.target
            .as_ref()
            .map(|t| !t.target_disk.is_empty() && t.efi_partition_mb >= 512)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // AAA: legacy API still works.
    #[test]
    fn legacy_installer_readiness() {
        let mut inst = InstallerEngineLegacy::new();
        assert!(!inst.is_ready());
        inst.configure("/dev/nvme0n1", true, 512);
        assert!(inst.is_ready());
        inst.configure("/dev/nvme0n1", true, 256);
        assert!(!inst.is_ready());
    }
}

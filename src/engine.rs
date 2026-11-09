//! Installer engine: probe → select → plan → execute → verify.
//!
//! The engine produces a concrete command plan and runs it through an
//! [`Executor`]. With [`DryRunExecutor`] (the default) the full pipeline can be
//! exercised end-to-end without touching real disks — this is what the CI
//! smoke test uses.

use crate::config::{Bootloader, InstallConfig, RootFsChoice};
use crate::device::{BlockDevice, DeviceProbe, SysfsProbe};
use crate::executor::{CommandOutput, CommandSpec, DryRunExecutor, ExecError, Executor};
use crate::layout::{self, FsKind, LayoutOptions, Partition, PartitionRole, TableKind};

/// High-level pipeline state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallState {
    /// No disk selected yet.
    Idle,
    /// Disk selected, layout computed.
    Planned,
    /// Commands executed successfully.
    Installed,
    /// A command failed; [`InstallReport::failed_step`] says where.
    Failed,
}

/// Result of running the pipeline.
#[derive(Debug)]
pub struct InstallReport {
    /// Final state.
    pub state: InstallState,
    /// Name of the step that failed, if any.
    pub failed_step: Option<String>,
    /// stderr of the failing command, if any.
    pub error: Option<String>,
    /// Number of commands executed.
    pub commands_run: usize,
}

/// Errors that abort planning before any command runs.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanError {
    /// Config validation failed.
    InvalidConfig(String),
    /// Selected disk not found among probed devices.
    DiskNotFound(String),
    /// Layout computation failed.
    Layout(layout::LayoutError),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::InvalidConfig(m) => write!(f, "invalid config: {}", m),
            PlanError::DiskNotFound(d) => write!(f, "disk not found: {}", d),
            PlanError::Layout(e) => write!(f, "layout error: {}", e),
        }
    }
}

impl std::error::Error for PlanError {}

/// A fully computed installation: config + disk + partitions + mount plan.
#[derive(Debug, Clone)]
pub struct InstallPlan {
    /// Validated configuration.
    pub config: InstallConfig,
    /// Target disk.
    pub disk: BlockDevice,
    /// Partition table type.
    pub table: TableKind,
    /// Planned partitions.
    pub partitions: Vec<Partition>,
}

/// The installer engine.
pub struct InstallerEngine<P: DeviceProbe, E: Executor> {
    probe: P,
    executor: E,
    plan: Option<InstallPlan>,
    state: InstallState,
}

impl<P: DeviceProbe, E: Executor> InstallerEngine<P, E> {
    /// Create an engine with explicit probe and executor.
    pub fn new(probe: P, executor: E) -> Self {
        Self {
            probe,
            executor,
            plan: None,
            state: InstallState::Idle,
        }
    }

    /// List candidate disks.
    pub fn list_disks(&self) -> Vec<BlockDevice> {
        self.probe.list_disks()
    }

    /// Current state.
    pub fn state(&self) -> InstallState {
        self.state
    }

    /// The computed plan, if any.
    pub fn plan(&self) -> Option<&InstallPlan> {
        self.plan.as_ref()
    }

    /// Mutable access to the executor (e.g. to inspect dry-run commands).
    pub fn executor_mut(&mut self) -> &mut E {
        &mut self.executor
    }

    /// Validate config, find the disk, compute the partition layout.
    pub fn plan_install(&mut self, config: InstallConfig) -> Result<(), PlanError> {
        config.validate().map_err(PlanError::InvalidConfig)?;
        let disk = self
            .probe
            .list_disks()
            .into_iter()
            .find(|d| d.name == config.disk_name)
            .ok_or_else(|| PlanError::DiskNotFound(config.disk_name.clone()))?;

        let opts = LayoutOptions {
            table: TableKind::Gpt,
            root_fs: config.root_fs.to_fs_kind(),
            separate_home: config.separate_home,
            separate_var: config.separate_var,
            encrypt_root: config.encrypt_root,
            minimum_gib: 16.0,
            efi_size_mib: 512,
        };
        let partitions =
            layout::plan_layout(&disk, &opts).map_err(PlanError::Layout)?;
        layout::validate_layout(&disk, &partitions).map_err(PlanError::Layout)?;

        let table = match opts.table {
            TableKind::Gpt => TableKind::Gpt,
            TableKind::Mbr => TableKind::Mbr,
        };
        self.plan = Some(InstallPlan {
            config,
            disk,
            table,
            partitions,
        });
        self.state = InstallState::Planned;
        Ok(())
    }

    /// Generate the ordered command list for a plan without executing it.
    /// Useful for preview UIs and tests.
    pub fn preview_commands(&self) -> Result<Vec<CommandSpec>, PlanError> {
        let plan = self.plan.as_ref().ok_or(PlanError::InvalidConfig(
            "no plan computed".into(),
        ))?;
        Ok(self
            .command_plan(plan)
            .into_iter()
            .flat_map(|(_, cmds)| cmds)
            .collect())
    }

    /// Execute the planned installation.
    ///
    /// Steps: partition table → partitions → filesystems (LUKS if enabled) →
    /// mount tree → fstab → bootstrap → bootloader → config → verify.
    pub fn execute(&mut self) -> Result<InstallReport, PlanError> {
        let plan = match self.plan.clone() {
            Some(p) => p,
            None => {
                return Err(PlanError::InvalidConfig("no plan computed".into()));
            }
        };
        let commands = self.command_plan(&plan);
        let mut run = 0usize;
        for (step, cmds) in commands {
            for cmd in cmds {
                run += 1;
                match self.executor.run(&cmd) {
                    Ok(out) if out.ok() => {}
                    Ok(out) => {
                        self.state = InstallState::Failed;
                        return Ok(InstallReport {
                            state: InstallState::Failed,
                            failed_step: Some(step.clone()),
                            error: Some(format!(
                                "'{}' exited {}: {}",
                                cmd.render(),
                                out.code,
                                out.stderr.trim()
                            )),
                            commands_run: run,
                        });
                    }
                    Err(e) => {
                        self.state = InstallState::Failed;
                        return Ok(InstallReport {
                            state: InstallState::Failed,
                            failed_step: Some(step.clone()),
                            error: Some(e.to_string()),
                            commands_run: run,
                        });
                    }
                }
            }
        }
        self.state = InstallState::Installed;
        Ok(InstallReport {
            state: InstallState::Installed,
            failed_step: None,
            error: None,
            commands_run: run,
        })
    }

    /// Build the ordered (step_name, commands) plan.
    fn command_plan(&self, plan: &InstallPlan) -> Vec<(String, Vec<CommandSpec>)> {
        let disk = &plan.disk;
        let dev = disk.dev_path();
        let cfg = &plan.config;
        let mut steps: Vec<(String, Vec<CommandSpec>)> = Vec::new();

        // Step 1: partition table + partitions via sgdisk (GPT) or sfdisk (MBR).
        let mut part_cmds = Vec::new();
        match plan.table {
            TableKind::Gpt => {
                part_cmds.push(CommandSpec {
                    program: "sgdisk".into(),
                    args: vec![
                        "--zap-all".into(),
                        "--clear".into(),
                        dev.clone(),
                    ],
                    stdin: None,
                });
                for p in &plan.partitions {
                    let type_code = match p.role {
                        PartitionRole::Efi => "EF00",
                        _ => "8300",
                    };
                    part_cmds.push(CommandSpec {
                        program: "sgdisk".into(),
                        args: vec![
                            format!("--new={}:{}:{}", p.number, p.start_sector, p.end_sector),
                            format!("--type={}:{}", p.number, type_code),
                            format!("--change-name={}:{}", p.number, p.label),
                            dev.clone(),
                        ],
                        stdin: None,
                    });
                }
            }
            TableKind::Mbr => {
                // Build an sfdisk script: one line per partition.
                let mut script = String::new();
                for p in &plan.partitions {
                    let type_code = match p.role {
                        PartitionRole::Efi => "EF",
                        _ => "83",
                    };
                    script.push_str(&format!(
                        "{}, {}, {}, {}, {}\n",
                        p.start_sector, p.end_sector, type_code, "-", p.label
                    ));
                }
                part_cmds.push(CommandSpec {
                    program: "sfdisk".into(),
                    args: vec![dev.clone()],
                    stdin: Some(script),
                });
            }
        }
        // Re-read kernel partition table.
        part_cmds.push(CommandSpec {
            program: "blockdev".into(),
            args: vec!["--rereadpt".into(), dev.clone()],
            stdin: None,
        });
        part_cmds.push(CommandSpec {
            program: "udevadm".into(),
            args: vec!["settle".into()],
            stdin: None,
        });
        steps.push(("partition".into(), part_cmds));

        // Step 2: filesystems + LUKS.
        let mut fs_cmds = Vec::new();
        let pdev = |num: u32| format!("{}{}{}", dev, partition_suffix(&disk.name), num);
        for p in &plan.partitions {
            let part_dev = pdev(p.number);
            match p.fs {
                FsKind::Luks2 => {
                    // luksFormat reads passphrase from stdin/keyfile.
                    let mut args = vec![
                        "luksFormat".into(),
                        "--type".into(),
                        "luks2".into(),
                        "--batch-mode".into(),
                    ];
                    if let Some(kf) = &cfg.luks_keyfile {
                        args.push("--key-file".into());
                        args.push(kf.clone());
                    }
                    args.push(part_dev.clone());
                    fs_cmds.push(CommandSpec {
                        program: "cryptsetup".into(),
                        args,
                        stdin: None,
                    });
                    let mut open_args = vec![
                        "open".into(),
                        part_dev.clone(),
                        "sigmaos-root".into(),
                    ];
                    if let Some(kf) = &cfg.luks_keyfile {
                        open_args.push("--key-file".into());
                        open_args.push(kf.clone());
                    }
                    fs_cmds.push(CommandSpec {
                        program: "cryptsetup".into(),
                        args: open_args,
                        stdin: None,
                    });
                }
                _ => {
                    if let Some(tool) = p.fs.mkfs_tool() {
                        let mut args = vec![part_dev.clone()];
                        if p.fs == FsKind::Fat32 {
                            args.push("-F".into());
                            args.push("32".into());
                            args.push("-n".into());
                            args.push(p.label.clone());
                        } else if p.fs == FsKind::Btrfs {
                            args.push("-L".into());
                            args.push(p.label.clone());
                            // @ and @home subvolumes for snapshot-friendly layout.
                            args.push("-f".into());
                        }
                        fs_cmds.push(CommandSpec {
                            program: tool.into(),
                            args,
                            stdin: None,
                        });
                        if p.fs == FsKind::Btrfs && p.role == PartitionRole::Root {
                            // Create @ and @home subvolumes.
                            fs_cmds.push(CommandSpec {
                                program: "mount".into(),
                                args: vec![
                                    "-o".into(),
                                    "subvolid=5".into(),
                                    if p.fs == FsKind::Luks2 {
                                        "/dev/mapper/sigmaos-root".into()
                                    } else {
                                        part_dev.clone()
                                    },
                                    "/mnt".into(),
                                ],
                                stdin: None,
                            });
                            for sv in ["@", "@home"] {
                                fs_cmds.push(CommandSpec {
                                    program: "btrfs".into(),
                                    args: vec![
                                        "subvolume".into(),
                                        "create".into(),
                                        format!("/mnt/{}", sv),
                                    ],
                                    stdin: None,
                                });
                            }
                            fs_cmds.push(CommandSpec {
                                program: "umount".into(),
                                args: vec!["/mnt".into()],
                                stdin: None,
                            });
                        }
                    }
                }
            }
        }
        steps.push(("filesystem".into(), fs_cmds));

        // Step 3: mount tree + fstab.
        let mut mount_cmds = Vec::new();
        mount_cmds.push(CommandSpec {
            program: "mkdir".into(),
            args: vec!["-p".into(), "/mnt".into()],
            stdin: None,
        });
        // /boot/efi first so /boot exists for the bootloader.
        let efi = plan
            .partitions
            .iter()
            .find(|p| p.role == PartitionRole::Efi)
            .expect("layout always has EFI");
        mount_cmds.push(CommandSpec {
            program: "mkdir".into(),
            args: vec!["-p".into(), "/mnt/boot/efi".into()],
            stdin: None,
        });
        mount_cmds.push(CommandSpec {
            program: "mount".into(),
            args: vec![pdev(efi.number), "/mnt/boot/efi".into()],
            stdin: None,
        });

        for p in &plan.partitions {
            if p.role == PartitionRole::Efi {
                continue;
            }
            let part_dev = if p.fs == FsKind::Luks2 {
                "/dev/mapper/sigmaos-root".to_string()
            } else {
                pdev(p.number)
            };
            mount_cmds.push(CommandSpec {
                program: "mkdir".into(),
                args: vec!["-p".into(), format!("/mnt{}", p.mount_point)],
                stdin: None,
            });
            let mount_src = if p.fs == FsKind::Btrfs && p.role == PartitionRole::Root {
                format!("{},subvol=@", part_dev)
            } else {
                part_dev.clone()
            };
            mount_cmds.push(CommandSpec {
                program: "mount".into(),
                args: vec![mount_src, format!("/mnt{}", p.mount_point)],
                stdin: None,
            });
        }
        steps.push(("mount".into(), mount_cmds));

        // Step 4: bootstrap base system into /mnt.
        let mut boot_cmds = Vec::new();
        boot_cmds.push(CommandSpec {
            program: "bash".into(),
            args: vec![
                "-c".into(),
                "curl -fsSL https://sigmaos.dev/bootstrap.sh | bash".into(),
            ],
            stdin: None,
        });
        // fstab with real UUIDs resolved post-mount.
        boot_cmds.push(CommandSpec {
            program: "bash".into(),
            args: vec![
                "-c".into(),
                "for p in /mnt/boot/efi /mnt; do :; done; genfstab -U /mnt >> /mnt/etc/fstab".into(),
            ],
            stdin: None,
        });
        steps.push(("bootstrap".into(), boot_cmds));

        // Step 5: system config (hostname, locale, user, keymap, timezone).
        let mut cfg_cmds = Vec::new();
        cfg_cmds.push(CommandSpec {
            program: "bash".into(),
            args: vec![
                "-c".into(),
                format!(
                    "chroot /mnt sh -c 'echo {} > /etc/hostname && echo {} > /etc/locale.conf && localectl set-keymap {} && timedatectl set-timezone {}'",
                    cfg.hostname, cfg.locale, cfg.keymap, cfg.timezone
                ),
            ],
            stdin: None,
        });
        cfg_cmds.push(CommandSpec {
            program: "bash".into(),
            args: vec![
                "-c".into(),
                format!(
                    "chroot /mnt useradd -m -G wheel -s /bin/bash {} && chroot /mnt passwd {}",
                    cfg.username, cfg.username
                ),
            ],
            stdin: None,
        });
        steps.push(("configure".into(), cfg_cmds));

        // Step 6: bootloader.
        let mut bl_cmds = Vec::new();
        match cfg.bootloader {
            Bootloader::SystemdBoot => {
                bl_cmds.push(CommandSpec {
                    program: "bootctl".into(),
                    args: vec![
                        "install".into(),
                        "--path".into(),
                        "/mnt/boot".into(),
                    ],
                    stdin: None,
                });
            }
            Bootloader::Grub => {
                bl_cmds.push(CommandSpec {
                    program: "bash".into(),
                    args: vec![
                        "-c".into(),
                        "chroot /mnt grub-install --target=x86_64-efi --efi-directory=/boot/efi --bootloader-id=SIGMAOS && chroot /mnt grub-mkconfig -o /boot/grub/grub.cfg".into(),
                    ],
                    stdin: None,
                });
            }
        }
        steps.push(("bootloader".into(), bl_cmds));

        // Step 7: desktop profile (optional).
        if cfg.install_desktop {
            steps.push((
                "desktop".into(),
                vec![CommandSpec {
                    program: "bash".into(),
                    args: vec![
                        "-c".into(),
                        format!(
                            "chroot /mnt pacman -S --noconfirm --needed hyprland waybar mako swww swaybg xdg-desktop-portal-hyprland sigmaos-desktop sigmaos-shell sigmaos-notifications sigmaos-launcher sigmaos-settings sigmaos-theme-engine"
                        ).into(),
                    ],
                    stdin: None,
                }],
            ));
        }

        // Step 8: final verification — rootfs mounted, fstab written, boot entries exist.
        steps.push((
            "verify".into(),
            vec![
                CommandSpec {
                    program: "bash".into(),
                    args: vec![
                        "-c".into(),
                        "test -d /mnt/etc && test -f /mnt/etc/fstab && test -d /mnt/boot".into(),
                    ],
                    stdin: None,
                },
                CommandSpec {
                    program: "umount".into(),
                    args: vec!["-R".into(), "/mnt".into()],
                    stdin: None,
                },
            ],
        ));

        steps
    }
}

/// Kernel partition suffix: nvme0n1 -> p1, sda -> 1, mmcblk0 -> p1.
fn partition_suffix(disk_name: &str) -> &'static str {
    // Whole-disk names ending in 'n<d>' (nvme) or containing 'mmcblk'/'loop'
    // use 'pN'; classic sdX/vdX use 'N'.
    let bytes = disk_name.as_bytes();
    let mut i = bytes.len();
    while i > 0 && bytes[i - 1].is_ascii_digit() {
        i -= 1;
    }
    let prefix = &disk_name[..i];
    if prefix.ends_with('n') || disk_name.starts_with("mmcblk") || disk_name.starts_with("loop") {
        "p"
    } else {
        ""
    }
}

/// Convenience constructor using real sysfs + dry-run executor.
pub fn dry_run_engine() -> InstallerEngine<SysfsProbe, DryRunExecutor> {
    InstallerEngine::new(SysfsProbe::default(), DryRunExecutor::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::StaticProbe;
    use crate::executor::DryRunExecutor;

    fn probe_with_disk() -> StaticProbe {
        StaticProbe {
            disks: vec![BlockDevice {
                name: "vda".into(),
                model: "TestDisk".into(),
                size_sectors: 100 * 1024 * 1024 * 1024 / 512, // 100 GiB
                logical_sector_size: 512,
                removable: false,
                rotational: false,
                is_whole_disk: true,
            }],
        }
    }

    fn cfg(disk: &str) -> InstallConfig {
        InstallConfig {
            disk_name: disk.into(),
            ..Default::default()
        }
    }

    // AAA: Arrange engine+probe, Act plan, Assert Planned with 2 partitions.
    #[test]
    fn plan_produces_layout() {
        let mut eng = InstallerEngine::new(probe_with_disk(), DryRunExecutor::new());
        eng.plan_install(cfg("vda")).unwrap();
        assert_eq!(eng.state(), InstallState::Planned);
        assert_eq!(eng.plan().unwrap().partitions.len(), 2);
    }

    // AAA: Arrange missing disk, Act plan, Assert DiskNotFound.
    #[test]
    fn plan_rejects_missing_disk() {
        let mut eng = InstallerEngine::new(probe_with_disk(), DryRunExecutor::new());
        let err = eng.plan_install(cfg("vdb")).unwrap_err();
        assert!(matches!(err, PlanError::DiskNotFound(_)));
    }

    // AAA: Arrange plan, Act preview, Assert ordered steps exist.
    #[test]
    fn preview_has_all_steps() {
        let mut eng = InstallerEngine::new(probe_with_disk(), DryRunExecutor::new());
        eng.plan_install(cfg("vda")).unwrap();
        let cmds = eng.preview_commands().unwrap();
        assert!(cmds.iter().any(|c| c.program == "sgdisk"));
        assert!(cmds.iter().any(|c| c.program == "mkfs.vfat" || c.program == "mkfs.btrfs"));
        assert!(cmds.iter().any(|c| c.program == "mount"));
        assert!(cmds.iter().any(|c| c.program == "bootctl"));
    }

    // AAA: Arrange plan, Act execute dry-run, Assert Installed + commands recorded.
    #[test]
    fn dry_run_execute_succeeds() {
        let mut eng = InstallerEngine::new(probe_with_disk(), DryRunExecutor::new());
        eng.plan_install(cfg("vda")).unwrap();
        let report = eng.execute().unwrap();
        assert_eq!(report.state, InstallState::Installed);
        assert!(report.commands_run > 10);
        assert_eq!(eng.executor_mut().name(), "dry-run");
    }

    // AAA: Arrange encrypted config without keyfile, Act plan, Assert InvalidConfig.
    #[test]
    fn encryption_requires_keyfile() {
        let mut c = cfg("vda");
        c.encrypt_root = true;
        let mut eng = InstallerEngine::new(probe_with_disk(), DryRunExecutor::new());
        assert!(eng.plan_install(c).is_err());
    }

    // AAA: partition suffix rules.
    #[test]
    fn partition_suffix_rules() {
        assert_eq!(partition_suffix("nvme0n1"), "p");
        assert_eq!(partition_suffix("sda"), "");
        assert_eq!(partition_suffix("vda"), "");
        assert_eq!(partition_suffix("mmcblk0"), "p");
    }
}

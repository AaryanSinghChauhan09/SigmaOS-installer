//! Partition layout planning.
//!
//! Pure computation: given a disk size and options, produce a sector-accurate
//! partition plan. No I/O happens here — [`crate::executor`] turns plans into
//! commands.

use crate::device::BlockDevice;

/// Filesystem choices for partitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsKind {
    /// EFI System Partition (always FAT32).
    Fat32,
    Ext4,
    Btrfs,
    Xfs,
    /// LUKS2 encrypted container (formatted by cryptsetup, not mkfs).
    Luks2,
}

impl FsKind {
    /// mkfs tool name used to create this filesystem (None for LUKS2).
    pub fn mkfs_tool(self) -> Option<&'static str> {
        match self {
            FsKind::Fat32 => Some("mkfs.vfat"),
            FsKind::Ext4 => Some("mkfs.ext4"),
            FsKind::Btrfs => Some("mkfs.btrfs"),
            FsKind::Xfs => Some("mkfs.xfs"),
            FsKind::Luks2 => None,
        }
    }

    /// fstab filesystem type.
    pub fn fstab_type(self) -> &'static str {
        match self {
            FsKind::Fat32 => "vfat",
            FsKind::Ext4 => "ext4",
            FsKind::Btrfs => "btrfs",
            FsKind::Xfs => "xfs",
            FsKind::Luks2 => "crypto_LUKS",
        }
    }
}

/// A single planned partition.
#[derive(Debug, Clone, PartialEq)]
pub struct Partition {
    /// 1-based partition number.
    pub number: u32,
    /// First sector (inclusive).
    pub start_sector: u64,
    /// Last sector (inclusive).
    pub end_sector: u64,
    /// Partition role.
    pub role: PartitionRole,
    /// Filesystem to create.
    pub fs: FsKind,
    /// Mount point inside the target system.
    pub mount_point: String,
    /// Partition name for GPT (PARTLABEL).
    pub label: String,
}

impl Partition {
    /// Size in sectors.
    pub fn sectors(&self) -> u64 {
        self.end_sector - self.start_sector + 1
    }

    /// Size in MiB.
    pub fn size_mib(&self) -> u64 {
        self.sectors() * 512 / (1024 * 1024)
    }
}

/// Role of a partition in the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionRole {
    /// EFI System Partition.
    Efi,
    /// Root filesystem `/`.
    Root,
    /// Separate `/home`.
    Home,
    /// Separate `/var`.
    Var,
}

/// Partition table type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TableKind {
    /// GUID Partition Table (default for UEFI systems).
    #[default]
    Gpt,
    /// Master Boot Record (legacy BIOS).
    Mbr,
}

/// Errors produced while planning a layout.
#[derive(Debug, Clone, PartialEq)]
pub enum LayoutError {
    /// Disk is smaller than the requested minimum.
    DiskTooSmall { size_gib: f64, minimum_gib: f64 },
    /// A computed region had non-positive size.
    InvalidGeometry,
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayoutError::DiskTooSmall {
                size_gib,
                minimum_gib,
            } => write!(
                f,
                "disk too small: {:.1} GiB < minimum {:.0} GiB",
                size_gib, minimum_gib
            ),
            LayoutError::InvalidGeometry => write!(f, "invalid partition geometry"),
        }
    }
}

impl std::error::Error for LayoutError {}

/// Options controlling the generated layout.
#[derive(Debug, Clone)]
pub struct LayoutOptions {
    /// Partition table type.
    pub table: TableKind,
    /// Root filesystem (ignored for the EFI partition, always FAT32).
    pub root_fs: FsKind,
    /// Create a separate `/home` partition using half the remaining space.
    pub separate_home: bool,
    /// Create a separate `/var` partition (1 GiB).
    pub separate_var: bool,
    /// Encrypt the root partition with LUKS2.
    pub encrypt_root: bool,
    /// Minimum disk size accepted, in GiB.
    pub minimum_gib: f64,
    /// EFI partition size in MiB.
    pub efi_size_mib: u64,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            table: TableKind::Gpt,
            root_fs: FsKind::Btrfs,
            separate_home: false,
            separate_var: false,
            encrypt_root: false,
            minimum_gib: 16.0,
            efi_size_mib: 512,
        }
    }
}

const SECTOR: u64 = 512;
const GIB: u64 = 1024 * 1024 * 1024;

fn mib_to_sectors(mib: u64) -> u64 {
    mib * 1024 * 1024 / SECTOR
}

fn gib_to_sectors(gib: u64) -> u64 {
    gib * GIB / SECTOR
}

/// Compute a partition layout for `disk`.
///
/// Layout (GPT):
///   1. EFI System Partition — `efi_size_mib` MiB, FAT32, `/boot/efi`
///   2. `/var` — 1 GiB (optional)
///   3. `/` — remaining space (or half, when `separate_home`)
///   4. `/home` — remaining space (optional)
///
/// The first usable sector is 2048 (1 MiB alignment) for both GPT and MBR.
pub fn plan_layout(
    disk: &BlockDevice,
    opts: &LayoutOptions,
) -> Result<Vec<Partition>, LayoutError> {
    let size_gib = disk.size_gib();
    if size_gib < opts.minimum_gib {
        return Err(LayoutError::DiskTooSmall {
            size_gib,
            minimum_gib: opts.minimum_gib,
        });
    }

    let total_sectors = disk.size_sectors;
    let first_usable = 2048u64;
    if total_sectors <= first_usable {
        return Err(LayoutError::InvalidGeometry);
    }

    let mut parts = Vec::new();
    let mut cursor = first_usable;
    let mut number = 1u32;

    let push = |role: PartitionRole,
                    fs: FsKind,
                    mount: &str,
                    label: &str,
                    sectors: u64,
                    parts: &mut Vec<Partition>,
                    number: &mut u32,
                    cursor: &mut u64|
     -> Result<(), LayoutError> {
        if sectors == 0 || *cursor + sectors - 1 >= total_sectors {
            return Err(LayoutError::InvalidGeometry);
        }
        parts.push(Partition {
            number: *number,
            start_sector: *cursor,
            end_sector: *cursor + sectors - 1,
            role,
            fs,
            mount_point: mount.to_string(),
            label: label.to_string(),
        });
        *cursor += sectors;
        *number += 1;
        Ok(())
    };

    // 1. EFI System Partition
    push(
        PartitionRole::Efi,
        FsKind::Fat32,
        "/boot/efi",
        "SIGMAOS-ESP",
        mib_to_sectors(opts.efi_size_mib),
        &mut parts,
        &mut number,
        &mut cursor,
    )?;

    // 2. Optional /var (1 GiB)
    if opts.separate_var {
        push(
            PartitionRole::Var,
            opts.root_fs,
            "/var",
            "SIGMAOS-VAR",
            mib_to_sectors(1024),
            &mut parts,
            &mut number,
            &mut cursor,
        )?;
    }

    let remaining = total_sectors - cursor;
    let (root_sectors, home_sectors) = if opts.separate_home {
        // Give root 60%, home 40% of the remainder.
        let root = remaining * 6 / 10;
        (root, remaining - root)
    } else {
        (remaining, 0)
    };

    // 3. Root (optionally LUKS2-encrypted container)
    let root_fs = if opts.encrypt_root {
        FsKind::Luks2
    } else {
        opts.root_fs
    };
    push(
        PartitionRole::Root,
        root_fs,
        "/",
        "SIGMAOS-ROOT",
        root_sectors,
        &mut parts,
        &mut number,
        &mut cursor,
    )?;

    // 4. Optional /home
    if opts.separate_home {
        push(
            PartitionRole::Home,
            opts.root_fs,
            "/home",
            "SIGMAOS-HOME",
            home_sectors,
            &mut parts,
            &mut number,
            &mut cursor,
        )?;
    }

    // Sanity: last partition must not overrun the disk.
    if let Some(last) = parts.last() {
        if last.end_sector >= total_sectors {
            return Err(LayoutError::InvalidGeometry);
        }
    }
    Ok(parts)
}

/// Validate that a plan covers the disk sensibly: EFI first, root present,
/// no overlapping ranges, everything within bounds.
pub fn validate_layout(disk: &BlockDevice, parts: &[Partition]) -> Result<(), LayoutError> {
    if parts.is_empty() {
        return Err(LayoutError::InvalidGeometry);
    }
    if !parts.iter().any(|p| p.role == PartitionRole::Root) {
        return Err(LayoutError::InvalidGeometry);
    }
    if !parts.iter().any(|p| p.role == PartitionRole::Efi) {
        return Err(LayoutError::InvalidGeometry);
    }
    let mut prev_end: Option<u64> = None;
    for p in parts {
        if p.start_sector < 2048 || p.end_sector >= disk.size_sectors {
            return Err(LayoutError::InvalidGeometry);
        }
        if let Some(pe) = prev_end {
            if p.start_sector <= pe {
                return Err(LayoutError::InvalidGeometry);
            }
        }
        prev_end = Some(p.end_sector);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk_gib(gib: u64) -> BlockDevice {
        BlockDevice {
            name: "vda".into(),
            model: String::new(),
            size_sectors: gib * GIB / SECTOR,
            logical_sector_size: 512,
            removable: false,
            rotational: false,
            is_whole_disk: true,
        }
    }

    // AAA: 100 GiB disk default opts -> EFI + root, validates.
    #[test]
    fn default_layout_two_partitions() {
        let d = disk_gib(100);
        let parts = plan_layout(&d, &LayoutOptions::default()).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].role, PartitionRole::Efi);
        assert_eq!(parts[0].fs, FsKind::Fat32);
        assert_eq!(parts[0].size_mib(), 512);
        assert_eq!(parts[1].role, PartitionRole::Root);
        validate_layout(&d, &parts).unwrap();
    }

    // AAA: separate home+var -> 4 partitions in role order.
    #[test]
    fn layout_with_home_and_var() {
        let d = disk_gib(200);
        let opts = LayoutOptions {
            separate_home: true,
            separate_var: true,
            ..Default::default()
        };
        let parts = plan_layout(&d, &opts).unwrap();
        assert_eq!(parts.len(), 4);
        let roles: Vec<_> = parts.iter().map(|p| p.role).collect();
        assert_eq!(
            roles,
            vec![
                PartitionRole::Efi,
                PartitionRole::Var,
                PartitionRole::Root,
                PartitionRole::Home
            ]
        );
        validate_layout(&d, &parts).unwrap();
    }

    // AAA: 8 GiB disk -> DiskTooSmall.
    #[test]
    fn rejects_small_disk() {
        let d = disk_gib(8);
        let err = plan_layout(&d, &LayoutOptions::default()).unwrap_err();
        assert!(matches!(err, LayoutError::DiskTooSmall { .. }));
    }

    // AAA: encrypted root -> root partition fs is Luks2.
    #[test]
    fn encrypted_root_uses_luks() {
        let d = disk_gib(100);
        let opts = LayoutOptions {
            encrypt_root: true,
            ..Default::default()
        };
        let parts = plan_layout(&d, &opts).unwrap();
        let root = parts.iter().find(|p| p.role == PartitionRole::Root).unwrap();
        assert_eq!(root.fs, FsKind::Luks2);
        assert_eq!(root.fs.fstab_type(), "crypto_LUKS");
    }

    // AAA: home gets ~40% of post-EFI space, root ~60%.
    #[test]
    fn home_root_split() {
        let d = disk_gib(100);
        let opts = LayoutOptions {
            separate_home: true,
            ..Default::default()
        };
        let parts = plan_layout(&d, &opts).unwrap();
        let root = parts.iter().find(|p| p.role == PartitionRole::Root).unwrap();
        let home = parts.iter().find(|p| p.role == PartitionRole::Home).unwrap();
        assert!(root.sectors() > home.sectors());
        assert!(home.sectors() > 0);
    }
}

//! Block device discovery backed by Linux sysfs.
//!
//! Mechanism only: reads `/sys/class/block` and `/sys/block/<dev>/` attributes.
//! Policy (which disk to pick, minimum size) lives in [`crate::engine`].

use std::fs;
use std::path::Path;

/// A discovered block device.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockDevice {
    /// Kernel device name, e.g. `nvme0n1`.
    pub name: String,
    /// Marketing/model string from `/sys/block/<dev>/device/model` when present.
    pub model: String,
    /// Size in 512-byte sectors (`/sys/block/<dev>/size`).
    pub size_sectors: u64,
    /// Logical sector size in bytes (`queue/logical_block_size`).
    pub logical_sector_size: u32,
    /// Removable flag (`removable` sysfs attribute).
    pub removable: bool,
    /// Rotational flag (`queue/rotational`); false = SSD/NVMe.
    pub rotational: bool,
    /// Whole-disk devices only; partition children are excluded.
    pub is_whole_disk: bool,
}

impl BlockDevice {
    /// Device node path, e.g. `/dev/nvme0n1`.
    pub fn dev_path(&self) -> String {
        format!("/dev/{}", self.name)
    }

    /// Size in GiB.
    pub fn size_gib(&self) -> f64 {
        self.size_sectors as f64 * self.logical_sector_size as f64
            / (1024.0 * 1024.0 * 1024.0)
    }
}

/// Source of block device information. Real systems use [`SysfsProbe`];
/// tests inject [`StaticProbe`].
pub trait DeviceProbe {
    /// Return all whole-disk block devices visible to the probe.
    fn list_disks(&self) -> Vec<BlockDevice>;
}

/// Reads device topology from sysfs. Returns an empty list on non-Linux or
/// when sysfs is unavailable — callers must handle "no disks" explicitly.
pub struct SysfsProbe {
    /// Root of the sysfs tree; `/sys` on real systems.
    pub sysfs_root: std::path::PathBuf,
}

impl Default for SysfsProbe {
    fn default() -> Self {
        Self {
            sysfs_root: Path::new("/sys").to_path_buf(),
        }
    }
}

fn read_u64(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_u32(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn read_trimmed(path: &Path) -> Option<String> {
    let s = fs::read_to_string(path).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn looks_like_partition(name: &str) -> bool {
    // nvme0n1p1, sda1, mmcblk0p1, loop0p1 ... but not nvme0n1 / sda / mmcblk0
    let bytes = name.as_bytes();
    let mut i = bytes.len();
    while i > 0 && bytes[i - 1].is_ascii_digit() {
        i -= 1;
    }
    if i == bytes.len() {
        return false; // no trailing digits -> whole disk name
    }
    let prefix = &name[..i];
    // Whole-disk names end in 'n<part>' style handled below; simpler rule:
    // a partition name always has a digit-suffix AND its prefix is itself a
    // plausible disk name. We approximate: names containing 'p' before the
    // digit run (nvme0n1p1, mmcblk0p1) or classic sda1/vda1 patterns.
    if prefix == "mmcblk" {
        return false;
    }
    prefix.ends_with('p')
        || (prefix.chars().last().map(|c| c.is_ascii_alphabetic()).unwrap_or(false)
            && !prefix.ends_with('n')
            && prefix != "loop")
}

impl DeviceProbe for SysfsProbe {
    fn list_disks(&self) -> Vec<BlockDevice> {
        let class_block = self.sysfs_root.join("class/block");
        let entries = match fs::read_dir(&class_block) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };
        let mut disks = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("loop") || looks_like_partition(&name) {
                continue;
            }
            let dev = self.sysfs_root.join("block").join(&name);
            let Some(size) = read_u64(&dev.join("size")) else {
                continue;
            };
            if size == 0 {
                continue;
            }
            let model = read_trimmed(&dev.join("device/model"))
                .or_else(|| read_trimmed(&dev.join("device/vendor")))
                .unwrap_or_default();
            disks.push(BlockDevice {
                name: name.clone(),
                model,
                size_sectors: size,
                logical_sector_size: read_u32(&dev.join("queue/logical_block_size"))
                    .unwrap_or(512),
                removable: read_u32(&dev.join("removable")).unwrap_or(0) == 1,
                rotational: read_u32(&dev.join("queue/rotational")).unwrap_or(1) == 1,
                is_whole_disk: true,
            });
        }
        disks.sort_by(|a, b| a.name.cmp(&b.name));
        disks
    }
}

/// Deterministic probe for tests and dry-run environments.
#[derive(Debug, Default)]
pub struct StaticProbe {
    /// Devices returned by [`StaticProbe::list_disks`].
    pub disks: Vec<BlockDevice>,
}

impl DeviceProbe for StaticProbe {
    fn list_disks(&self) -> Vec<BlockDevice> {
        let mut d = self.disks.clone();
        d.sort_by(|a, b| a.name.cmp(&b.name));
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk(name: &str, sectors: u64) -> BlockDevice {
        BlockDevice {
            name: name.into(),
            model: String::new(),
            size_sectors: sectors,
            logical_sector_size: 512,
            removable: false,
            rotational: false,
            is_whole_disk: true,
        }
    }

    // AAA: Arrange static probe with two disks, Act list, Assert both returned sorted.
    #[test]
    fn static_probe_lists_disks() {
        let probe = StaticProbe {
            disks: vec![disk("sdb", 1000), disk("sda", 2000)],
        };
        let out = probe.list_disks();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, "sda");
        assert_eq!(out[1].name, "sdb");
    }

    // AAA: Arrange 1 TiB disk, Act size_gib, Assert ~1024.
    #[test]
    fn size_gib_calculation() {
        let d = disk("nvme0n1", 2_147_483_648);
        assert!((d.size_gib() - 1024.0).abs() < 0.01);
    }

    // AAA: Arrange sysfs fixture tree, Act SysfsProbe, Assert parsed attributes.
    #[test]
    fn sysfs_probe_reads_attributes() {
        let tmp = std::env::temp_dir().join(format!("sigmaos-probe-{}", std::process::id()));
        let blk = tmp.join("block/vda");
        fs::create_dir_all(blk.join("queue")).unwrap();
        fs::create_dir_all(blk.join("device")).unwrap();
        fs::create_dir_all(tmp.join("class/block")).unwrap();
        // class/block entries are symlinks in real sysfs; emulate with dirs
        fs::create_dir_all(tmp.join("class/block/vda")).unwrap();
        fs::write(blk.join("size"), "209715200\n").unwrap();
        fs::write(blk.join("queue/logical_block_size"), "512\n").unwrap();
        fs::write(blk.join("queue/rotational"), "0\n").unwrap();
        fs::write(blk.join("removable"), "0\n").unwrap();
        fs::write(blk.join("device/model"), "QEMU HARDDISK\n").unwrap();

        let probe = SysfsProbe {
            sysfs_root: tmp.clone(),
        };
        let disks = probe.list_disks();
        assert_eq!(disks.len(), 1);
        let d = &disks[0];
        assert_eq!(d.name, "vda");
        assert_eq!(d.size_sectors, 209715200);
        assert_eq!(d.logical_sector_size, 512);
        assert!(!d.rotational);
        assert_eq!(d.model, "QEMU HARDDISK");
        fs::remove_dir_all(&tmp).ok();
    }

    // AAA: partition-looking names are excluded from whole-disk listing.
    #[test]
    fn partition_names_excluded() {
        assert!(looks_like_partition("nvme0n1p1"));
        assert!(looks_like_partition("sda1"));
        assert!(looks_like_partition("mmcblk0p2"));
        assert!(!looks_like_partition("nvme0n1"));
        assert!(!looks_like_partition("sda"));
        assert!(!looks_like_partition("mmcblk0"));
    }
}

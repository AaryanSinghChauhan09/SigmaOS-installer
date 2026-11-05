#[derive(Debug, Clone)]
pub struct InstallationTarget {
    pub target_disk: String,
    pub encrypt_luks2: bool,
    pub efi_partition_mb: u64,
}

#[derive(Debug, Default)]
pub struct InstallerEngine {
    target: Option<InstallationTarget>,
}

impl InstallerEngine {
    pub fn new() -> Self { Self::default() }
    pub fn configure(&mut self, disk: &str, encrypt: bool, efi_mb: u64) {
        self.target = Some(InstallationTarget {
            target_disk: disk.to_string(),
            encrypt_luks2: encrypt,
            efi_partition_mb: efi_mb,
        });
    }
    pub fn is_ready(&self) -> bool {
        self.target.as_ref().map(|t| !t.target_disk.is_empty() && t.efi_partition_mb >= 512).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_installer_readiness() {
        let mut inst = InstallerEngine::new();
        inst.configure("/dev/nvme0n1", true, 512);
        assert!(inst.is_ready());
    }
}

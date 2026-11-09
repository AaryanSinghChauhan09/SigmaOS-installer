//! Installer configuration: user-supplied policy for an installation.

/// Filesystem options the user can pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootFsChoice {
    /// Btrfs with @ and @home subvolumes (default; snapshot-friendly).
    Btrfs,
    /// Traditional ext4.
    Ext4,
    /// XFS.
    Xfs,
}

impl RootFsChoice {
    /// Map to the layout-layer filesystem kind.
    pub fn to_fs_kind(self) -> crate::layout::FsKind {
        match self {
            RootFsChoice::Btrfs => crate::layout::FsKind::Btrfs,
            RootFsChoice::Ext4 => crate::layout::FsKind::Ext4,
            RootFsChoice::Xfs => crate::layout::FsKind::Xfs,
        }
    }
}

/// Bootloader choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Bootloader {
    /// systemd-boot (default; simple, fast, UKI-friendly).
    #[default]
    SystemdBoot,
    /// GRUB2.
    Grub,
}

/// Installation configuration collected from the wizard or autoinstall file.
#[derive(Debug, Clone)]
pub struct InstallConfig {
    /// Target disk kernel name (must match a probed [`crate::device::BlockDevice`]).
    pub disk_name: String,
    /// Root filesystem choice.
    pub root_fs: RootFsChoice,
    /// LUKS2-encrypt the root partition.
    pub encrypt_root: bool,
    /// Passphrase file for LUKS (read by cryptsetup via stdin); never logged.
    pub luks_keyfile: Option<String>,
    /// Separate `/home` partition.
    pub separate_home: bool,
    /// Separate `/var` partition.
    pub separate_var: bool,
    /// Bootloader to install.
    pub bootloader: Bootloader,
    /// Hostname for the installed system.
    pub hostname: String,
    /// Username of the primary account.
    pub username: String,
    /// Keyboard layout (console-setup/keymap name).
    pub keymap: String,
    /// Timezone (e.g. `Europe/Berlin`).
    pub timezone: String,
    /// Locale (e.g. `en_US.UTF-8`).
    pub locale: String,
    /// Full name of the primary user (GECOS).
    pub full_name: String,
    /// Whether to install the SigmaOS desktop profile (Hyprland session etc.).
    pub install_desktop: bool,
}

impl Default for InstallConfig {
    fn default() -> Self {
        Self {
            disk_name: String::new(),
            root_fs: RootFsChoice::Btrfs,
            encrypt_root: false,
            luks_keyfile: None,
            separate_home: false,
            separate_var: false,
            bootloader: Bootloader::SystemdBoot,
            hostname: "sigmaos".into(),
            username: "user".into(),
            keymap: "us".into(),
            timezone: "UTC".into(),
            locale: "en_US.UTF-8".into(),
            full_name: String::new(),
            install_desktop: true,
        }
    }
}

impl InstallConfig {
    /// Basic validation before planning.
    pub fn validate(&self) -> Result<(), String> {
        if self.disk_name.is_empty() {
            return Err("no target disk selected".into());
        }
        if self.hostname.is_empty()
            || self.hostname.len() > 63
            || !self
                .hostname
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        {
            return Err("invalid hostname".into());
        }
        if self.username.is_empty() || self.username == "root" {
            return Err("invalid username".into());
        }
        if self.encrypt_root && self.luks_keyfile.is_none() {
            return Err("LUKS encryption requested but no keyfile/passphrase configured".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // AAA: default config is valid except empty disk.
    #[test]
    fn default_config_needs_disk() {
        let cfg = InstallConfig::default();
        assert!(cfg.validate().is_err());
        let mut cfg = InstallConfig {
            disk_name: "vda".into(),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
        cfg.hostname = "bad host!".into();
        assert!(cfg.validate().is_err());
        cfg.hostname = "sigmaos-pc".into();
        cfg.encrypt_root = true;
        assert!(cfg.validate().is_err()); // no keyfile
        cfg.luks_keyfile = Some("/tmp/key".into());
        assert!(cfg.validate().is_ok());
    }
}

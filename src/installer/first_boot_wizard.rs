//! First-boot experience wizard for SigmaOS.
//!
//! This module drives the first-boot OOBE (out-of-box experience) that runs
//! the first time a freshly-installed SigmaOS system is started.  It collects
//! user profile information (username, locale, keyboard layout, timezone,
//! network preferences, privacy settings and extra-app selection) and applies
//! them to the live system.
//!
//! # Design
//! - Completely stateful; the caller creates a [`FirstBootWizard`] and drives
//!   it forward one step at a time by supplying a [`FirstBootUpdate`].
//! - All I/O is performed through thin helpers that can be stubbed in tests
//!   via conditional compilation.
//! - No `unsafe` code; no heap-global state.

#![warn(missing_docs)]

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs as unix_fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

// ─── error type ─────────────────────────────────────────────────────────────

/// Errors that may occur during the first-boot wizard.
#[derive(Debug, Error)]
pub enum FirstBootError {
    /// A required configuration field is missing or empty.
    #[error("missing required field: {field}")]
    MissingField {
        /// The name of the missing field.
        field: &'static str,
    },

    /// The wizard was advanced past the `Complete` step.
    #[error("wizard is already complete; no further steps available")]
    AlreadyComplete,

    /// The supplied username contains invalid characters.
    #[error("invalid username '{0}': must be 1–32 lowercase alphanumeric characters or '-'/'_'")]
    InvalidUsername(String),

    /// The supplied locale string is not in the expected `lang_TERRITORY` format.
    #[error("invalid locale '{0}': expected format 'lang_TERRITORY' (e.g. 'en_US')")]
    InvalidLocale(String),

    /// The supplied timezone is not a valid IANA zone identifier.
    #[error("invalid timezone '{0}': must be a valid IANA timezone (e.g. 'America/New_York')")]
    InvalidTimezone(String),

    /// A filesystem operation failed.
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),

    /// The wizard attempted to write a system file but was called without
    /// root privileges.
    #[error("insufficient privileges: operation requires root")]
    NotRoot,

    /// The first-boot marker file could not be created; the OS may not
    /// support the target path.
    #[error("could not create first-boot completion marker at '{path}': {source}")]
    MarkerCreateFailed {
        /// Path that was attempted.
        path: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },
}

// ─── enums ──────────────────────────────────────────────────────────────────

/// The ordered steps of the first-boot wizard.
///
/// Steps must be advanced in the sequence they are declared here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FirstBootStep {
    /// Welcome / splash screen shown on very first boot.
    Welcome,
    /// Collect username, full name and password hash.
    UserProfile,
    /// Choose locale and timezone.
    RegionLocale,
    /// Choose keyboard layout and variant.
    KeyboardLayout,
    /// Configure network interfaces.
    NetworkSetup,
    /// Accept or decline optional telemetry / crash reporting.
    PrivacySettings,
    /// Select from optional additional applications to install.
    DefaultApps,
    /// Show a summary of all collected settings before applying.
    Review,
    /// Wizard is complete; settings have been applied.
    Complete,
}

impl FirstBootStep {
    /// Return the next step in the sequence, or `None` if already `Complete`.
    #[must_use]
    pub fn next(self) -> Option<Self> {
        match self {
            Self::Welcome => Some(Self::UserProfile),
            Self::UserProfile => Some(Self::RegionLocale),
            Self::RegionLocale => Some(Self::KeyboardLayout),
            Self::KeyboardLayout => Some(Self::NetworkSetup),
            Self::NetworkSetup => Some(Self::PrivacySettings),
            Self::PrivacySettings => Some(Self::DefaultApps),
            Self::DefaultApps => Some(Self::Review),
            Self::Review => Some(Self::Complete),
            Self::Complete => None,
        }
    }
}

// ─── data structs ───────────────────────────────────────────────────────────

/// Network configuration captured during first boot.
#[derive(Debug, Clone, Default)]
pub struct NetworkConfig {
    /// Use DHCP for automatic addressing (default: `true`).
    pub use_dhcp: bool,
    /// Static IP address (used only when `use_dhcp` is `false`).
    pub static_ip: Option<String>,
    /// Default gateway (used only when `use_dhcp` is `false`).
    pub gateway: Option<String>,
    /// DNS resolver addresses.
    pub dns_servers: Vec<String>,
    /// Optional Wi-Fi SSID.
    pub wifi_ssid: Option<String>,
}

/// XDG base directories created in the new user's home.
#[derive(Debug, Clone)]
pub struct XdgDirectories {
    /// `~/Desktop`
    pub desktop: PathBuf,
    /// `~/Documents`
    pub documents: PathBuf,
    /// `~/Downloads`
    pub downloads: PathBuf,
    /// `~/Music`
    pub music: PathBuf,
    /// `~/Pictures`
    pub pictures: PathBuf,
    /// `~/Videos`
    pub videos: PathBuf,
    /// `~/Templates`
    pub templates: PathBuf,
}

/// A resolved user profile ready to be written to the system.
#[derive(Debug, Clone)]
pub struct UserProfile {
    /// Absolute path to the user's home directory (e.g. `/home/alice`).
    pub home_dir: PathBuf,
    /// Login shell (e.g. `/bin/zsh`).
    pub shell: String,
    /// Groups the user should be a member of.
    pub groups: Vec<String>,
    /// XDG directory layout.
    pub xdg_dirs: XdgDirectories,
}

/// Full configuration state accumulated by the wizard.
#[derive(Debug, Clone, Default)]
pub struct FirstBootConfig {
    /// POSIX-compatible login name (max 32 chars).
    pub username: String,
    /// Human-readable display name.
    pub full_name: String,
    /// Pre-hashed password (SHA-512 crypt string or similar).
    pub password_hash: String,
    /// Locale string e.g. `"en_US.UTF-8"`.
    pub locale: String,
    /// IANA timezone identifier e.g. `"Asia/Kolkata"`.
    pub timezone: String,
    /// XKB keyboard layout e.g. `"us"`.
    pub keyboard_layout: String,
    /// Optional XKB variant e.g. `"dvorak"`.
    pub keyboard_variant: Option<String>,
    /// Network preferences.
    pub network_config: NetworkConfig,
    /// Whether the user opted into anonymous crash/usage reporting.
    pub privacy_telemetry: bool,
    /// Names of additional app bundles to install.
    pub install_extra_apps: Vec<String>,
}

/// An incremental update supplied by the caller when advancing a step.
///
/// Only the fields relevant to the *current* step need to be populated.
/// Fields left as `None` are ignored.
#[derive(Debug, Clone, Default)]
pub struct FirstBootUpdate {
    /// Updated username.
    pub username: Option<String>,
    /// Updated display name.
    pub full_name: Option<String>,
    /// Updated pre-hashed password.
    pub password_hash: Option<String>,
    /// Updated locale.
    pub locale: Option<String>,
    /// Updated timezone.
    pub timezone: Option<String>,
    /// Updated keyboard layout.
    pub keyboard_layout: Option<String>,
    /// Updated keyboard variant.
    pub keyboard_variant: Option<String>,
    /// Updated network configuration.
    pub network_config: Option<NetworkConfig>,
    /// Updated telemetry preference.
    pub privacy_telemetry: Option<bool>,
    /// Additional applications to install.
    pub install_extra_apps: Option<Vec<String>>,
}

// ─── wizard ─────────────────────────────────────────────────────────────────

/// Stateful driver for the first-boot OOBE wizard.
///
/// # Example
/// ```rust,no_run
/// # use sigmaos_component::*;
/// let mut wizard = FirstBootWizard::new();
/// assert_eq!(wizard.current_step(), FirstBootStep::Welcome);
/// ```
#[derive(Debug)]
pub struct FirstBootWizard {
    step: FirstBootStep,
    config: FirstBootConfig,
    /// Root of the target system (overridable for testing, default `/`).
    root: PathBuf,
}

impl FirstBootWizard {
    /// Create a new wizard at the `Welcome` step.
    #[must_use]
    pub fn new() -> Self {
        Self {
            step: FirstBootStep::Welcome,
            config: FirstBootConfig::default(),
            root: PathBuf::from("/"),
        }
    }

    /// Create a wizard with a custom system root (useful for integration tests).
    #[must_use]
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self {
            step: FirstBootStep::Welcome,
            config: FirstBootConfig::default(),
            root: root.into(),
        }
    }

    /// Return the current step without consuming the wizard.
    #[must_use]
    pub fn current_step(&self) -> FirstBootStep {
        self.step
    }

    /// Immutable view of the accumulated configuration.
    #[must_use]
    pub fn config(&self) -> &FirstBootConfig {
        &self.config
    }

    /// Apply a partial update to the configuration and advance to the next step.
    ///
    /// Returns the *new* step on success, or a [`FirstBootError`] if validation
    /// fails or the wizard has already reached `Complete`.
    ///
    /// # Errors
    /// - [`FirstBootError::AlreadyComplete`] if called after reaching `Complete`.
    /// - [`FirstBootError::MissingField`] / [`FirstBootError::InvalidUsername`] etc.
    ///   if required fields are absent or malformed for the current step.
    pub fn advance(&mut self, update: &FirstBootUpdate) -> Result<FirstBootStep, FirstBootError> {
        if self.step == FirstBootStep::Complete {
            return Err(FirstBootError::AlreadyComplete);
        }

        // Merge update into config.
        self.apply_update(update);

        // Validate the fields required by the current step.
        self.validate_step(self.step)?;

        // Advance.
        self.step = self.step.next().expect("non-complete step always has next");
        Ok(self.step)
    }

    /// Generate a [`UserProfile`] from the accumulated configuration.
    ///
    /// Validation: username must be non-empty and well-formed.
    ///
    /// # Errors
    /// Returns [`FirstBootError::MissingField`] if the username is absent, or
    /// [`FirstBootError::InvalidUsername`] if it contains illegal characters.
    pub fn generate_user_profile(&self) -> Result<UserProfile, FirstBootError> {
        let username = self.require_username()?;
        let home = self.root.join("home").join(&username);

        let xdg = XdgDirectories {
            desktop: home.join("Desktop"),
            documents: home.join("Documents"),
            downloads: home.join("Downloads"),
            music: home.join("Music"),
            pictures: home.join("Pictures"),
            videos: home.join("Videos"),
            templates: home.join("Templates"),
        };

        Ok(UserProfile {
            home_dir: home,
            shell: "/bin/zsh".to_owned(),
            groups: default_user_groups(),
            xdg_dirs: xdg,
        })
    }

    /// Write `/etc/locale.conf` (or `<root>/etc/locale.conf`) with the
    /// accumulated locale setting.
    ///
    /// # Errors
    /// Returns [`FirstBootError::MissingField`] if the locale has not been set,
    /// [`FirstBootError::InvalidLocale`] if the locale string is malformed, or
    /// [`FirstBootError::Io`] on filesystem failure.
    pub fn apply_locale(&self) -> Result<(), FirstBootError> {
        let locale = self.require_locale()?;
        let path = self.root.join("etc").join("locale.conf");
        let contents = format!("LANG={locale}\nLC_ALL={locale}\n");
        fs::create_dir_all(path.parent().unwrap_or(Path::new("/etc")))?;
        fs::write(&path, contents)?;
        Ok(())
    }

    /// Write `/etc/vconsole.conf` (or `<root>/etc/vconsole.conf`) with the
    /// accumulated keyboard layout setting.
    ///
    /// # Errors
    /// Returns [`FirstBootError::MissingField`] if the keyboard layout has not
    /// been set, or [`FirstBootError::Io`] on filesystem failure.
    pub fn apply_keyboard(&self) -> Result<(), FirstBootError> {
        if self.config.keyboard_layout.is_empty() {
            return Err(FirstBootError::MissingField {
                field: "keyboard_layout",
            });
        }
        let layout = &self.config.keyboard_layout;
        let path = self.root.join("etc").join("vconsole.conf");
        let variant_line = match &self.config.keyboard_variant {
            Some(v) if !v.is_empty() => format!("KEYMAP_VARIANT={v}\n"),
            _ => String::new(),
        };
        let contents = format!("KEYMAP={layout}\n{variant_line}");
        fs::create_dir_all(path.parent().unwrap_or(Path::new("/etc")))?;
        fs::write(&path, contents)?;
        Ok(())
    }

    /// Symlink `/etc/localtime` (or `<root>/etc/localtime`) to the correct
    /// zoneinfo file.
    ///
    /// # Errors
    /// Returns [`FirstBootError::MissingField`] if the timezone has not been
    /// set, [`FirstBootError::InvalidTimezone`] if the timezone identifier is
    /// malformed, or [`FirstBootError::Io`] if the zoneinfo file is absent or
    /// the symlink cannot be created.
    pub fn apply_timezone(&self) -> Result<(), FirstBootError> {
        let tz = self.require_timezone()?;
        let zoneinfo = self
            .root
            .join("usr")
            .join("share")
            .join("zoneinfo")
            .join(&tz);
        let target = self.root.join("etc").join("localtime");

        // Remove any existing symlink / file at the target path.
        if target.exists() || target.symlink_metadata().is_ok() {
            fs::remove_file(&target)?;
        }
        fs::create_dir_all(target.parent().unwrap_or(Path::new("/etc")))?;
        unix_fs::symlink(&zoneinfo, &target)?;
        Ok(())
    }

    /// Finalise the first-boot wizard by writing the completion marker file
    /// (`/etc/sigma_firstboot_complete`) so that the wizard is not shown again
    /// on the next boot.
    ///
    /// # Errors
    /// Returns [`FirstBootError::MarkerCreateFailed`] if the marker cannot be
    /// written, or [`FirstBootError::MissingField`] if mandatory fields have
    /// never been provided.
    pub fn finalize(&self) -> Result<(), FirstBootError> {
        // Guard: username must exist before finalising.
        self.require_username()?;

        let marker = self.root.join("etc").join("sigma_firstboot_complete");
        fs::create_dir_all(marker.parent().unwrap_or(Path::new("/etc")))?;
        fs::write(&marker, "1\n").map_err(|e| FirstBootError::MarkerCreateFailed {
            path: marker.display().to_string(),
            source: e,
        })?;
        Ok(())
    }

    // ── private helpers ────────────────────────────────────────────────────

    fn apply_update(&mut self, u: &FirstBootUpdate) {
        if let Some(v) = &u.username {
            self.config.username.clone_from(v);
        }
        if let Some(v) = &u.full_name {
            self.config.full_name.clone_from(v);
        }
        if let Some(v) = &u.password_hash {
            self.config.password_hash.clone_from(v);
        }
        if let Some(v) = &u.locale {
            self.config.locale.clone_from(v);
        }
        if let Some(v) = &u.timezone {
            self.config.timezone.clone_from(v);
        }
        if let Some(v) = &u.keyboard_layout {
            self.config.keyboard_layout.clone_from(v);
        }
        if let Some(v) = &u.keyboard_variant {
            self.config.keyboard_variant = Some(v.clone());
        }
        if let Some(v) = u.network_config.clone() {
            self.config.network_config = v;
        }
        if let Some(v) = u.privacy_telemetry {
            self.config.privacy_telemetry = v;
        }
        if let Some(v) = u.install_extra_apps.clone() {
            self.config.install_extra_apps = v;
        }
    }

    fn validate_step(&self, step: FirstBootStep) -> Result<(), FirstBootError> {
        match step {
            FirstBootStep::Welcome => Ok(()),
            FirstBootStep::UserProfile => {
                self.require_username()?;
                if self.config.password_hash.is_empty() {
                    return Err(FirstBootError::MissingField {
                        field: "password_hash",
                    });
                }
                Ok(())
            }
            FirstBootStep::RegionLocale => {
                self.require_locale()?;
                self.require_timezone()?;
                Ok(())
            }
            FirstBootStep::KeyboardLayout => {
                if self.config.keyboard_layout.is_empty() {
                    return Err(FirstBootError::MissingField {
                        field: "keyboard_layout",
                    });
                }
                Ok(())
            }
            // Remaining steps have no mandatory fields.
            FirstBootStep::NetworkSetup
            | FirstBootStep::PrivacySettings
            | FirstBootStep::DefaultApps
            | FirstBootStep::Review
            | FirstBootStep::Complete => Ok(()),
        }
    }

    fn require_username(&self) -> Result<String, FirstBootError> {
        if self.config.username.is_empty() {
            return Err(FirstBootError::MissingField { field: "username" });
        }
        validate_username(&self.config.username)?;
        Ok(self.config.username.clone())
    }

    fn require_locale(&self) -> Result<String, FirstBootError> {
        if self.config.locale.is_empty() {
            return Err(FirstBootError::MissingField { field: "locale" });
        }
        validate_locale(&self.config.locale)?;
        Ok(self.config.locale.clone())
    }

    fn require_timezone(&self) -> Result<String, FirstBootError> {
        if self.config.timezone.is_empty() {
            return Err(FirstBootError::MissingField { field: "timezone" });
        }
        validate_timezone(&self.config.timezone)?;
        Ok(self.config.timezone.clone())
    }
}

impl Default for FirstBootWizard {
    fn default() -> Self {
        Self::new()
    }
}

// ─── validation helpers ──────────────────────────────────────────────────────

/// Validate a POSIX username: 1–32 chars, lowercase letters, digits, `-`, `_`.
fn validate_username(name: &str) -> Result<(), FirstBootError> {
    if name.is_empty() || name.len() > 32 {
        return Err(FirstBootError::InvalidUsername(name.to_owned()));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(FirstBootError::InvalidUsername(name.to_owned()));
    }
    if name.starts_with('-') || name.starts_with('_') {
        return Err(FirstBootError::InvalidUsername(name.to_owned()));
    }
    Ok(())
}

/// Loose locale validation: must contain an underscore (e.g. `en_US`).
fn validate_locale(locale: &str) -> Result<(), FirstBootError> {
    if !locale.contains('_') || locale.trim().is_empty() {
        return Err(FirstBootError::InvalidLocale(locale.to_owned()));
    }
    Ok(())
}

/// Loose timezone validation: must contain a `/` (e.g. `America/New_York`).
/// The `UTC` and `GMT` special cases are also accepted.
fn validate_timezone(tz: &str) -> Result<(), FirstBootError> {
    let canonical = tz.trim();
    if canonical == "UTC" || canonical == "GMT" {
        return Ok(());
    }
    if !canonical.contains('/') || canonical.trim().is_empty() {
        return Err(FirstBootError::InvalidTimezone(tz.to_owned()));
    }
    Ok(())
}

/// Returns the standard set of supplementary groups for a new desktop user.
fn default_user_groups() -> Vec<String> {
    vec![
        "wheel".to_owned(),
        "audio".to_owned(),
        "video".to_owned(),
        "optical".to_owned(),
        "storage".to_owned(),
        "network".to_owned(),
        "power".to_owned(),
        "lp".to_owned(),
        "scanner".to_owned(),
    ]
}

// ─── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn tmp_root() -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "sigma_fbw_test_{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create tmp root");
        dir
    }

    // ── Arrange / Act / Assert helpers ──

    fn make_wizard_with_root(root: &Path) -> FirstBootWizard {
        FirstBootWizard::with_root(root)
    }

    fn full_update() -> FirstBootUpdate {
        FirstBootUpdate {
            username: Some("alice".to_owned()),
            full_name: Some("Alice Example".to_owned()),
            password_hash: Some("$6$rounds=65536$xxx".to_owned()),
            locale: Some("en_US.UTF-8".to_owned()),
            timezone: Some("America/New_York".to_owned()),
            keyboard_layout: Some("us".to_owned()),
            keyboard_variant: None,
            network_config: Some(NetworkConfig {
                use_dhcp: true,
                ..Default::default()
            }),
            privacy_telemetry: Some(false),
            install_extra_apps: Some(vec!["sigma-extra".to_owned()]),
        }
    }

    // ── Step: Welcome ──────────────────────────────────────────────────────

    #[test]
    fn welcome_step_is_initial_state() {
        // Arrange
        let wizard = FirstBootWizard::new();
        // Act (read-only)
        let step = wizard.current_step();
        // Assert
        assert_eq!(step, FirstBootStep::Welcome);
    }

    #[test]
    fn welcome_advances_without_data() {
        // Arrange
        let mut wizard = FirstBootWizard::new();
        // Act
        let next = wizard.advance(&FirstBootUpdate::default()).unwrap();
        // Assert
        assert_eq!(next, FirstBootStep::UserProfile);
    }

    // ── Step: UserProfile ─────────────────────────────────────────────────

    #[test]
    fn user_profile_advances_with_valid_fields() {
        // Arrange
        let mut wizard = FirstBootWizard::new();
        wizard.advance(&FirstBootUpdate::default()).unwrap(); // -> UserProfile

        let update = FirstBootUpdate {
            username: Some("bob".to_owned()),
            full_name: Some("Bob Builder".to_owned()),
            password_hash: Some("$6$abc".to_owned()),
            ..Default::default()
        };
        // Act
        let next = wizard.advance(&update).unwrap();
        // Assert
        assert_eq!(next, FirstBootStep::RegionLocale);
    }

    #[test]
    fn user_profile_rejects_missing_username() {
        // Arrange
        let mut wizard = FirstBootWizard::new();
        wizard.advance(&FirstBootUpdate::default()).unwrap(); // -> UserProfile

        let update = FirstBootUpdate {
            password_hash: Some("$6$abc".to_owned()),
            ..Default::default()
        };
        // Act
        let result = wizard.advance(&update);
        // Assert
        assert!(matches!(
            result,
            Err(FirstBootError::MissingField { field: "username" })
        ));
    }

    #[test]
    fn user_profile_rejects_invalid_username_with_uppercase() {
        // Arrange
        let mut wizard = FirstBootWizard::new();
        wizard.advance(&FirstBootUpdate::default()).unwrap(); // -> UserProfile

        let update = FirstBootUpdate {
            username: Some("Alice".to_owned()), // uppercase – invalid
            password_hash: Some("$6$abc".to_owned()),
            ..Default::default()
        };
        // Act
        let result = wizard.advance(&update);
        // Assert
        assert!(matches!(
            result,
            Err(FirstBootError::InvalidUsername(_))
        ));
    }

    #[test]
    fn user_profile_rejects_missing_password_hash() {
        // Arrange
        let mut wizard = FirstBootWizard::new();
        wizard.advance(&FirstBootUpdate::default()).unwrap(); // -> UserProfile

        let update = FirstBootUpdate {
            username: Some("carol".to_owned()),
            ..Default::default()
        };
        // Act
        let result = wizard.advance(&update);
        // Assert
        assert!(matches!(
            result,
            Err(FirstBootError::MissingField {
                field: "password_hash"
            })
        ));
    }

    // ── Step: RegionLocale ────────────────────────────────────────────────

    #[test]
    fn region_locale_advances_with_valid_locale_and_timezone() {
        // Arrange
        let mut wizard = FirstBootWizard::new();
        wizard.advance(&FirstBootUpdate::default()).unwrap(); // -> UserProfile
        wizard
            .advance(&FirstBootUpdate {
                username: Some("dave".to_owned()),
                password_hash: Some("$6$x".to_owned()),
                ..Default::default()
            })
            .unwrap(); // -> RegionLocale

        let update = FirstBootUpdate {
            locale: Some("fr_FR.UTF-8".to_owned()),
            timezone: Some("Europe/Paris".to_owned()),
            ..Default::default()
        };
        // Act
        let next = wizard.advance(&update).unwrap();
        // Assert
        assert_eq!(next, FirstBootStep::KeyboardLayout);
    }

    #[test]
    fn region_locale_rejects_locale_without_underscore() {
        // Arrange
        let mut wizard = FirstBootWizard::new();
        wizard.advance(&FirstBootUpdate::default()).unwrap();
        wizard
            .advance(&FirstBootUpdate {
                username: Some("eve".to_owned()),
                password_hash: Some("$6$x".to_owned()),
                ..Default::default()
            })
            .unwrap();

        let update = FirstBootUpdate {
            locale: Some("badlocale".to_owned()),
            timezone: Some("UTC".to_owned()),
            ..Default::default()
        };
        // Act
        let result = wizard.advance(&update);
        // Assert
        assert!(matches!(result, Err(FirstBootError::InvalidLocale(_))));
    }

    #[test]
    fn region_locale_rejects_timezone_without_slash_unless_utc() {
        // Arrange
        let mut wizard = FirstBootWizard::new();
        wizard.advance(&FirstBootUpdate::default()).unwrap();
        wizard
            .advance(&FirstBootUpdate {
                username: Some("frank".to_owned()),
                password_hash: Some("$6$x".to_owned()),
                ..Default::default()
            })
            .unwrap();

        let update = FirstBootUpdate {
            locale: Some("en_GB.UTF-8".to_owned()),
            timezone: Some("NotAZone".to_owned()),
            ..Default::default()
        };
        // Act
        let result = wizard.advance(&update);
        // Assert
        assert!(matches!(result, Err(FirstBootError::InvalidTimezone(_))));
    }

    #[test]
    fn region_locale_accepts_utc_timezone_special_case() {
        // Arrange
        let mut wizard = FirstBootWizard::new();
        wizard.advance(&FirstBootUpdate::default()).unwrap();
        wizard
            .advance(&FirstBootUpdate {
                username: Some("grace".to_owned()),
                password_hash: Some("$6$x".to_owned()),
                ..Default::default()
            })
            .unwrap();

        let update = FirstBootUpdate {
            locale: Some("en_US.UTF-8".to_owned()),
            timezone: Some("UTC".to_owned()),
            ..Default::default()
        };
        // Act
        let result = wizard.advance(&update);
        // Assert
        assert!(result.is_ok());
    }

    // ── Step: KeyboardLayout ──────────────────────────────────────────────

    #[test]
    fn keyboard_layout_rejects_empty_layout() {
        // Arrange
        let mut wizard = FirstBootWizard::new();
        // Drive to KeyboardLayout step.
        wizard.advance(&FirstBootUpdate::default()).unwrap();
        wizard
            .advance(&FirstBootUpdate {
                username: Some("harry".to_owned()),
                password_hash: Some("$6$x".to_owned()),
                ..Default::default()
            })
            .unwrap();
        wizard
            .advance(&FirstBootUpdate {
                locale: Some("de_DE.UTF-8".to_owned()),
                timezone: Some("Europe/Berlin".to_owned()),
                ..Default::default()
            })
            .unwrap();

        // Act: advance without setting keyboard_layout.
        let result = wizard.advance(&FirstBootUpdate::default());
        // Assert
        assert!(matches!(
            result,
            Err(FirstBootError::MissingField {
                field: "keyboard_layout"
            })
        ));
    }

    // ── AlreadyComplete guard ─────────────────────────────────────────────

    #[test]
    fn advance_after_complete_returns_error() {
        // Arrange: drive wizard to Complete.
        let mut wizard = FirstBootWizard::new();
        let u = full_update();
        // Advance through all steps.
        for _ in 0..8 {
            let _ = wizard.advance(&u);
        }
        assert_eq!(wizard.current_step(), FirstBootStep::Complete);
        // Act
        let result = wizard.advance(&FirstBootUpdate::default());
        // Assert
        assert!(matches!(result, Err(FirstBootError::AlreadyComplete)));
    }

    // ── generate_user_profile ─────────────────────────────────────────────

    #[test]
    fn generate_user_profile_creates_correct_paths() {
        // Arrange
        let root = tmp_root();
        let mut wizard = make_wizard_with_root(&root);
        wizard.config.username = "sigma".to_owned();
        // Act
        let profile = wizard.generate_user_profile().unwrap();
        // Assert
        assert_eq!(profile.home_dir, root.join("home").join("sigma"));
        assert_eq!(profile.xdg_dirs.downloads, root.join("home/sigma/Downloads"));
        assert_eq!(profile.shell, "/bin/zsh");
        assert!(profile.groups.contains(&"wheel".to_owned()));
    }

    #[test]
    fn generate_user_profile_fails_without_username() {
        // Arrange
        let wizard = FirstBootWizard::new(); // username empty
        // Act
        let result = wizard.generate_user_profile();
        // Assert
        assert!(matches!(
            result,
            Err(FirstBootError::MissingField { field: "username" })
        ));
    }

    // ── apply_locale ──────────────────────────────────────────────────────

    #[test]
    fn apply_locale_writes_locale_conf() {
        // Arrange
        let root = tmp_root();
        fs::create_dir_all(root.join("etc")).unwrap();
        let mut wizard = make_wizard_with_root(&root);
        wizard.config.locale = "ja_JP.UTF-8".to_owned();
        // Act
        wizard.apply_locale().unwrap();
        // Assert
        let contents = fs::read_to_string(root.join("etc/locale.conf")).unwrap();
        assert!(contents.contains("LANG=ja_JP.UTF-8"));
        assert!(contents.contains("LC_ALL=ja_JP.UTF-8"));
    }

    #[test]
    fn apply_locale_fails_without_locale() {
        // Arrange
        let root = tmp_root();
        let wizard = make_wizard_with_root(&root);
        // Act
        let result = wizard.apply_locale();
        // Assert
        assert!(matches!(
            result,
            Err(FirstBootError::MissingField { field: "locale" })
        ));
    }

    // ── apply_keyboard ────────────────────────────────────────────────────

    #[test]
    fn apply_keyboard_writes_vconsole_conf() {
        // Arrange
        let root = tmp_root();
        fs::create_dir_all(root.join("etc")).unwrap();
        let mut wizard = make_wizard_with_root(&root);
        wizard.config.keyboard_layout = "de".to_owned();
        wizard.config.keyboard_variant = Some("nodeadkeys".to_owned());
        // Act
        wizard.apply_keyboard().unwrap();
        // Assert
        let contents = fs::read_to_string(root.join("etc/vconsole.conf")).unwrap();
        assert!(contents.contains("KEYMAP=de"));
        assert!(contents.contains("KEYMAP_VARIANT=nodeadkeys"));
    }

    // ── apply_timezone ────────────────────────────────────────────────────

    #[test]
    fn apply_timezone_creates_symlink() {
        // Arrange
        let root = tmp_root();
        // Create the zoneinfo stub so the symlink target exists.
        let zone_dir = root.join("usr/share/zoneinfo/America");
        fs::create_dir_all(&zone_dir).unwrap();
        fs::write(zone_dir.join("Chicago"), b"TZif").unwrap();
        fs::create_dir_all(root.join("etc")).unwrap();

        let mut wizard = make_wizard_with_root(&root);
        wizard.config.timezone = "America/Chicago".to_owned();
        // Act
        wizard.apply_timezone().unwrap();
        // Assert
        let target = root.join("etc/localtime");
        assert!(target.symlink_metadata().is_ok(), "symlink should exist");
    }

    // ── finalize ──────────────────────────────────────────────────────────

    #[test]
    fn finalize_writes_marker_file() {
        // Arrange
        let root = tmp_root();
        fs::create_dir_all(root.join("etc")).unwrap();
        let mut wizard = make_wizard_with_root(&root);
        wizard.config.username = "sigma".to_owned();
        // Act
        wizard.finalize().unwrap();
        // Assert
        let marker = root.join("etc/sigma_firstboot_complete");
        assert!(marker.exists(), "marker file should be created");
    }

    #[test]
    fn finalize_fails_without_username() {
        // Arrange
        let root = tmp_root();
        let wizard = make_wizard_with_root(&root);
        // Act
        let result = wizard.finalize();
        // Assert
        assert!(matches!(
            result,
            Err(FirstBootError::MissingField { field: "username" })
        ));
    }
}

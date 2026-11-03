//! SigmaOS Installer
//! System installer and setup wizard

pub mod installer;
pub mod mintinstall;

pub use installer::*;
pub use mintinstall::*;

#[derive(Debug, Default, Clone)]
pub struct Installer;

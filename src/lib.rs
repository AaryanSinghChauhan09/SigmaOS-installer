//! SigmaOS Installer & First-Boot Setup Wizard
//! System installer, disk partitioner, and out-of-box setup wizard

#![allow(dead_code)]

pub mod config;
pub mod error;
pub mod installer;
pub mod mintinstall;
pub mod service;

pub use config::Config;
pub use error::{ComponentError, Result};
pub use installer::*;
pub use mintinstall::*;
pub use service::Service;

#[derive(Debug, Default, Clone)]
pub struct Installer;

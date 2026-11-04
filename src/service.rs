use crate::config::Config;
use crate::error::Result;
use log::{info, error};

pub struct Service {
    config: Config,
    running: bool,
}

impl Service {
    pub fn new(config: Config) -> Self {
        Service {
            config,
            running: false,
        }
    }

    pub fn start(&mut self) -> Result<()> {
        if self.running {
            error!("Service already running");
            return Err(crate::error::ComponentError::RuntimeError("Service already running".to_string()).into());
        }
        info!("Starting service");
        self.config.process()?;
        self.running = true;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        if !self.running {
            error!("Service not running");
            return Err(crate::error::ComponentError::RuntimeError("Service not running".to_string()).into());
        }
        info!("Stopping service");
        self.running = false;
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        self.running
    }
}

mod config;
mod error;
mod service;

use config::Config;
use service::Service;
use anyhow::Result;

fn main() -> Result<()> {
    env_logger::init();
    
    let config = Config::default();
    let mut service = Service::new(config);
    
    service.start()?;
    println!("Service started successfully");
    
    service.stop()?;
    println!("Service stopped successfully");
    
    Ok(())
}

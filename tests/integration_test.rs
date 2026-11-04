use anyhow::Result;
use sigmaos_component::{
    Config, FirstBootStep, FirstBootWizard, Service,
};

#[test]
fn test_installer_config_lifecycle_aaa() -> Result<()> {
    // Arrange
    let config = Config::default();

    // Act
    let res = config.process();

    // Assert
    assert!(res.is_ok());
    Ok(())
}

#[test]
fn test_installer_service_lifecycle_aaa() -> Result<()> {
    // Arrange
    let config = Config::default();
    let mut service = Service::new(config);
    assert!(!service.is_running());

    // Act
    service.start()?;

    // Assert
    assert!(service.is_running());
    assert!(service.start().is_err()); // Double start fails

    // Act
    service.stop()?;

    // Assert
    assert!(!service.is_running());
    assert!(service.stop().is_err()); // Double stop fails
    Ok(())
}

#[test]
fn test_first_boot_wizard_init_aaa() {
    // Arrange
    let wizard = FirstBootWizard::new();

    // Act
    let step = wizard.current_step();

    // Assert
    assert_eq!(step, FirstBootStep::Welcome);
}

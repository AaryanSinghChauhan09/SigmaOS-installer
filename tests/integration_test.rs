use sigmaos_installer::Component;

#[test]
fn test_component_lifecycle_aaa() {
    // Arrange
    let mut c = Component::new();
    assert!(!c.is_ready());
    // Act
    c.init().unwrap();
    // Assert
    assert!(c.is_ready());
}

#[test]
fn test_run_requires_init_aaa() {
    // Arrange
    let c = Component::new();
    // Act
    let res = c.run();
    // Assert
    assert!(res.is_err());
}

#[test]
fn test_run_after_init_aaa() {
    // Arrange
    let mut c = Component::new();
    c.init().unwrap();
    // Act
    let res = c.run();
    // Assert
    assert!(res.is_ok());
}

#[test]
fn test_reset_then_run_fails_aaa() {
    // Arrange
    let mut c = Component::new();
    c.init().unwrap();
    c.reset();
    // Act
    let res = c.run();
    // Assert
    assert!(res.is_err());
}

//! SigmaOS-installer — installer component for SigmaOS
#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// Graphical system installer module.
pub mod sigmainstaller;

use thiserror::Error;

/// Error types for this component.
#[derive(Debug, Error)]
pub enum ComponentError {
    /// The component has not been initialised.
    #[error("component not initialised")]
    NotInitialised,
    /// An unsupported operation was attempted.
    #[error("unsupported operation: {0}")]
    Unsupported(&'static str),
}

/// Convenience result type.
pub type ComponentResult<T> = Result<T, ComponentError>;

/// Core component struct for installer.
#[derive(Debug, Default, Clone)]
pub struct Component {
    initialised: bool,
    name: String,
}

impl Component {
    /// Create a new, uninitialised component.
    pub fn new() -> Self {
        Self {
            initialised: false,
            name: String::from("installer"),
        }
    }

    /// Initialise the component.
    pub fn init(&mut self) -> ComponentResult<()> {
        self.initialised = true;
        Ok(())
    }

    /// Returns true if the component has been initialised.
    pub fn is_ready(&self) -> bool {
        self.initialised
    }

    /// Execute the component's primary operation.
    pub fn run(&self) -> ComponentResult<()> {
        if !self.initialised {
            return Err(ComponentError::NotInitialised);
        }
        Ok(())
    }

    /// Return the component name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Reset the component to uninitialised state.
    pub fn reset(&mut self) {
        self.initialised = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_not_ready_aaa() {
        // Arrange + Act
        let c = Component::new();
        // Assert
        assert!(!c.is_ready());
    }

    #[test]
    fn test_init_makes_ready_aaa() {
        // Arrange
        let mut c = Component::new();
        // Act
        c.init().unwrap();
        // Assert
        assert!(c.is_ready());
    }

    #[test]
    fn test_run_before_init_fails_aaa() {
        // Arrange
        let c = Component::new();
        // Act
        let result = c.run();
        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn test_run_after_init_succeeds_aaa() {
        // Arrange
        let mut c = Component::new();
        c.init().unwrap();
        // Act
        let result = c.run();
        // Assert
        assert!(result.is_ok());
    }

    #[test]
    fn test_reset_clears_init_aaa() {
        // Arrange
        let mut c = Component::new();
        c.init().unwrap();
        assert!(c.is_ready());
        // Act
        c.reset();
        // Assert
        assert!(!c.is_ready());
    }

    #[test]
    fn test_name_returns_component_name_aaa() {
        // Arrange
        let c = Component::new();
        // Act
        let n = c.name();
        // Assert
        assert!(!n.is_empty());
    }
}

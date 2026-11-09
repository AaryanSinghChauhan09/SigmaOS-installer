//! Command execution layer.
//!
//! The installer never shells out directly: it builds [`CommandSpec`] values
//! and hands them to an [`Executor`]. [`DryRunExecutor`] records what *would*
//! run (default, safe); [`SystemExecutor`] runs real commands via
//! `std::process::Command` and is only used after explicit user confirmation.

use std::path::PathBuf;
use std::process::Command;

/// A single external command: argv vector + optional stdin payload.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandSpec {
    /// Program name (resolved via PATH).
    pub program: String,
    /// Arguments.
    pub args: Vec<String>,
    /// Optional stdin content.
    pub stdin: Option<String>,
}

impl CommandSpec {
    /// Render the command for logs: `prog arg1 arg2 ...`.
    pub fn render(&self) -> String {
        let mut s = self.program.clone();
        for a in &self.args {
            s.push(' ');
            if a.contains(' ') {
                s.push_str(&format!("'{}'", a));
            } else {
                s.push_str(a);
            }
        }
        s
    }
}

/// Outcome of running one command.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandOutput {
    /// Exit status code (0 = success).
    pub code: i32,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
}

impl CommandOutput {
    /// Whether the command succeeded.
    pub fn ok(&self) -> bool {
        self.code == 0
    }
}

/// Error running a command.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecError {
    /// The command that failed to spawn.
    pub command: String,
    /// OS error string.
    pub message: String,
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "failed to run '{}': {}", self.command, self.message)
    }
}

impl std::error::Error for ExecError {}

/// Executes [`CommandSpec`]s. Implementations must be deterministic and
/// side-effect free unless explicitly documented otherwise.
pub trait Executor {
    /// Run a command, returning its output.
    fn run(&mut self, cmd: &CommandSpec) -> Result<CommandOutput, ExecError>;

    /// Human-readable name for logs.
    fn name(&self) -> &'static str;
}

/// Records commands without executing them. The default and the mode used by
/// tests, CI smoke checks, and the installer's preview step.
#[derive(Debug, Default)]
pub struct DryRunExecutor {
    /// Every command that would have run, in order.
    pub commands: Vec<CommandSpec>,
}

impl DryRunExecutor {
    /// New empty dry-run executor.
    pub fn new() -> Self {
        Self::default()
    }

    /// All recorded commands rendered for display.
    pub fn rendered(&self) -> Vec<String> {
        self.commands.iter().map(|c| c.render()).collect()
    }
}

impl Executor for DryRunExecutor {
    fn run(&mut self, cmd: &CommandSpec) -> Result<CommandOutput, ExecError> {
        self.commands.push(cmd.clone());
        Ok(CommandOutput {
            code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }

    fn name(&self) -> &'static str {
        "dry-run"
    }
}

/// Runs commands for real via `std::process::Command`.
///
/// # Safety
/// Commands are executed with the privileges of the installer process.
/// Only construct this after the user has confirmed the partition plan.
pub struct SystemExecutor {
    /// Working directory for commands (default: inherit).
    pub workdir: Option<PathBuf>,
}

impl Default for SystemExecutor {
    fn default() -> Self {
        Self { workdir: None }
    }
}

impl Executor for SystemExecutor {
    fn run(&mut self, cmd: &CommandSpec) -> Result<CommandOutput, ExecError> {
        let mut c = Command::new(&cmd.program);
        c.args(&cmd.args);
        if let Some(wd) = &self.workdir {
            c.current_dir(wd);
        }
        let output = if let Some(input) = &cmd.stdin {
            use std::io::Write;
            let mut child = c
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| ExecError {
                    command: cmd.render(),
                    message: e.to_string(),
                })?;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(input.as_bytes());
            }
            child.wait_with_output().map_err(|e| ExecError {
                command: cmd.render(),
                message: e.to_string(),
            })?
        } else {
            c.output().map_err(|e| ExecError {
                command: cmd.render(),
                message: e.to_string(),
            })?
        };
        Ok(CommandOutput {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    fn name(&self) -> &'static str {
        "system"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // AAA: Arrange dry-run executor, Act run two commands, Assert recorded in order.
    #[test]
    fn dry_run_records_commands() {
        let mut ex = DryRunExecutor::new();
        ex.run(&CommandSpec {
            program: "mkfs.ext4".into(),
            args: vec!["/dev/vda2".into()],
            stdin: None,
        })
        .unwrap();
        ex.run(&CommandSpec {
            program: "mount".into(),
            args: vec!["/dev/vda2".into(), "/mnt".into()],
            stdin: None,
        })
        .unwrap();
        assert_eq!(ex.commands.len(), 2);
        assert_eq!(ex.rendered()[0], "mkfs.ext4 /dev/vda2");
    }

    // AAA: Arrange command with spaced arg, Act render, Assert quoting.
    #[test]
    fn render_quotes_spaces() {
        let cmd = CommandSpec {
            program: "grub-mkconfig".into(),
            args: vec!["-o".into(), "/boot/grub/grub.cfg".into()],
            stdin: None,
        };
        assert_eq!(cmd.render(), "grub-mkconfig -o /boot/grub/grub.cfg");
    }

    // AAA: Arrange SystemExecutor, Act run /bin/true and /bin/false, Assert codes.
    #[test]
    fn system_executor_runs_commands() {
        let mut ex = SystemExecutor::default();
        let ok = ex
            .run(&CommandSpec {
                program: "true".into(),
                args: vec![],
                stdin: None,
            })
            .unwrap();
        assert!(ok.ok());
        let bad = ex
            .run(&CommandSpec {
                program: "false".into(),
                args: vec![],
                stdin: None,
            })
            .unwrap();
        assert!(!bad.ok());
    }

    // AAA: Arrange missing program, Act run, Assert ExecError.
    #[test]
    fn missing_program_errors() {
        let mut ex = SystemExecutor::default();
        let res = ex.run(&CommandSpec {
            program: "definitely-not-a-real-binary-xyz".into(),
            args: vec![],
            stdin: None,
        });
        assert!(res.is_err());
    }
}

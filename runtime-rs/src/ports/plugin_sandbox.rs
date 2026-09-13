// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::ports::VersionedPort;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, ExitStatus};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginSandboxRequest {
    pub executable: PathBuf,
    pub install_root: PathBuf,
    pub allowed_domains: Vec<String>,
}

pub struct SandboxedPluginProcess {
    child: Child,
    settings_path: Option<PathBuf>,
}

impl SandboxedPluginProcess {
    pub fn new(child: Child, settings_path: Option<PathBuf>) -> Self {
        Self {
            child,
            settings_path,
        }
    }

    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin.take()
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub fn kill(&mut self) -> std::io::Result<()> {
        self.child.kill()
    }

    pub fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.child.wait()
    }
}

impl Drop for SandboxedPluginProcess {
    fn drop(&mut self) {
        if let Some(path) = self.settings_path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

pub trait PluginProcessSandboxPort: VersionedPort {
    fn spawn(&self, request: &PluginSandboxRequest) -> Result<SandboxedPluginProcess, String>;
}

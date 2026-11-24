use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// Samples resident memory of a process and all its descendants (a browser and its renderer processes).
#[derive(Debug)]
pub struct ProcessMemory {
    sys: System,
}

impl ProcessMemory {
    /// `None` if the platform cannot provide process information.
    pub fn new() -> Option<Self> {
        System::new().ok().map(|sys| Self { sys })
    }

    fn refresh(&mut self) {
        self.sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_memory());
    }

    /// Resident bytes of `pid` plus every descendant. `None` if the process does not exist.
    pub fn tree_rss(&mut self, pid: u32) -> Option<u64> {
        self.refresh();
        let root = Pid::from_u32(pid);
        self.sys.process(root)?;
        let mut total = 0;
        let mut stack = vec![root];
        while let Some(p) = stack.pop() {
            if let Some(proc) = self.sys.process(p) {
                total += proc.memory();
            }
            stack.extend(self.sys.processes().iter().filter(|(_, c)| c.parent() == Some(p)).map(|(id, _)| *id));
        }
        Some(total)
    }

    /// Resident bytes of the current process.
    pub fn own_rss(&mut self) -> u64 {
        self.tree_rss(std::process::id()).map_or(0, |bytes| bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_process_has_nonzero_rss_and_missing_pid_is_none() {
        let mut m = ProcessMemory::new().expect("process info available on CI platforms");
        assert!(m.own_rss() > 0);
        assert!(m.tree_rss(u32::MAX - 1).is_none());
    }
}

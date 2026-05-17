use std::any::Any;
use std::collections::HashSet;
use std::io::{Cursor, Read, Write};

use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System, MINIMUM_CPU_UPDATE_INTERVAL};

use crate::vfs::{DirChild, VfsDriver, VfsEntry, VfsError, VfsKind};
use crate::ws::protocol::{FilterSpec, SortSpec};

pub struct ProcsDriver {
    pub hierarchical: bool,
}

/// Parses the PID from an entry path.
///
/// Flat mode path:    `"{ppid1} | ... | {pid} - {name}"` → identified by ` | ` separators
/// Hierarchical path: `"{pid} - {name}/{pid2} - {name2}/..."` or bare `"{pid}/{pid2}/..."` →
///                    extracts the PID from the last `/`-delimited segment
pub(crate) fn pid_from_path(path: &str) -> Option<u32> {
    if path.contains(" | ") {
        // Flat mode: "ppid1 | ppid2 | ... | pid - name"
        path.splitn(2, " - ")
            .next()?
            .split(" | ")
            .last()?
            .trim()
            .parse::<u32>()
            .ok()
    } else {
        // Hierarchical mode: last segment may be "pid - name" or bare "pid"
        let last = path.split('/').last()?.trim();
        last.splitn(2, " - ").next()?.trim().parse::<u32>().ok()
    }
}

/// Walks parent links up to root and returns the full ancestor chain ending with the given PID.
fn build_pid_chain(sys: &System, start_pid: Pid) -> Vec<u32> {
    let mut chain = Vec::new();
    let mut visited = HashSet::new();
    let mut cursor = Some(start_pid);

    while let Some(pid) = cursor {
        if visited.contains(&pid) {
            break;
        }
        visited.insert(pid);
        chain.push(pid.as_u32());
        cursor = sys.process(pid).and_then(|p| p.parent());
        if chain.len() > 64 {
            break;
        }
    }
    chain.reverse();
    chain
}

/// Generates the textual detail block for a process.
fn process_detail(path: &str) -> Result<Vec<u8>, VfsError> {
    let pid_val = pid_from_path(path)
        .ok_or_else(|| VfsError::NotFound(format!("Cannot parse PID from: {path}")))?;

    let mut sys = System::new_all();
    sys.refresh_processes();

    let proc = sys
        .process(Pid::from_u32(pid_val))
        .ok_or_else(|| VfsError::NotFound(format!("Process {pid_val} not found")))?;

    let mut text = String::new();
    text.push_str(&format!("PID:         {}\n", pid_val));
    text.push_str(&format!("Name:        {}\n", proc.name()));
    text.push_str(&format!("Status:      {:?}\n", proc.status()));
    text.push_str(&format!("CPU usage:   {:.2}%\n", proc.cpu_usage()));
    text.push_str(&format!("Memory:      {} KiB\n", proc.memory() / 1024));
    text.push_str(&format!(
        "Virtual mem: {} KiB\n",
        proc.virtual_memory() / 1024
    ));
    text.push_str(&format!("Start time:  {}\n", proc.start_time()));
    text.push_str(&format!("Run time:    {}s\n", proc.run_time()));
    if let Some(uid) = proc.user_id() {
        text.push_str(&format!("User ID:     {}\n", **uid));
    }
    #[cfg(unix)]
    if let Some(gid) = proc.group_id() {
        text.push_str(&format!("Group ID:    {}\n", *gid));
    }
    let cmd = proc.cmd().join(" ");
    text.push_str(&format!("Command:     {}\n", cmd));
    text.push_str(&format!(
        "Executable:  {}\n",
        proc.exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    ));
    text.push_str(&format!(
        "CWD:         {}\n",
        proc.cwd()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default()
    ));
    text.push_str(&format!("Env vars:    {}\n", proc.environ().len()));

    #[cfg(target_os = "linux")]
    {
        let fd_dir = format!("/proc/{}/fd", pid_val);
        if let Ok(fds) = std::fs::read_dir(&fd_dir) {
            let open_files: Vec<String> = fds
                .filter_map(|e| e.ok())
                .filter_map(|e| std::fs::read_link(e.path()).ok())
                .map(|p| p.to_string_lossy().to_string())
                .filter(|s| {
                    !s.starts_with("socket:")
                        && !s.starts_with("pipe:")
                        && !s.starts_with("anon_inode:")
                })
                .collect();
            if !open_files.is_empty() {
                text.push_str("\nOpen files:\n");
                for f in open_files.iter().take(64) {
                    text.push_str(&format!("  {f}\n"));
                }
                if open_files.len() > 64 {
                    text.push_str(&format!("  ... and {} more\n", open_files.len() - 64));
                }
            }
        }
    }

    Ok(text.into_bytes())
}

impl VfsDriver for ProcsDriver {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn list(
        &self,
        path: &str,
        sort: &SortSpec,
        filter: Option<&FilterSpec>,
        _show_hidden: bool,
    ) -> Result<Vec<VfsEntry>, VfsError> {
        let mut sys = System::new_with_specifics(
            RefreshKind::new().with_processes(ProcessRefreshKind::everything()),
        );
        // First refresh creates new ProcessInner entries with old_stime/old_utime = 0.
        // A second refresh sets those baselines; the sleep + third refresh computes the delta.
        sys.refresh_processes_specifics(ProcessRefreshKind::new().with_cpu());
        std::thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL);
        sys.refresh_processes_specifics(ProcessRefreshKind::new().with_cpu());

        let num_cpus = sys.cpus().len().max(1) as f32;
        let filter_lower = filter.map(|f| f.value.to_lowercase());

        let mut entries: Vec<VfsEntry> = if self.hierarchical {
            self.list_hierarchical(&sys, path, num_cpus, filter_lower.as_deref())
        } else {
            self.list_flat(&sys, num_cpus, filter_lower.as_deref())
        };

        match (sort.by.as_str(), sort.order.as_str()) {
            ("size", "desc") => entries.sort_by(|a, b| b.size.cmp(&a.size)),
            ("size", _) => entries.sort_by(|a, b| a.size.cmp(&b.size)),
            ("mtime", "desc") => entries.sort_by(|a, b| b.mtime.cmp(&a.mtime)),
            ("mtime", _) => entries.sort_by(|a, b| a.mtime.cmp(&b.mtime)),
            ("name", "desc") => {
                entries.sort_by(|a, b| b.name.to_lowercase().cmp(&a.name.to_lowercase()))
            }
            _ => entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
        }

        Ok(entries)
    }

    fn resolve_child(&self, base: &str, name: &str) -> String {
        if !self.hierarchical {
            return name.to_string();
        }
        // Store the full "pid - name" as the path segment so that navigate_up
        // can use the segment directly as a cursor hint to restore focus.
        if base.is_empty() {
            name.to_string()
        } else {
            format!("{base}/{name}")
        }
    }

    fn parent_path(&self, path: &str) -> Option<String> {
        if !self.hierarchical {
            return None;
        }
        if path.is_empty() {
            None // at tree root — exit procs entirely
        } else if let Some(slash) = path.rfind('/') {
            Some(path[..slash].to_string())
        } else {
            Some(String::new()) // one level deep → go back to tree root
        }
    }

    fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, VfsError> {
        let bytes = process_detail(path)?;
        Ok(Box::new(Cursor::new(bytes)))
    }

    fn open_write(&self, _path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
        Err(VfsError::NotSupported("procs source is read-only".into()))
    }

    fn create_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported("procs source is read-only".into()))
    }

    fn remove_file(&self, path: &str) -> Result<(), VfsError> {
        let pid_val = pid_from_path(path)
            .ok_or_else(|| VfsError::NotFound(format!("Cannot parse PID from: {path}")))?;

        let mut sys = System::new_all();
        sys.refresh_processes();

        let proc = sys
            .process(Pid::from_u32(pid_val))
            .ok_or_else(|| VfsError::NotFound(format!("Process {pid_val} not found")))?;

        #[cfg(unix)]
        {
            let sent = proc.kill_with(sysinfo::Signal::Term).unwrap_or(false);
            if !sent {
                return Err(VfsError::PermissionDenied(format!(
                    "Failed to send SIGTERM to process {pid_val}"
                )));
            }
        }
        #[cfg(windows)]
        {
            if !proc.kill() {
                return Err(VfsError::PermissionDenied(format!(
                    "Failed to terminate process {pid_val}"
                )));
            }
        }

        Ok(())
    }

    fn remove_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported("procs source is read-only".into()))
    }

    fn exists(&self, path: &str) -> bool {
        let Some(pid_val) = pid_from_path(path) else {
            return false;
        };
        let mut sys = System::new();
        sys.refresh_processes();
        sys.process(Pid::from_u32(pid_val)).is_some()
    }

    fn file_size(&self, path: &str) -> Option<u64> {
        process_detail(path).ok().map(|b| b.len() as u64)
    }

    fn read_dir_entries(&self, _path: &str) -> Result<Vec<DirChild>, VfsError> {
        Ok(vec![])
    }

    fn read_chunk(&self, path: &str, offset: u64, size: usize) -> Result<Vec<u8>, VfsError> {
        let all = process_detail(path)?;
        let start = (offset as usize).min(all.len());
        let end = (start + size).min(all.len());
        Ok(all[start..end].to_vec())
    }

    fn try_rename(&self, _src: &str, _dst: &str) -> Result<bool, VfsError> {
        Err(VfsError::NotSupported("Cannot rename processes".into()))
    }
}

impl ProcsDriver {
    fn list_flat(
        &self,
        sys: &System,
        num_cpus: f32,
        filter_lower: Option<&str>,
    ) -> Vec<VfsEntry> {
        sys.processes()
            .values()
            .filter_map(|proc| {
                let pid = proc.pid();
                let name = proc.name().to_string();

                let chain = build_pid_chain(sys, pid);
                let chain_str = chain
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(" | ");
                let entry_name = format!("{chain_str} - {name}");

                if let Some(needle) = filter_lower {
                    let entry_lower = entry_name.to_lowercase();
                    if !needle
                        .split('|')
                        .any(|part| entry_lower.contains(part.trim()))
                    {
                        return None;
                    }
                }

                let size = (proc.cpu_usage() / num_cpus * 10.0).clamp(0.0, 1000.0) as u64;
                let mtime = proc.start_time().to_string();
                let permissions = proc
                    .user_id()
                    .map(|uid| format!("uid:{}", **uid))
                    .or_else(|| Some("unknown".to_string()));

                Some(VfsEntry {
                    name: entry_name,
                    kind: VfsKind::File,
                    size: Some(size),
                    mtime: Some(mtime.clone()),
                    btime: Some(mtime),
                    permissions,
                    icon_name: Some("system-run".to_string()),
                    cc: None,
                    source_path: None,
                })
            })
            .collect()
    }

    fn list_hierarchical(
        &self,
        sys: &System,
        path: &str,
        num_cpus: f32,
        filter_lower: Option<&str>,
    ) -> Vec<VfsEntry> {
        // Normalise parent: processes whose parent PID is absent from the
        // running process list are treated as roots (listed at path "").
        let all_pids: HashSet<Pid> = sys.processes().keys().cloned().collect();

        // Target parent PID: None = root level, Some(pid) = children of that pid.
        // Path segments may be bare "pid" or full "pid - name"; extract the numeric PID.
        let target_parent: Option<Pid> = if path.is_empty() {
            None
        } else {
            path.split('/').last()
                .and_then(|s| s.trim().splitn(2, " - ").next()?.trim().parse::<u32>().ok())
                .map(Pid::from_u32)
        };

        // Build a set of PIDs that have at least one child (for Directory kind)
        let parent_pids: HashSet<Pid> = sys
            .processes()
            .values()
            .filter_map(|p| p.parent().filter(|pp| all_pids.contains(pp)))
            .collect();

        let result: Vec<VfsEntry> = sys.processes()
            .values()
            .filter_map(|proc| {
                // Normalise the process's effective parent
                let effective_parent = proc.parent().filter(|pp| all_pids.contains(pp));
                if effective_parent != target_parent {
                    return None;
                }

                let pid = proc.pid();
                let name = proc.name().to_string();
                let entry_name = format!("{} - {name}", pid.as_u32());

                if let Some(needle) = filter_lower {
                    let entry_lower = entry_name.to_lowercase();
                    if !needle
                        .split('|')
                        .any(|part| entry_lower.contains(part.trim()))
                    {
                        return None;
                    }
                }

                let is_parent = parent_pids.contains(&pid);
                let kind = if is_parent { VfsKind::Directory } else { VfsKind::File };
                let icon_name = if is_parent { "folder" } else { "system-run" };
                let cc = if is_parent { Some("cc-dir".to_string()) } else { None };
                let size = (proc.cpu_usage() / num_cpus * 10.0).clamp(0.0, 1000.0) as u64;
                let mtime = proc.start_time().to_string();
                let permissions = proc
                    .user_id()
                    .map(|uid| format!("uid:{}", **uid))
                    .or_else(|| Some("unknown".to_string()));

                Some(VfsEntry {
                    name: entry_name,
                    kind,
                    size: Some(size),
                    mtime: Some(mtime.clone()),
                    btime: Some(mtime),
                    permissions,
                    icon_name: Some(icon_name.to_string()),
                    cc,
                    source_path: None,
                })
            })
            .collect();
        result
    }
}

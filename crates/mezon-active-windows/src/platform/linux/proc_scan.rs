use crate::catalog::{ActivityKind, match_linux_process, running_process_kind_priority};
use crate::info::ActiveWindowInfo;

struct ProcMatch {
    app_name: String,
    kind: ActivityKind,
    pid: u32,
    role_score: u8,
}

pub fn scan_tracked_process() -> Option<ActiveWindowInfo> {
    let uid = current_uid();
    let mut matches = Vec::new();
    let mut owned_processes = 0usize;
    let entries = std::fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let Some(pid) = parse_proc_pid(&entry.file_name().to_string_lossy()) else {
            continue;
        };
        let Some(puid) = process_uid(pid) else {
            continue;
        };
        if puid != uid {
            continue;
        }
        owned_processes += 1;
        let comm = process_comm(pid).unwrap_or_default();
        let cmdline = process_cmdline(pid).unwrap_or_default();
        let exe = process_exe(pid);
        let Some((app_name, kind)) = match_linux_process(&comm, &cmdline, exe.as_deref()) else {
            continue;
        };
        matches.push(ProcMatch {
            app_name,
            kind,
            pid,
            role_score: process_role_score(&cmdline),
        });
    }
    let best = pick_best_proc_match(matches).or_else(|| {
        tracing::debug!(
            uid,
            owned_processes,
            "proc scan found no tracked process for current user"
        );
        None
    })?;
    tracing::debug!(
        uid,
        app_name = best.app_name,
        pid = best.pid,
        "proc scan matched tracked process"
    );
    Some(ActiveWindowInfo {
        os: "linux".to_string(),
        window_class: best.app_name,
        window_name: String::new(),
        window_desktop: "0".to_string(),
        window_type: "0".to_string(),
        window_pid: best.pid.to_string(),
        idle_time: "0".to_string(),
    })
}

fn pick_best_proc_match(matches: Vec<ProcMatch>) -> Option<ProcMatch> {
    let mut best: Option<ProcMatch> = None;
    for item in matches {
        let kind_priority = running_process_kind_priority(item.kind);
        let replace = best
            .as_ref()
            .map(|current| {
                let current_kind = running_process_kind_priority(current.kind);
                kind_priority > current_kind
                    || (kind_priority == current_kind && item.role_score > current.role_score)
            })
            .unwrap_or(true);
        if replace {
            best = Some(item);
        }
    }
    best
}

fn process_role_score(cmdline: &str) -> u8 {
    if !cmdline.contains("--type=") {
        return 3;
    }
    if cmdline.contains("--type=renderer") {
        return 2;
    }
    if cmdline.contains("--type=zygote") || cmdline.contains("--type=gpu-process") {
        return 0;
    }
    1
}

fn parse_proc_pid(name: &str) -> Option<u32> {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    name.parse().ok()
}

fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

fn process_uid(pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

fn process_comm(pid: u32) -> Option<String> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let comm = comm.trim();
    if comm.is_empty() {
        None
    } else {
        Some(comm.to_string())
    }
}

fn process_cmdline(pid: u32) -> Option<String> {
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    if cmdline.is_empty() {
        return None;
    }
    let joined = cmdline
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

fn process_exe(pid: u32) -> Option<String> {
    let path = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    path.into_os_string().into_string().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{ActivityKind, match_linux_process};

    #[test]
    fn parse_proc_pid_skips_non_numeric_proc_entries() {
        let names = ["1", "self", "cpuinfo", "171399", "thread-self", "net", "2"];
        let pids: Vec<u32> = names
            .iter()
            .filter_map(|name| parse_proc_pid(name))
            .collect();
        assert_eq!(pids, vec![1, 171399, 2]);
    }

    #[test]
    fn process_role_score_prefers_main_process() {
        assert_eq!(process_role_score("/usr/share/cursor/cursor"), 3);
        assert_eq!(
            process_role_score("/usr/share/cursor/cursor --type=zygote"),
            0
        );
    }

    #[test]
    fn pick_best_proc_match_prefers_work_over_play() {
        let picked = pick_best_proc_match(vec![
            ProcMatch {
                app_name: "LeagueClientUx".into(),
                kind: ActivityKind::Play,
                pid: 10,
                role_score: 3,
            },
            ProcMatch {
                app_name: "Cursor".into(),
                kind: ActivityKind::Coding,
                pid: 11,
                role_score: 3,
            },
        ])
        .expect("work activity");
        assert_eq!(picked.app_name, "Cursor");
    }

    #[test]
    fn pick_best_proc_match_prefers_main_cursor_pid() {
        let picked = pick_best_proc_match(vec![
            ProcMatch {
                app_name: "Cursor".into(),
                kind: ActivityKind::Coding,
                pid: 10,
                role_score: 0,
            },
            ProcMatch {
                app_name: "Cursor".into(),
                kind: ActivityKind::Coding,
                pid: 11,
                role_score: 3,
            },
        ])
        .expect("cursor pid");
        assert_eq!(picked.pid, 11);
    }

    #[test]
    fn match_linux_process_detects_cursor_install_paths() {
        assert_eq!(
            match_linux_process(
                "cursor",
                "/usr/share/cursor/cursor --type=zygote",
                Some("/usr/share/cursor/cursor"),
            ),
            Some(("Cursor".into(), ActivityKind::Coding))
        );
    }
}

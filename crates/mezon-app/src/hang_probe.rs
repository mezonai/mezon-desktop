#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::collections::HashMap;
use std::time::Duration;

const SAMPLE_WINDOW: Duration = Duration::from_secs(1);
const MAX_BUSY_THREADS: usize = 6;

#[derive(Debug, PartialEq)]
struct ThreadStat {
    comm: String,
    state: char,
    cpu_ticks: u64,
    major_faults: u64,
}

struct ThreadRow<'a> {
    tid: u32,
    stat: &'a ThreadStat,
    window_cpu_ticks: u64,
    window_major_faults: u64,
}

fn parse_stat(line: &str) -> Option<ThreadStat> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    let comm = line.get(open + 1..close)?.to_string();
    let mut fields = line.get(close + 1..)?.split_whitespace();
    let state = fields.next()?.chars().next()?;
    let rest: Vec<&str> = fields.collect();
    let major_faults = rest.get(8)?.parse().ok()?;
    let utime: u64 = rest.get(10)?.parse().ok()?;
    let stime: u64 = rest.get(11)?.parse().ok()?;
    Some(ThreadStat {
        comm,
        state,
        cpu_ticks: utime + stime,
        major_faults,
    })
}

fn field_kb(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        line.strip_prefix(key)?
            .strip_prefix(':')?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    })
}

fn mb(kb: Option<u64>) -> String {
    kb.map_or_else(|| "?".to_string(), |kb| (kb / 1024).to_string())
}

fn snapshot() -> HashMap<u32, ThreadStat> {
    let Ok(entries) = std::fs::read_dir("/proc/self/task") else {
        return HashMap::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let tid = entry.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            Some((tid, parse_stat(&stat)?))
        })
        .collect()
}

fn task_file(tid: u32, name: &str) -> String {
    std::fs::read_to_string(format!("/proc/self/task/{tid}/{name}"))
        .ok()
        .and_then(|text| text.split_whitespace().next().map(str::to_string))
        .unwrap_or_else(|| "?".to_string())
}

fn describe(label: &str, row: &ThreadRow<'_>) -> String {
    format!(
        "[watchdog]   {label} tid={} comm={} state={} cpu_ticks_last_1s={} major_faults_last_1s={} syscall={} wchan={}",
        row.tid,
        row.stat.comm,
        row.stat.state,
        row.window_cpu_ticks,
        row.window_major_faults,
        task_file(row.tid, "syscall"),
        task_file(row.tid, "wchan"),
    )
}

fn memory_line() -> String {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let pressure = std::fs::read_to_string("/proc/pressure/memory")
        .ok()
        .and_then(|text| text.lines().next().map(str::to_string))
        .unwrap_or_else(|| "?".to_string());
    format!(
        "[watchdog]   memory: rss={}MB swapped={}MB available={}MB swap_free={}/{}MB pressure=[{}]",
        mb(field_kb(&status, "VmRSS")),
        mb(field_kb(&status, "VmSwap")),
        mb(field_kb(&meminfo, "MemAvailable")),
        mb(field_kb(&meminfo, "SwapFree")),
        mb(field_kb(&meminfo, "SwapTotal")),
        pressure,
    )
}

pub fn report(stalled_secs: u64) -> Vec<String> {
    let before = snapshot();
    std::thread::sleep(SAMPLE_WINDOW);
    let after = snapshot();
    let main_tid = std::process::id();

    let mut rows: Vec<ThreadRow<'_>> = after
        .iter()
        .map(|(tid, stat)| {
            let previous = before.get(tid);
            ThreadRow {
                tid: *tid,
                stat,
                window_cpu_ticks: stat
                    .cpu_ticks
                    .saturating_sub(previous.map_or(stat.cpu_ticks, |p| p.cpu_ticks)),
                window_major_faults: stat
                    .major_faults
                    .saturating_sub(previous.map_or(stat.major_faults, |p| p.major_faults)),
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.window_cpu_ticks
            .cmp(&a.window_cpu_ticks)
            .then(a.tid.cmp(&b.tid))
    });

    let mut lines = vec![format!(
        "[watchdog] main thread stalled {stalled_secs}s; {} threads sampled over {}ms",
        rows.len(),
        SAMPLE_WINDOW.as_millis()
    )];
    match rows.iter().find(|row| row.tid == main_tid) {
        Some(row) => lines.push(describe("main", row)),
        None => lines.push(format!("[watchdog]   main tid={main_tid} not readable")),
    }
    lines.extend(
        rows.iter()
            .filter(|row| {
                row.tid != main_tid && (row.window_cpu_ticks > 0 || row.stat.state == 'D')
            })
            .take(MAX_BUSY_THREADS)
            .map(|row| describe("busy", row)),
    );
    lines.push(memory_line());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stat_reads_state_cpu_and_faults_past_a_comm_with_parens() {
        let line = "4242 (mezon (io) x) S 1 4242 4242 0 -1 4194560 1200 0 37 0 150 25 0 0 20 0 9 0 100 0 0";
        assert_eq!(
            parse_stat(line),
            Some(ThreadStat {
                comm: "mezon (io) x".to_string(),
                state: 'S',
                cpu_ticks: 175,
                major_faults: 37,
            })
        );
    }

    #[test]
    fn parse_stat_rejects_a_truncated_line() {
        assert_eq!(parse_stat("4242 (mezon) R 1 4242"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn report_describes_the_main_thread_and_memory_from_proc() {
        let lines = report(39);
        assert!(lines[0].starts_with("[watchdog] main thread stalled 39s;"));
        let main = lines
            .iter()
            .find(|line| line.contains("  main tid="))
            .expect("main thread line");
        let state = main
            .split("state=")
            .nth(1)
            .and_then(|rest| rest.chars().next())
            .expect("state field");
        assert!(
            "RSDTtZXIPWK".contains(state),
            "unexpected thread state {state}"
        );
        assert!(main.contains(&format!("tid={}", std::process::id())));
        let memory = lines.last().expect("memory line");
        assert!(memory.contains("memory: rss="));
        assert!(!memory.contains("rss=?MB"));
    }

    #[test]
    fn field_kb_reads_the_named_kilobyte_field_only() {
        let status = "VmRSSmax:\t1 kB\nVmRSS:\t  524288 kB\nVmSwap:\t0 kB\n";
        assert_eq!(field_kb(status, "VmRSS"), Some(524_288));
        assert_eq!(field_kb(status, "VmSwap"), Some(0));
        assert_eq!(field_kb(status, "SwapFree"), None);
        assert_eq!(mb(field_kb(status, "VmRSS")), "512");
    }
}

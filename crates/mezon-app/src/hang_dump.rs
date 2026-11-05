#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::time::Duration;

const DUMP_PREFIX: &str = "mezon-hang-";
const DUMP_SUFFIX: &str = ".dmp";
const KEPT_DUMPS: usize = 3;
const CPU_WINDOW: Duration = Duration::from_secs(2);

fn dump_name(unix_secs: u64) -> String {
    format!("{DUMP_PREFIX}{unix_secs}{DUMP_SUFFIX}")
}

fn dump_stamp(path: &Path) -> Option<u64> {
    path.file_name()?
        .to_str()?
        .strip_prefix(DUMP_PREFIX)?
        .strip_suffix(DUMP_SUFFIX)?
        .parse()
        .ok()
}

fn dumps_to_prune(mut dumps: Vec<PathBuf>, keep: usize) -> Vec<PathBuf> {
    dumps.retain(|path| dump_stamp(path).is_some());
    dumps.sort_by_key(|path| dump_stamp(path));
    let excess = dumps.len().saturating_sub(keep);
    dumps.truncate(excess);
    dumps
}

fn prune_old_dumps(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let dumps = entries.flatten().map(|entry| entry.path()).collect();
    for path in dumps_to_prune(dumps, keep) {
        let _ = std::fs::remove_file(path);
    }
}

fn cpu_line(used: Option<Duration>) -> String {
    match used {
        Some(used) => format!(
            "main thread used {}ms cpu in the last {}ms",
            used.as_millis(),
            CPU_WINDOW.as_millis()
        ),
        None => "main thread cpu time not readable".to_string(),
    }
}

#[cfg(windows)]
pub use platform::{capture, remember_main_thread};

#[cfg(windows)]
mod platform {
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
    use windows::Win32::System::Diagnostics::Debug::{
        MINIDUMP_TYPE, MiniDumpNormal, MiniDumpWithProcessThreadData, MiniDumpWithThreadInfo,
        MiniDumpWithUnloadedModules, MiniDumpWriteDump,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, GetThreadTimes, OpenThread,
        THREAD_QUERY_LIMITED_INFORMATION,
    };

    use super::{CPU_WINDOW, KEPT_DUMPS, cpu_line, dump_name, prune_old_dumps};

    static MAIN_THREAD_ID: AtomicU32 = AtomicU32::new(0);

    pub fn remember_main_thread() {
        MAIN_THREAD_ID.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
    }

    pub fn capture(stalled_secs: u64, dir: &Path, emit: impl Fn(String)) {
        emit(format!(
            "[watchdog] main thread stalled {stalled_secs}s; {}",
            cpu_line(main_thread_cpu_over_window())
        ));
        emit(write_dump(dir));
    }

    fn main_thread_cpu_over_window() -> Option<Duration> {
        let thread_id = MAIN_THREAD_ID.load(Ordering::Relaxed);
        if thread_id == 0 {
            return None;
        }
        let thread =
            unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, thread_id) }.ok()?;
        let before = thread_cpu(thread);
        std::thread::sleep(CPU_WINDOW);
        let after = thread_cpu(thread);
        unsafe {
            let _ = CloseHandle(thread);
        }
        Some(after?.saturating_sub(before?))
    }

    fn thread_cpu(thread: HANDLE) -> Option<Duration> {
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        unsafe { GetThreadTimes(thread, &mut creation, &mut exit, &mut kernel, &mut user) }.ok()?;
        let ticks = filetime_ticks(kernel) + filetime_ticks(user);
        Some(Duration::from_nanos(ticks.saturating_mul(100)))
    }

    fn filetime_ticks(time: FILETIME) -> u64 {
        (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
    }

    fn write_dump(dir: &Path) -> String {
        if let Err(error) = std::fs::create_dir_all(dir) {
            return format!("[watchdog] hang dump skipped: {error}");
        }
        prune_old_dumps(dir, KEPT_DUMPS - 1);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default();
        let path = dir.join(dump_name(stamp));
        let file = match std::fs::File::create(&path) {
            Ok(file) => file,
            Err(error) => return format!("[watchdog] hang dump skipped: {error}"),
        };
        let started = Instant::now();
        let dump_type = MINIDUMP_TYPE(
            MiniDumpNormal.0
                | MiniDumpWithThreadInfo.0
                | MiniDumpWithUnloadedModules.0
                | MiniDumpWithProcessThreadData.0,
        );
        let written = unsafe {
            MiniDumpWriteDump(
                GetCurrentProcess(),
                GetCurrentProcessId(),
                HANDLE(file.as_raw_handle()),
                dump_type,
                None,
                None,
                None,
            )
        };
        let size_kb = file.metadata().map(|meta| meta.len() / 1024).unwrap_or(0);
        drop(file);
        match written {
            Ok(()) => format!(
                "[watchdog] hang dump written to {} ({size_kb} KB in {}ms)",
                path.display(),
                started.elapsed().as_millis()
            ),
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                format!("[watchdog] hang dump failed: {error}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dump_names_round_trip_their_stamp() {
        let name = dump_name(1_789_114_889);
        assert_eq!(name, "mezon-hang-1789114889.dmp");
        assert_eq!(dump_stamp(Path::new(&name)), Some(1_789_114_889));
    }

    #[test]
    fn only_hang_dumps_carry_a_stamp() {
        for other in [
            "mezon.log.2026-11-05",
            "mezon-hang-1789114889.txt",
            "mezon-hang-.dmp",
            "mezon-hang-abc.dmp",
        ] {
            assert_eq!(dump_stamp(Path::new(other)), None, "{other}");
        }
    }

    #[test]
    fn pruning_drops_the_oldest_dumps_and_ignores_other_files() {
        let dir = Path::new("logs");
        let files = vec![
            dir.join("mezon-hang-300.dmp"),
            dir.join("mezon.log.2026-11-05"),
            dir.join("mezon-hang-100.dmp"),
            dir.join("mezon-hang-1000.dmp"),
            dir.join("mezon-hang-200.dmp"),
        ];
        assert_eq!(
            dumps_to_prune(files, 2),
            vec![
                dir.join("mezon-hang-100.dmp"),
                dir.join("mezon-hang-200.dmp")
            ]
        );
    }

    #[test]
    fn pruning_keeps_everything_under_the_limit() {
        let files = vec![PathBuf::from("mezon-hang-1.dmp")];
        assert!(dumps_to_prune(files, 2).is_empty());
    }

    #[test]
    fn prune_old_dumps_removes_files_on_disk() {
        let dir = std::env::temp_dir().join(format!("mezon-hang-dump-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for stamp in [10, 20, 30] {
            std::fs::write(dir.join(dump_name(stamp)), b"dump").unwrap();
        }
        std::fs::write(dir.join("mezon.log.2026-11-05"), b"log").unwrap();
        prune_old_dumps(&dir, 1);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(left, vec!["mezon-hang-30.dmp", "mezon.log.2026-11-05"]);
    }

    #[test]
    fn cpu_line_reports_the_window() {
        assert_eq!(
            cpu_line(Some(Duration::from_millis(1950))),
            "main thread used 1950ms cpu in the last 2000ms"
        );
        assert_eq!(cpu_line(None), "main thread cpu time not readable");
    }

    #[cfg(windows)]
    #[test]
    fn capture_dumps_a_blocked_main_thread() {
        use std::sync::{Mutex, mpsc};

        let dir =
            std::env::temp_dir().join(format!("mezon-hang-capture-test-{}", std::process::id()));
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let blocked = std::thread::spawn(move || {
            remember_main_thread();
            ready_tx.send(()).unwrap();
            let _ = release_rx.recv();
        });
        ready_rx.recv().unwrap();

        let lines = Mutex::new(Vec::new());
        capture(31, &dir, |line| lines.lock().unwrap().push(line));
        release_tx.send(()).unwrap();
        blocked.join().unwrap();

        let lines = lines.into_inner().unwrap();
        let dumps: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| dump_stamp(path).is_some())
            .collect();
        let header = dumps
            .first()
            .map(|path| std::fs::read(path).unwrap()[..4].to_vec());
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(
            lines[0].starts_with("[watchdog] main thread stalled 31s; main thread used "),
            "{lines:?}"
        );
        assert!(
            lines[1].starts_with("[watchdog] hang dump written to "),
            "{lines:?}"
        );
        assert_eq!(dumps.len(), 1);
        assert_eq!(header.as_deref(), Some(&b"MDMP"[..]));
    }
}

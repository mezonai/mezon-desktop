#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::path::Path;

const RECORD_FILE: &str = "unclean-exit.txt";
const LINE_CAPACITY: usize = 256;

struct LineBuf {
    bytes: [u8; LINE_CAPACITY],
    len: usize,
}

impl LineBuf {
    const fn new() -> Self {
        Self {
            bytes: [0; LINE_CAPACITY],
            len: 0,
        }
    }

    fn push_bytes(&mut self, bytes: &[u8]) {
        let room = LINE_CAPACITY - 1 - self.len;
        let take = bytes.len().min(room);
        self.bytes[self.len..self.len + take].copy_from_slice(&bytes[..take]);
        self.len += take;
    }

    fn push_str(&mut self, text: &str) {
        self.push_bytes(text.as_bytes());
    }

    fn push_dec(&mut self, mut value: u64) {
        let mut digits = [0u8; 20];
        let mut start = digits.len();
        loop {
            start -= 1;
            digits[start] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        self.push_bytes(&digits[start..]);
    }

    fn push_signed(&mut self, value: i64) {
        if value < 0 {
            self.push_str("-");
        }
        self.push_dec(value.unsigned_abs());
    }

    fn push_hex(&mut self, mut value: u64) {
        let mut digits = [0u8; 16];
        let mut start = digits.len();
        loop {
            start -= 1;
            digits[start] = b"0123456789abcdef"[(value & 0xf) as usize];
            value >>= 4;
            if value == 0 {
                break;
            }
        }
        self.push_str("0x");
        self.push_bytes(&digits[start..]);
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    fn finish_line(&mut self) -> &[u8] {
        self.bytes[self.len] = b'\n';
        &self.bytes[..=self.len]
    }
}

struct Mapping<'a> {
    start: u64,
    end: u64,
    file_offset: u64,
    path: &'a [u8],
}

fn parse_hex(field: &[u8]) -> Option<u64> {
    u64::from_str_radix(std::str::from_utf8(field).ok()?, 16).ok()
}

fn parse_mapping(line: &[u8]) -> Option<Mapping<'_>> {
    let mut rest = line;
    let mut fields: [&[u8]; 5] = [&[]; 5];
    for field in &mut fields {
        rest = rest.trim_ascii_start();
        let end = rest
            .iter()
            .position(|byte| *byte == b' ')
            .unwrap_or(rest.len());
        *field = &rest[..end];
        rest = &rest[end..];
    }
    let dash = fields[0].iter().position(|byte| *byte == b'-')?;
    Some(Mapping {
        start: parse_hex(&fields[0][..dash])?,
        end: parse_hex(&fields[0][dash + 1..])?,
        file_offset: parse_hex(fields[2])?,
        path: rest.trim_ascii(),
    })
}

fn describe_location(mapping: &Mapping<'_>, pc: u64) -> LineBuf {
    let mut line = LineBuf::new();
    line.push_str("pc_in=");
    if mapping.path.is_empty() {
        line.push_str("[anonymous]");
    } else {
        line.push_bytes(mapping.path);
    }
    line.push_str("+");
    line.push_hex(pc - mapping.start + mapping.file_offset);
    line
}

struct MapsScanner {
    pc: u64,
    line: LineBuf,
}

impl MapsScanner {
    const fn new(pc: u64) -> Self {
        Self {
            pc,
            line: LineBuf::new(),
        }
    }

    fn feed(&mut self, chunk: &[u8]) -> Option<LineBuf> {
        for &byte in chunk {
            if byte != b'\n' {
                self.line.push_bytes(&[byte]);
                continue;
            }
            let hit = parse_mapping(self.line.as_bytes())
                .filter(|mapping| mapping.start <= self.pc && self.pc < mapping.end)
                .map(|mapping| describe_location(&mapping, self.pc));
            self.line.clear();
            if hit.is_some() {
                return hit;
            }
        }
        None
    }
}

fn header_field(header: &str, key: &str) -> Option<u64> {
    header
        .split_whitespace()
        .find_map(|field| field.strip_prefix(key)?.strip_prefix('=')?.parse().ok())
}

fn describe_previous_exit(record: &str) -> Vec<String> {
    let mut lines = record.lines();
    let Some(header) = lines.next().and_then(|line| line.strip_prefix("run ")) else {
        return Vec::new();
    };
    let started = header_field(header, "started");
    let details: Vec<&str> = lines
        .filter(|line| line.starts_with("signal=") || line.starts_with("pc_in="))
        .collect();
    if details.is_empty() {
        return vec![format!(
            "[exit] previous run ({header}) did not quit cleanly and caught no signal: \
             SIGKILL (Force Quit, out of memory), power loss, or a native exit()"
        )];
    }
    details
        .into_iter()
        .map(|detail| {
            let uptime = started
                .zip(header_field(detail, "at"))
                .map(|(started, at)| format!(" uptime={}s", at.saturating_sub(started)))
                .unwrap_or_default();
            format!("[exit] previous run ({header}) ended by {detail}{uptime}")
        })
        .collect()
}

pub fn report_previous_and_arm(dir: &Path) {
    let path = dir.join(RECORD_FILE);
    if let Ok(record) = std::fs::read_to_string(&path) {
        for line in describe_previous_exit(&record) {
            tracing::error!("{line}");
        }
    }
    #[cfg(target_os = "linux")]
    if let Err(error) = signals::arm(&path) {
        tracing::warn!("exit record disabled ({}): {error}", path.display());
    }
}

#[cfg(target_os = "linux")]
pub fn mark_clean_exit() {
    signals::disarm();
}

#[cfg(target_os = "linux")]
mod signals {
    use super::{LineBuf, MapsScanner};
    use libc::{c_int, c_void, siginfo_t};
    use std::os::fd::IntoRawFd as _;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicI32, Ordering};

    const FAULT_SIGNALS: [c_int; 5] = [
        libc::SIGSEGV,
        libc::SIGBUS,
        libc::SIGILL,
        libc::SIGFPE,
        libc::SIGTRAP,
    ];
    const OTHER_SIGNALS: [c_int; 5] = [
        libc::SIGABRT,
        libc::SIGTERM,
        libc::SIGHUP,
        libc::SIGINT,
        libc::SIGQUIT,
    ];

    static RECORD_FD: AtomicI32 = AtomicI32::new(-1);
    static RECORD_PATH: OnceLock<PathBuf> = OnceLock::new();
    static PREVIOUS: OnceLock<Vec<(c_int, libc::sigaction)>> = OnceLock::new();

    fn unix_now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default()
    }

    pub(super) fn arm(path: &Path) -> std::io::Result<()> {
        if RECORD_PATH.get().is_some() {
            return Ok(());
        }
        std::fs::write(
            path,
            format!(
                "run pid={} version={} started={}\n",
                std::process::id(),
                env!("CARGO_PKG_VERSION"),
                unix_now()
            ),
        )?;
        let file = std::fs::OpenOptions::new().append(true).open(path)?;
        RECORD_FD.store(file.into_raw_fd(), Ordering::Release);
        let _ = RECORD_PATH.set(path.to_path_buf());
        install_handlers();
        Ok(())
    }

    pub(super) fn disarm() {
        let fd = RECORD_FD.swap(-1, Ordering::AcqRel);
        if fd >= 0 {
            unsafe { libc::close(fd) };
        }
        if let Some(path) = RECORD_PATH.get()
            && let Err(error) = std::fs::remove_file(path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("failed to clear exit record {}: {error}", path.display());
        }
    }

    fn install_handlers() {
        let mut previous = Vec::new();
        for signum in FAULT_SIGNALS.into_iter().chain(OTHER_SIGNALS) {
            let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
            if unsafe { libc::sigaction(signum, std::ptr::null(), &mut old) } != 0 {
                continue;
            }
            let is_termination = matches!(
                signum,
                libc::SIGTERM | libc::SIGHUP | libc::SIGINT | libc::SIGQUIT
            );
            if old.sa_sigaction == libc::SIG_IGN
                || (is_termination && old.sa_sigaction != libc::SIG_DFL)
            {
                continue;
            }
            previous.push((signum, old));
        }
        if PREVIOUS.set(previous).is_err() {
            return;
        }
        for (signum, _) in PREVIOUS.get().into_iter().flatten() {
            let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
            action.sa_sigaction = on_signal as *const () as libc::sighandler_t;
            action.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
            unsafe {
                libc::sigemptyset(&mut action.sa_mask);
                libc::sigaction(*signum, &action, std::ptr::null_mut());
            }
        }
    }

    extern "C" fn on_signal(signum: c_int, info: *mut siginfo_t, context: *mut c_void) {
        let fd = RECORD_FD.load(Ordering::Acquire);
        if fd >= 0 {
            unsafe { write_record(fd, signum, info, context) };
        }
        unsafe { chain_previous(signum, info, context) };
        reraise_with_default(signum);
    }

    #[inline(never)]
    unsafe fn write_record(fd: c_int, signum: c_int, info: *mut siginfo_t, context: *mut c_void) {
        let mut line = LineBuf::new();
        line.push_str("signal=");
        line.push_str(signal_name(signum));
        line.push_str("(");
        line.push_signed(i64::from(signum));
        line.push_str(")");
        let code = unsafe { info.as_ref() }.map_or(0, |info| info.si_code);
        line.push_str(" code=");
        line.push_signed(i64::from(code));
        let fault = FAULT_SIGNALS.contains(&signum) && code > 0;
        if let Some(info) = unsafe { info.as_ref() } {
            if fault {
                line.push_str(" addr=");
                line.push_hex(unsafe { info.si_addr() } as u64);
            } else if code <= 0 {
                line.push_str(" from_pid=");
                line.push_signed(i64::from(unsafe { info.si_pid() }));
            }
        }
        let pc = unsafe { program_counter(context) };
        if let Some(pc) = pc {
            line.push_str(" pc=");
            line.push_hex(pc);
        }
        line.push_str(" tid=");
        line.push_signed(unsafe { libc::syscall(libc::SYS_gettid) });
        line.push_str(" thread=");
        push_thread_name(&mut line);
        line.push_str(" at=");
        line.push_signed(unsafe { libc::time(std::ptr::null_mut()) });
        write_all(fd, line.finish_line());
        if fault
            && let Some(pc) = pc
            && let Some(mut location) = locate(pc)
        {
            write_all(fd, location.finish_line());
        }
    }

    fn locate(pc: u64) -> Option<LineBuf> {
        let maps = unsafe {
            libc::open(
                c"/proc/self/maps".as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC,
            )
        };
        if maps < 0 {
            return None;
        }
        let mut scanner = MapsScanner::new(pc);
        let mut chunk = [0u8; 256];
        let mut found = None;
        loop {
            let read = unsafe { libc::read(maps, chunk.as_mut_ptr().cast(), chunk.len()) };
            if read <= 0 {
                break;
            }
            found = scanner.feed(&chunk[..read as usize]);
            if found.is_some() {
                break;
            }
        }
        unsafe { libc::close(maps) };
        found
    }

    fn push_thread_name(line: &mut LineBuf) {
        let mut name = [0u8; 16];
        if unsafe { libc::prctl(libc::PR_GET_NAME, name.as_mut_ptr()) } == 0 {
            let len = name
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(name.len());
            line.push_bytes(&name[..len]);
        } else {
            line.push_str("?");
        }
    }

    fn write_all(fd: c_int, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
            if written > 0 {
                bytes = &bytes[written as usize..];
            } else if written < 0
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR)
            {
                continue;
            } else {
                return;
            }
        }
    }

    unsafe fn chain_previous(signum: c_int, info: *mut siginfo_t, context: *mut c_void) {
        let Some((_, previous)) = PREVIOUS
            .get()
            .and_then(|list| list.iter().find(|(candidate, _)| *candidate == signum))
        else {
            return;
        };
        let handler = previous.sa_sigaction;
        if handler == libc::SIG_DFL || handler == libc::SIG_IGN {
            return;
        }
        if previous.sa_flags & libc::SA_SIGINFO != 0 {
            let handler = unsafe {
                std::mem::transmute::<
                    libc::sighandler_t,
                    extern "C" fn(c_int, *mut siginfo_t, *mut c_void),
                >(handler)
            };
            handler(signum, info, context);
        } else {
            let handler =
                unsafe { std::mem::transmute::<libc::sighandler_t, extern "C" fn(c_int)>(handler) };
            handler(signum);
        }
    }

    fn reraise_with_default(signum: c_int) {
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = libc::SIG_DFL;
        unsafe {
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signum, &action, std::ptr::null_mut());
            libc::raise(signum);
        }
    }

    #[cfg(target_arch = "x86_64")]
    unsafe fn program_counter(context: *mut c_void) -> Option<u64> {
        let context = unsafe { context.cast::<libc::ucontext_t>().as_ref() }?;
        Some(context.uc_mcontext.gregs[libc::REG_RIP as usize] as u64)
    }

    #[cfg(target_arch = "aarch64")]
    unsafe fn program_counter(context: *mut c_void) -> Option<u64> {
        let context = unsafe { context.cast::<libc::ucontext_t>().as_ref() }?;
        Some(context.uc_mcontext.pc)
    }

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    unsafe fn program_counter(_context: *mut c_void) -> Option<u64> {
        None
    }

    fn signal_name(signum: c_int) -> &'static str {
        match signum {
            libc::SIGSEGV => "SIGSEGV",
            libc::SIGBUS => "SIGBUS",
            libc::SIGILL => "SIGILL",
            libc::SIGFPE => "SIGFPE",
            libc::SIGTRAP => "SIGTRAP",
            libc::SIGABRT => "SIGABRT",
            libc::SIGTERM => "SIGTERM",
            libc::SIGHUP => "SIGHUP",
            libc::SIGINT => "SIGINT",
            libc::SIGQUIT => "SIGQUIT",
            _ => "SIG?",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &mut LineBuf) -> String {
        String::from_utf8(line.finish_line().to_vec()).unwrap()
    }

    #[test]
    fn line_buf_formats_numbers_without_allocating() {
        let mut line = LineBuf::new();
        line.push_dec(0);
        line.push_str(" ");
        line.push_dec(u64::MAX);
        line.push_str(" ");
        line.push_signed(-42);
        line.push_str(" ");
        line.push_hex(0);
        line.push_str(" ");
        line.push_hex(0xdead_beef);
        assert_eq!(
            text(&mut line),
            "0 18446744073709551615 -42 0x0 0xdeadbeef\n"
        );
    }

    #[test]
    fn line_buf_truncates_but_keeps_the_newline() {
        let mut line = LineBuf::new();
        line.push_str(&"x".repeat(LINE_CAPACITY * 2));
        let finished = line.finish_line();
        assert_eq!(finished.len(), LINE_CAPACITY);
        assert_eq!(finished.last(), Some(&b'\n'));
    }

    const MAPS: &str = "\
55d4c0000000-55d4c1000000 r-xp 00000000 08:01 1000    /usr/bin/mezon
7f1200000000-7f1200100000 r-xp 00028000 08:01 2000    /usr/lib/x86_64-linux-gnu/libgstlibav.so
7f1300000000-7f1300010000 rw-p 00000000 00:00 0
";

    #[test]
    fn scanner_names_the_library_and_file_offset_of_the_pc() {
        let mut scanner = MapsScanner::new(0x7f12_0000_1234);
        let mut location = scanner.feed(MAPS.as_bytes()).expect("pc is mapped");
        assert_eq!(
            text(&mut location),
            "pc_in=/usr/lib/x86_64-linux-gnu/libgstlibav.so+0x29234\n"
        );
    }

    #[test]
    fn scanner_reassembles_lines_split_across_reads() {
        let mut scanner = MapsScanner::new(0x55d4_c000_0010);
        let (first, second) = MAPS.as_bytes().split_at(17);
        assert!(scanner.feed(first).is_none());
        let mut location = scanner.feed(second).expect("pc is mapped");
        assert_eq!(text(&mut location), "pc_in=/usr/bin/mezon+0x10\n");
    }

    #[test]
    fn scanner_reports_anonymous_memory_and_misses_unmapped_pcs() {
        let mut scanner = MapsScanner::new(0x7f13_0000_0008);
        let mut location = scanner.feed(MAPS.as_bytes()).expect("pc is mapped");
        assert_eq!(text(&mut location), "pc_in=[anonymous]+0x8\n");

        let mut scanner = MapsScanner::new(0x10);
        assert!(scanner.feed(MAPS.as_bytes()).is_none());
    }

    #[test]
    fn previous_exit_with_a_signal_is_reported_with_uptime() {
        let record = "run pid=7 version=2.1.21 started=1000\n\
                      signal=SIGSEGV(11) code=1 addr=0x0 pc=0x7f1 tid=9 thread=gst at=1185\n\
                      pc_in=/usr/lib/libgstlibav.so+0x29234\n";
        assert_eq!(
            describe_previous_exit(record),
            vec![
                "[exit] previous run (pid=7 version=2.1.21 started=1000) ended by \
                 signal=SIGSEGV(11) code=1 addr=0x0 pc=0x7f1 tid=9 thread=gst at=1185 uptime=185s"
                    .to_string(),
                "[exit] previous run (pid=7 version=2.1.21 started=1000) ended by \
                 pc_in=/usr/lib/libgstlibav.so+0x29234"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn previous_exit_without_a_signal_points_at_sigkill() {
        let lines = describe_previous_exit("run pid=7 version=2.1.21 started=1000\n");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("caught no signal"), "{}", lines[0]);
        assert!(lines[0].contains("pid=7"), "{}", lines[0]);
    }

    #[test]
    fn empty_or_foreign_record_reports_nothing() {
        assert!(describe_previous_exit("").is_empty());
        assert!(describe_previous_exit("garbage\nsignal=SIGSEGV(11)\n").is_empty());
    }

    #[cfg(target_os = "linux")]
    mod process {
        use super::super::{RECORD_FILE, report_previous_and_arm};
        use std::os::unix::process::ExitStatusExt as _;
        use std::path::{Path, PathBuf};
        use std::process::{Child, Command, Stdio};
        use std::time::{Duration, Instant};

        const CHILD_ENV: &str = "MEZON_EXIT_RECORD_CHILD";

        fn recurse_forever(depth: u64) -> u64 {
            let frame = std::hint::black_box([depth; 64]);
            if std::hint::black_box(depth) == u64::MAX {
                return frame[0];
            }
            recurse_forever(depth + 1).wrapping_add(frame[1])
        }

        #[test]
        fn crash_child() {
            let Ok(spec) = std::env::var(CHILD_ENV) else {
                return;
            };
            let (action, dir) = spec.split_once(':').unwrap();
            report_previous_and_arm(Path::new(dir));
            match action {
                "fault" => unsafe {
                    std::ptr::without_provenance_mut::<u8>(0x10).write_volatile(1);
                },
                "overflow" => {
                    std::hint::black_box(recurse_forever(0));
                }
                "wait" => std::thread::sleep(Duration::from_secs(30)),
                _ => {}
            }
        }

        fn scratch_dir(action: &str) -> PathBuf {
            let dir = std::env::temp_dir()
                .join(format!("mezon-exit-record-{}-{action}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        fn spawn_child(action: &str, dir: &Path) -> Child {
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "exit_record::tests::process::crash_child",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(CHILD_ENV, format!("{action}:{}", dir.display()))
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        }

        fn record(dir: &Path) -> String {
            std::fs::read_to_string(dir.join(RECORD_FILE)).unwrap_or_default()
        }

        #[test]
        fn real_fault_records_signal_thread_and_faulting_binary() {
            let dir = scratch_dir("fault");
            let output = spawn_child("fault", &dir).wait_with_output().unwrap();
            assert_eq!(output.status.signal(), Some(libc::SIGSEGV));
            let record = record(&dir);
            assert!(record.starts_with("run pid="), "{record}");
            assert!(
                record.contains("signal=SIGSEGV(11) code=1 addr=0x10 pc=0x"),
                "{record}"
            );
            assert!(record.contains(" thread="), "{record}");
            let location = record
                .lines()
                .find(|line| line.starts_with("pc_in="))
                .unwrap_or_else(|| panic!("no pc_in line: {record}"));
            let exe = std::env::current_exe().unwrap();
            let exe_name = exe.file_name().unwrap().to_str().unwrap();
            assert!(location.contains(exe_name), "{location}");
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn termination_records_the_sender_and_still_dies_by_that_signal() {
            let dir = scratch_dir("wait");
            let mut child = spawn_child("wait", &dir);
            let deadline = Instant::now() + Duration::from_secs(20);
            while !record(&dir).starts_with("run pid=") {
                assert!(Instant::now() < deadline, "child never armed");
                std::thread::sleep(Duration::from_millis(50));
            }
            std::thread::sleep(Duration::from_millis(200));
            unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
            let status = child.wait().unwrap();
            assert_eq!(status.signal(), Some(libc::SIGTERM));
            let record = record(&dir);
            let expected = format!("signal=SIGTERM(15) code=0 from_pid={}", std::process::id());
            assert!(record.contains(&expected), "{record}");
            let _ = std::fs::remove_dir_all(&dir);
        }

        #[test]
        fn stack_overflow_keeps_the_rust_report_and_records_both_signals() {
            let dir = scratch_dir("overflow");
            let output = spawn_child("overflow", &dir).wait_with_output().unwrap();
            assert_eq!(output.status.signal(), Some(libc::SIGABRT));
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("has overflowed its stack"), "{stderr}");
            let record = record(&dir);
            assert!(record.contains("signal=SIGSEGV(11)"), "{record}");
            assert!(record.contains("signal=SIGABRT(6)"), "{record}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

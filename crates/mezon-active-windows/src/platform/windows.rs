use crate::info::ActiveWindowInfo;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use windows::Win32::Foundation::{CloseHandle, MAX_PATH};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
};

pub fn get_active_window() -> anyhow::Result<ActiveWindowInfo> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return Err(anyhow::anyhow!("No active window"));
        }

        let title_len = GetWindowTextLengthW(hwnd);
        let window_name = if title_len > 0 {
            let mut title_buf = vec![0u16; title_len as usize + 1];
            let len = GetWindowTextW(hwnd, &mut title_buf);
            if len > 0 {
                String::from_utf16_lossy(&title_buf[..len as usize])
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));

        let mut window_class = String::new();
        if pid != 0 {
            if let Ok(handle) = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_QUERY_INFORMATION,
                false,
                pid,
            ) {
                let mut path_buf = [0u16; MAX_PATH as usize];
                let mut size = path_buf.len() as u32;
                let query_ok = QueryFullProcessImageNameW(
                    handle,
                    windows::Win32::System::Threading::PROCESS_NAME_FORMAT(0),
                    windows::core::PWSTR::from_raw(path_buf.as_mut_ptr()),
                    &mut size,
                )
                .is_ok();
                let _ = CloseHandle(handle);
                if query_ok {
                    let full_path = OsString::from_wide(&path_buf[..size as usize])
                        .to_string_lossy()
                        .into_owned();
                    window_class = std::path::Path::new(&full_path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or(&full_path)
                        .to_string();
                }
            }
        }

        let mut last_input = LASTINPUTINFO::default();
        last_input.cbSize = std::mem::size_of::<LASTINPUTINFO>() as u32;
        let mut idle_time = 0;
        if GetLastInputInfo(&mut last_input).as_bool() {
            let tick_count = windows::Win32::System::SystemInformation::GetTickCount();
            idle_time = (tick_count.saturating_sub(last_input.dwTime)) / 1000;
        }

        Ok(ActiveWindowInfo {
            os: "windows".to_string(),
            window_class,
            window_name,
            window_desktop: "0".to_string(),
            window_type: "0".to_string(),
            window_pid: pid.to_string(),
            idle_time: idle_time.to_string(),
        })
    }
}

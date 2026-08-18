use std::path::Path;

#[cfg(unix)]
pub fn free_bytes(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let raw = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(raw.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    Some(stat.f_bavail as u64 * stat.f_frsize as u64)
}

#[cfg(windows)]
pub fn free_bytes(path: &Path) -> Option<u64> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::core::HSTRING;

    let wide = HSTRING::from(path.as_os_str());
    let mut available: u64 = 0;
    unsafe { GetDiskFreeSpaceExW(&wide, Some(&mut available), None, None) }.ok()?;
    Some(available)
}

#[cfg(not(any(unix, windows)))]
pub fn free_bytes(_path: &Path) -> Option<u64> {
    None
}

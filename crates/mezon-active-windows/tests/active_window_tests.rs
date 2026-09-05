use mezon_active_windows::get_active_window;

#[test]
fn test_get_active_window() {
    // We expect the query to either succeed (if running in an active GUI session)
    // or return a handled/logged error (if the compositor/X11 connection fails),
    // but the function itself must not crash/panic.
    match get_active_window() {
        Ok(info) => {
            println!("Active Window Info: {:#?}", info);
            assert_eq!(
                info.os,
                if cfg!(target_os = "windows") {
                    "windows"
                } else if cfg!(target_os = "macos") {
                    "macos"
                } else {
                    "linux"
                }
            );
        }
        Err(e) => {
            println!("Handled Active Window Query Error: {}", e);
        }
    }
}

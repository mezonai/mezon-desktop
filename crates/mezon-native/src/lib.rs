pub mod audio;
pub mod autostart;
pub mod badge;
pub mod browser;
pub mod cli_install;
pub mod control;
pub mod deep_link;
pub mod instance;
pub mod location;
pub mod notifications;
pub mod power;
pub mod tray;
pub mod window_icon;

pub(crate) fn ensure_http_url(url: &str) -> anyhow::Result<()> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err(anyhow::anyhow!(
            "url rejected: only http(s) scheme is allowed"
        ));
    }
    Ok(())
}

/// Opens a URL in the system default browser.
///
/// Only `http://` and `https://` URLs are accepted; other schemes are rejected.
pub fn open_url(url: &str) -> anyhow::Result<()> {
    ensure_http_url(url)?;
    #[cfg(unix)]
    {
        open_url_reaped(url)
    }
    #[cfg(not(unix))]
    {
        open::that_detached(url).map_err(|e| anyhow::anyhow!("Failed to open URL: {}", e))
    }
}

#[cfg(unix)]
fn open_url_reaped(url: &str) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt as _;
    use std::process::Stdio;

    let mut last_err: Option<std::io::Error> = None;
    for mut command in open::commands(url) {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            command.pre_exec(|| {
                match libc::fork() {
                    -1 => return Err(std::io::Error::last_os_error()),
                    0 => (),
                    _ => libc::_exit(0),
                }
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        match command.spawn() {
            Ok(mut child) => {
                let _ = child.wait();
                return Ok(());
            }
            Err(err) => last_err = Some(err),
        }
    }
    Err(anyhow::anyhow!(
        "Failed to open URL: {}",
        last_err
            .map(|err| err.to_string())
            .unwrap_or_else(|| "no launcher available".to_string())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_url_rejects_non_http_schemes() {
        assert!(open_url("ftp://mezon.ai/").is_err());
        assert!(open_url("file:///etc/passwd").is_err());
        assert!(open_url("javascript:alert(1)").is_err());
        assert!(open_url("data:text/html,<h1>hi</h1>").is_err());
        assert!(open_url("").is_err());
    }
}

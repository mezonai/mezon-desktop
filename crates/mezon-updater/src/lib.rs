pub mod winstore;

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha512};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

pub struct UpdaterEndpoints {
    pub manifest_base_url: String,
    pub download_url: String,
}

static ENDPOINTS: OnceLock<UpdaterEndpoints> = OnceLock::new();

pub fn configure_endpoints(endpoints: UpdaterEndpoints) {
    let _ = ENDPOINTS.set(endpoints);
}

fn endpoints() -> anyhow::Result<&'static UpdaterEndpoints> {
    ENDPOINTS
        .get()
        .ok_or_else(|| anyhow::anyhow!("updater endpoints not configured"))
}

pub fn download_url() -> anyhow::Result<&'static str> {
    Ok(endpoints()?.download_url.as_str())
}

const DOWNLOAD_STALL_TIMEOUT: Duration = Duration::from_secs(60);

fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

fn download_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
}

fn allowed_download_hosts() -> anyhow::Result<Vec<String>> {
    let endpoints = endpoints()?;
    Ok([
        host_of(&endpoints.manifest_base_url),
        host_of(&endpoints.download_url),
    ]
    .into_iter()
    .flatten()
    .collect())
}

pub struct UpdateManifest {
    pub version: String,
    pub sha512: String,
    pub path: String,
    pub deb_path: Option<String>,
    pub deb_sha512: Option<String>,
}

impl UpdateManifest {
    pub fn deb_artifact(&self) -> Option<(&str, &str)> {
        match (self.deb_path.as_deref(), self.deb_sha512.as_deref()) {
            (Some(path), Some(sha512)) => Some((path, sha512)),
            _ => None,
        }
    }
}

#[cfg_attr(any(target_os = "macos", target_os = "windows"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinuxInstallPlan {
    ReplaceBinary,
    InstallDeb,
    Unsupported,
}

#[cfg_attr(any(target_os = "macos", target_os = "windows"), allow(dead_code))]
fn linux_install_plan(exe_dir_writable: bool, deb_available: bool) -> LinuxInstallPlan {
    if exe_dir_writable {
        LinuxInstallPlan::ReplaceBinary
    } else if deb_available {
        LinuxInstallPlan::InstallDeb
    } else {
        LinuxInstallPlan::Unsupported
    }
}

pub struct DownloadedUpdate {
    _dir: tempfile::TempDir,
    staging: PathBuf,
    archive: PathBuf,
    #[allow(dead_code)]
    is_deb: bool,
}

pub struct InstallOutcome {
    pub restart_path: Option<PathBuf>,
    pub manual_deb: Option<PathBuf>,
}

fn insecure_loopback_allowed(parsed: &url::Url) -> bool {
    if !cfg!(debug_assertions) {
        return false;
    }
    if std::env::var("MEZON_ALLOW_INSECURE_UPDATE_URL").is_err() {
        return false;
    }
    matches!(parsed.host_str(), Some("127.0.0.1") | Some("localhost"))
}

fn validate_url_against(url: &str, allowed_hosts: &[String]) -> Result<()> {
    let parsed = url::Url::parse(url).map_err(|e| anyhow!("invalid URL: {e}"))?;
    if parsed.scheme() != "https" {
        if parsed.scheme() == "http" && insecure_loopback_allowed(&parsed) {
            return Ok(());
        }
        bail!("rejected update URL: scheme must be https");
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow!("rejected update URL: no host"))?;
    if !allowed_hosts.iter().any(|allowed| allowed == host) {
        bail!("rejected update URL: host not in allowlist");
    }
    Ok(())
}

pub fn validate_update_url(url: &str) -> Result<()> {
    validate_url_against(url, &allowed_download_hosts()?)
}

fn validate_url_with_base(url: &str, base_url: &str) -> Result<()> {
    let mut hosts = allowed_download_hosts()?;
    hosts.extend(host_of(base_url));
    validate_url_against(url, &hosts)
}

fn join_url(base_url: &str, file: &str) -> Result<String> {
    let mut base = base_url.to_string();
    if !base.ends_with('/') {
        base.push('/');
    }
    let joined = url::Url::parse(&base)
        .map_err(|e| anyhow!("invalid update base URL: {e}"))?
        .join(file)
        .map_err(|e| anyhow!("invalid update file name: {e}"))?;
    Ok(joined.to_string())
}

pub fn verify_file_checksum(file_bytes: &[u8], expected_sha512_b64: &str) -> Result<()> {
    let mut hasher = Sha512::new();
    hasher.update(file_bytes);
    verify_digest(hasher, expected_sha512_b64)
}

fn verify_digest(hasher: Sha512, expected_sha512_b64: &str) -> Result<()> {
    let digest = hasher.finalize();
    let actual_b64 = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        digest.as_slice(),
    );
    if actual_b64 != expected_sha512_b64 {
        bail!("checksum mismatch: expected {expected_sha512_b64}, got {actual_b64}");
    }
    Ok(())
}

pub fn manifest_filename() -> String {
    let arch = std::env::consts::ARCH;
    if cfg!(target_os = "macos") {
        "latest-native-mac.yml".to_string()
    } else if cfg!(target_os = "windows") {
        format!("latest-native-windows-{arch}.yml")
    } else {
        format!("latest-native-linux-{arch}.yml")
    }
}

fn parse_field<'a>(body: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}:");
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(prefix.as_str()) {
            let v = rest.trim().trim_matches('\'').trim_matches('"');
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

fn parse_version_from_manifest(body: &str) -> Result<semver::Version> {
    let v = parse_field(body, "version")
        .ok_or_else(|| anyhow!("version field not found in update manifest"))?;
    semver::Version::parse(v).map_err(|e| anyhow!("invalid semver in manifest: {e}"))
}

fn parse_manifest(body: &str) -> Result<UpdateManifest> {
    let version = parse_version_from_manifest(body)?.to_string();
    let sha512 = parse_field(body, "sha512")
        .ok_or_else(|| anyhow!("sha512 field not found in update manifest"))?
        .to_string();
    let path = parse_field(body, "path")
        .ok_or_else(|| anyhow!("path field not found in update manifest"))?
        .to_string();
    Ok(UpdateManifest {
        version,
        sha512,
        path,
        deb_path: parse_field(body, "deb").map(str::to_string),
        deb_sha512: parse_field(body, "debSha512").map(str::to_string),
    })
}

pub async fn check_for_updates(base_url: &str, current_version: &str) -> Result<Option<String>> {
    match check_for_updates_with_manifest(base_url, current_version).await? {
        Some(m) => Ok(Some(m.version)),
        None => Ok(None),
    }
}

pub async fn check_store_feed(feed_url: &str, current_version: &str) -> Result<Option<String>> {
    let current = semver::Version::parse(current_version)
        .map_err(|e| anyhow!("invalid current version '{current_version}': {e}"))?;
    validate_url_with_base(feed_url, feed_url)?;

    let response = http_client()
        .get(feed_url)
        .send()
        .await
        .map_err(|e| anyhow!("store update feed fetch failed: {e}"))?;
    if !response.status().is_success() {
        bail!("store update feed returned HTTP {}", response.status());
    }
    let body = response
        .text()
        .await
        .map_err(|e| anyhow!("failed to read store update feed body: {e}"))?;

    let latest = parse_version_from_manifest(&body)?;
    if latest > current {
        tracing::info!("store update available: {} -> {}", current, latest);
        Ok(Some(latest.to_string()))
    } else {
        Ok(None)
    }
}

pub async fn check_for_updates_with_manifest(
    base_url: &str,
    current_version: &str,
) -> Result<Option<UpdateManifest>> {
    let current = semver::Version::parse(current_version)
        .map_err(|e| anyhow!("invalid current version '{current_version}': {e}"))?;

    let manifest_url = join_url(base_url, &manifest_filename())?;
    validate_url_with_base(&manifest_url, base_url)?;

    tracing::debug!("fetching update manifest {}", manifest_filename());

    let response = http_client()
        .get(&manifest_url)
        .send()
        .await
        .map_err(|e| anyhow!("update manifest fetch failed: {e}"))?;

    if !response.status().is_success() {
        bail!("update manifest returned HTTP {}", response.status());
    }

    let body = response
        .text()
        .await
        .map_err(|e| anyhow!("failed to read update manifest body: {e}"))?;

    let manifest = parse_manifest(&body)?;
    let latest = semver::Version::parse(&manifest.version)
        .map_err(|e| anyhow!("invalid semver in manifest: {e}"))?;

    if latest > current {
        tracing::info!("update available: {} -> {}", current, latest);
        Ok(Some(manifest))
    } else {
        tracing::debug!("already up to date ({})", current);
        Ok(None)
    }
}

fn expected_archive_extension() -> &'static str {
    if cfg!(target_os = "macos") {
        ".dmg"
    } else if cfg!(target_os = "windows") {
        ".zip"
    } else {
        ".tar.gz"
    }
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn exe_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("failed to locate running executable")?;
    Ok(exe
        .parent()
        .ok_or_else(|| anyhow!("running executable has no parent directory"))?
        .to_path_buf())
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn exe_dir_writable() -> bool {
    let Ok(dir) = exe_dir() else { return false };
    let probe = dir.join(format!(".mezon-write-probe-{}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

pub fn needs_privileged_install() -> bool {
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        !exe_dir_writable()
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        false
    }
}

fn select_artifact(manifest: &UpdateManifest) -> Result<(&str, &str, &'static str, bool)> {
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        match linux_install_plan(exe_dir_writable(), manifest.deb_artifact().is_some()) {
            LinuxInstallPlan::InstallDeb => {
                let (path, sha512) = manifest
                    .deb_artifact()
                    .ok_or_else(|| anyhow!("update manifest has no .deb artifact"))?;
                return Ok((path, sha512, ".deb", true));
            }
            LinuxInstallPlan::Unsupported => {
                let dir = exe_dir()
                    .map(|d| d.display().to_string())
                    .unwrap_or_else(|_| "the install directory".to_string());
                bail!(
                    "cannot write to {dir} and the update feed has no .deb; if Mezon was installed with a package manager, update it with the package manager instead"
                );
            }
            LinuxInstallPlan::ReplaceBinary => {}
        }
    }
    Ok((
        &manifest.path,
        &manifest.sha512,
        expected_archive_extension(),
        false,
    ))
}

pub async fn download_update(
    base_url: &str,
    manifest: &UpdateManifest,
    on_progress: impl Fn(u64, Option<u64>) + Send,
) -> Result<DownloadedUpdate> {
    let (artifact_path, artifact_sha512, expected_ext, is_deb) = select_artifact(manifest)?;
    let file_name = Path::new(artifact_path)
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("invalid artifact path in manifest"))?
        .to_string();
    if !file_name.ends_with(expected_ext) {
        bail!("unexpected artifact '{file_name}' for this platform (want {expected_ext})");
    }

    let artifact_url = join_url(base_url, artifact_path)?;
    validate_url_with_base(&artifact_url, base_url)?;

    let dir = tempfile::Builder::new()
        .prefix("mezon-auto-update")
        .tempdir()
        .context("failed to create update temp dir")?;
    let staging = dir.path().join("staging");
    tokio::fs::create_dir_all(&staging)
        .await
        .context("failed to create update staging dir")?;
    let archive = dir.path().join(&file_name);

    let mut response = download_client()
        .get(&artifact_url)
        .send()
        .await
        .map_err(|e| anyhow!("update download failed: {e}"))?;
    if !response.status().is_success() {
        bail!("update download returned HTTP {}", response.status());
    }

    let total = response.content_length().filter(|t| *t > 0);
    let mut file = tokio::fs::File::create(&archive)
        .await
        .context("failed to create update archive file")?;
    let mut hasher = Sha512::new();
    let mut written: u64 = 0;
    let mut reported: u64 = 0;
    on_progress(0, total);
    loop {
        let chunk = tokio::time::timeout(DOWNLOAD_STALL_TIMEOUT, response.chunk())
            .await
            .map_err(|_| {
                anyhow!(
                    "update download stalled (no data for {}s)",
                    DOWNLOAD_STALL_TIMEOUT.as_secs()
                )
            })?
            .map_err(|e| anyhow!("update download failed: {e}"))?;
        let Some(chunk) = chunk else { break };
        hasher.update(&chunk);
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .context("failed to write update archive")?;
        written += chunk.len() as u64;
        let step = match total {
            Some(t) => written * 100 / t != reported * 100 / t,
            None => written - reported >= 1024 * 1024,
        };
        if step {
            reported = written;
            on_progress(written, total);
        }
    }
    tokio::io::AsyncWriteExt::flush(&mut file)
        .await
        .context("failed to flush update archive")?;
    drop(file);
    on_progress(written, total.or(Some(written)));

    verify_digest(hasher, artifact_sha512)?;
    tracing::info!("downloaded update archive {file_name} ({written} bytes)");

    Ok(DownloadedUpdate {
        _dir: dir,
        staging,
        archive,
        is_deb,
    })
}

pub async fn install_update(
    update: &DownloadedUpdate,
    running_app_path: Option<&Path>,
) -> Result<InstallOutcome> {
    #[cfg(target_os = "macos")]
    {
        install_macos(update, running_app_path).await
    }
    #[cfg(target_os = "windows")]
    {
        let _ = running_app_path;
        install_windows(update).await
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        let _ = running_app_path;
        install_linux(update).await
    }
}

async fn run_command(program: &str, args: &[&std::ffi::OsStr]) -> Result<()> {
    let mut command = tokio::process::Command::new(program);
    command.args(args);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = command
        .output()
        .await
        .with_context(|| format!("failed to run {program}"))?;
    if !output.status.success() {
        bail!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
async fn find_in_dir(dir: &Path, target_name: &str, depth: usize) -> Result<Option<PathBuf>> {
    let mut queue = vec![(dir.to_path_buf(), 0usize)];
    while let Some((current, level)) = queue.pop() {
        let mut entries = tokio::fs::read_dir(&current)
            .await
            .with_context(|| format!("failed to read {}", current.display()))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .context("failed to read dir entry")?
        {
            let path = entry.path();
            let file_type = entry.file_type().await.context("failed to stat entry")?;
            if file_type.is_file() {
                if path.file_name().and_then(|n| n.to_str()) == Some(target_name) {
                    return Ok(Some(path));
                }
            } else if file_type.is_dir() && level < depth {
                queue.push((path, level + 1));
            }
        }
    }
    Ok(None)
}

#[cfg(target_os = "macos")]
async fn install_macos(
    update: &DownloadedUpdate,
    running_app_path: Option<&Path>,
) -> Result<InstallOutcome> {
    use std::ffi::OsString;

    let app_path = running_app_path
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("app"))
        .ok_or_else(|| {
            anyhow!("auto-update requires Mezon to run from an installed .app bundle")
        })?;
    which::which("rsync").context("rsync not found; it is required for auto-update")?;

    let mount_root = update.staging.join("mnt");
    tokio::fs::create_dir_all(&mount_root)
        .await
        .context("failed to create mount dir")?;
    run_command(
        "hdiutil",
        &[
            "attach".as_ref(),
            "-nobrowse".as_ref(),
            update.archive.as_os_str(),
            "-mountroot".as_ref(),
            mount_root.as_os_str(),
        ],
    )
    .await?;

    let install = async {
        let volume = find_first_dir(&mount_root)
            .await?
            .ok_or_else(|| anyhow!("no volume found in mounted update image"))?;
        let app_src = find_app_bundle(&volume)
            .await?
            .ok_or_else(|| anyhow!("no .app bundle found in update image"))?;
        let mut app_src_contents: OsString = app_src.into();
        app_src_contents.push("/");
        run_command(
            "rsync",
            &[
                "-a".as_ref(),
                "--delete".as_ref(),
                "--exclude".as_ref(),
                "Icon?".as_ref(),
                app_src_contents.as_os_str(),
                app_path.as_os_str(),
            ],
        )
        .await?;
        anyhow::Ok(volume)
    }
    .await;

    match install {
        Ok(volume) => {
            run_command(
                "hdiutil",
                &["detach".as_ref(), "-force".as_ref(), volume.as_os_str()],
            )
            .await
            .unwrap_or_else(|e| tracing::warn!("failed to detach update image: {e}"));
            Ok(InstallOutcome {
                restart_path: None,
                manual_deb: None,
            })
        }
        Err(error) => {
            if let Ok(Some(volume)) = find_first_dir(&mount_root).await {
                let _ = run_command(
                    "hdiutil",
                    &["detach".as_ref(), "-force".as_ref(), volume.as_os_str()],
                )
                .await;
            }
            Err(error)
        }
    }
}

#[cfg(target_os = "macos")]
async fn find_first_dir(root: &Path) -> Result<Option<PathBuf>> {
    let mut entries = tokio::fs::read_dir(root)
        .await
        .context("failed to read mount root")?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .context("failed to read mount entry")?
    {
        if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            return Ok(Some(entry.path()));
        }
    }
    Ok(None)
}

#[cfg(target_os = "macos")]
async fn find_app_bundle(volume: &Path) -> Result<Option<PathBuf>> {
    let mut entries = tokio::fs::read_dir(volume)
        .await
        .context("failed to read update volume")?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .context("failed to read volume entry")?
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("app") {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
const PRIVILEGED_INSTALL_TIMEOUT: Duration = Duration::from_secs(300);

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
async fn install_linux_deb(update: &DownloadedUpdate) -> Result<InstallOutcome> {
    use std::process::Stdio;

    let current_exe = std::env::current_exe().context("failed to locate running executable")?;

    let child = match tokio::process::Command::new("pkexec")
        .arg("/usr/bin/dpkg")
        .arg("-i")
        .arg(&update.archive)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tracing::warn!("pkexec not available for privileged .deb install: {error}");
            return install_deb_fallback(update).await;
        }
        Err(error) => return Err(anyhow!("failed to run pkexec: {error}")),
    };

    let output =
        match tokio::time::timeout(PRIVILEGED_INSTALL_TIMEOUT, child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => return Err(anyhow!("failed to run pkexec: {error}")),
            Err(_) => {
                tracing::warn!(
                    "privileged .deb install timed out after {}s",
                    PRIVILEGED_INSTALL_TIMEOUT.as_secs()
                );
                return install_deb_fallback(update).await;
            }
        };

    if output.status.success() {
        return Ok(InstallOutcome {
            restart_path: Some(current_exe),
            manual_deb: None,
        });
    }

    let code = output.status.code();
    let stderr_raw = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr_raw.trim();
    if stderr.is_empty() {
        tracing::warn!("privileged .deb install failed (pkexec exit {code:?})");
    } else {
        tracing::warn!("privileged .deb install failed (pkexec exit {code:?}): {stderr}");
    }

    if !matches!(code, Some(126) | Some(127)) {
        return Err(anyhow!(
            "dpkg failed to install the update{}",
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {stderr}")
            }
        ));
    }

    install_deb_fallback(update).await
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
async fn install_deb_fallback(update: &DownloadedUpdate) -> Result<InstallOutcome> {
    let deb = persist_deb_for_manual_install(&update.archive)
        .await
        .unwrap_or_else(|| update.archive.clone());
    Ok(InstallOutcome {
        restart_path: None,
        manual_deb: Some(deb),
    })
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn update_cache_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        let xdg = PathBuf::from(xdg);
        if xdg.is_absolute() {
            return Some(xdg.join("mezon").join("updates"));
        }
    }
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".cache")
            .join("mezon")
            .join("updates"),
    )
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
async fn persist_deb_for_manual_install(archive: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let file_name = archive.file_name()?;
    let dir = update_cache_dir()?;
    if let Err(error) = tokio::fs::create_dir_all(&dir).await {
        tracing::warn!(
            "failed to create update cache dir {}: {error}",
            dir.display()
        );
        return None;
    }
    let dest = dir.join(file_name);
    if let Err(error) = tokio::fs::copy(archive, &dest).await {
        tracing::warn!("failed to preserve update package for manual install: {error}");
        return None;
    }
    let _ = tokio::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o644)).await;
    Some(dest)
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
async fn install_linux(update: &DownloadedUpdate) -> Result<InstallOutcome> {
    if update.is_deb {
        return install_linux_deb(update).await;
    }

    let current_exe = std::env::current_exe().context("failed to locate running executable")?;
    let exe_dir = current_exe
        .parent()
        .ok_or_else(|| anyhow!("running executable has no parent directory"))?
        .to_path_buf();

    let extract_dir = update.staging.join("extract");
    tokio::fs::create_dir_all(&extract_dir)
        .await
        .context("failed to create extract dir")?;
    run_command(
        "tar",
        &[
            "-xzf".as_ref(),
            update.archive.as_os_str(),
            "-C".as_ref(),
            extract_dir.as_os_str(),
        ],
    )
    .await?;

    let new_binary = find_in_dir(&extract_dir, "mezon", 2)
        .await?
        .ok_or_else(|| anyhow!("update archive does not contain a 'mezon' binary"))?;

    let staged = exe_dir.join(".mezon-update-staged");
    stage_binary(&new_binary, &staged).await.map_err(|e| {
        anyhow!(
            "cannot write to {} ({e}); if Mezon was installed with a package manager, update it with the package manager instead",
            exe_dir.display()
        )
    })?;

    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
        .await
        .context("failed to mark new binary executable")?;
    tokio::fs::rename(&staged, &current_exe)
        .await
        .with_context(|| format!("failed to replace {}", current_exe.display()))?;

    Ok(InstallOutcome {
        restart_path: Some(current_exe),
        manual_deb: None,
    })
}

#[cfg(not(target_os = "macos"))]
async fn stage_binary(source: &Path, staged: &Path) -> Result<()> {
    let _ = tokio::fs::remove_file(staged).await;
    tokio::fs::copy(source, staged)
        .await
        .map_err(anyhow::Error::from)?;
    Ok(())
}

#[cfg(target_os = "windows")]
async fn install_windows(update: &DownloadedUpdate) -> Result<InstallOutcome> {
    let current_exe = std::env::current_exe().context("failed to locate running executable")?;
    let exe_dir = current_exe
        .parent()
        .ok_or_else(|| anyhow!("running executable has no parent directory"))?
        .to_path_buf();

    let extract_dir = update.staging.join("extract");
    tokio::fs::create_dir_all(&extract_dir)
        .await
        .context("failed to create extract dir")?;
    run_command(
        "tar",
        &[
            "-xf".as_ref(),
            update.archive.as_os_str(),
            "-C".as_ref(),
            extract_dir.as_os_str(),
        ],
    )
    .await?;

    let new_binary = find_in_dir(&extract_dir, "mezon.exe", 2)
        .await?
        .ok_or_else(|| anyhow!("update archive does not contain mezon.exe"))?;

    let staged = exe_dir.join("mezon-update-staged.exe");
    stage_binary(&new_binary, &staged).await.map_err(|e| {
        anyhow!(
            "cannot write to {} ({e}); move Mezon to a user-writable folder to enable auto-update",
            exe_dir.display()
        )
    })?;

    let retired = exe_dir.join(format!("mezon-old-{}.exe", std::process::id()));
    let _ = tokio::fs::remove_file(&retired).await;
    tokio::fs::rename(&current_exe, &retired)
        .await
        .context("failed to move the running executable aside")?;
    if let Err(error) = tokio::fs::rename(&staged, &current_exe).await {
        let _ = tokio::fs::rename(&retired, &current_exe).await;
        return Err(anyhow!(error).context("failed to move new binary into place"));
    }

    Ok(InstallOutcome {
        restart_path: Some(current_exe),
        manual_deb: None,
    })
}

pub fn cleanup_stale_update_artifacts() {
    #[cfg(target_os = "windows")]
    {
        let Ok(current_exe) = std::env::current_exe() else {
            return;
        };
        let Some(exe_dir) = current_exe.parent() else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(exe_dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let stale = (name.starts_with("mezon-old-") && name.ends_with(".exe"))
                || name == "mezon-update-staged.exe";
            if stale && std::fs::remove_file(entry.path()).is_ok() {
                tracing::debug!("removed stale update artifact {name}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts() -> Vec<String> {
        vec!["site.example".to_string(), "cdn.example".to_string()]
    }

    #[test]
    fn validate_accepts_configured_site_host() {
        assert!(validate_url_against("https://site.example/download", &hosts()).is_ok());
    }

    #[test]
    fn validate_accepts_configured_cdn_host() {
        assert!(
            validate_url_against("https://cdn.example/release/1.0.0/mezon.dmg", &hosts()).is_ok()
        );
    }

    #[test]
    fn validate_rejects_http_scheme() {
        assert!(validate_url_against("http://site.example/download", &hosts()).is_err());
    }

    #[test]
    fn validate_rejects_unknown_host() {
        assert!(validate_url_against("https://evil.com/mezon.dmg", &hosts()).is_err());
    }

    #[test]
    fn validate_rejects_subdomain_bypass() {
        assert!(validate_url_against("https://site.example.evil.com/download", &hosts()).is_err());
    }

    #[test]
    fn validate_rejects_file_scheme() {
        assert!(validate_url_against("file:///tmp/malware", &hosts()).is_err());
    }

    #[test]
    fn validate_rejects_no_host() {
        assert!(validate_url_against("https:///no-host", &hosts()).is_err());
    }

    #[test]
    fn validate_rejects_malformed_url() {
        assert!(validate_url_against("not a url", &hosts()).is_err());
    }

    #[test]
    fn validate_rejects_everything_when_unconfigured() {
        assert!(validate_url_against("https://site.example/download", &[]).is_err());
    }

    #[test]
    fn allowed_hosts_derive_from_endpoint_urls() {
        assert_eq!(
            host_of("https://cdn.example/release/").as_deref(),
            Some("cdn.example")
        );
    }

    #[test]
    fn parse_version_bare() {
        let manifest = "version: 1.2.3\npath: mezon.dmg\nsha512: abc=\n";
        let v = parse_version_from_manifest(manifest).unwrap();
        assert_eq!(v, semver::Version::new(1, 2, 3));
    }

    #[test]
    fn parse_version_quoted_single() {
        let manifest = "version: '1.4.0'\npath: mezon.dmg\nsha512: abc=\n";
        let v = parse_version_from_manifest(manifest).unwrap();
        assert_eq!(v, semver::Version::new(1, 4, 0));
    }

    #[test]
    fn parse_version_quoted_double() {
        let manifest = "version: \"2.0.1\"\npath: mezon.dmg\nsha512: abc=\n";
        let v = parse_version_from_manifest(manifest).unwrap();
        assert_eq!(v, semver::Version::new(2, 0, 1));
    }

    #[test]
    fn parse_version_missing_returns_err() {
        let manifest = "files:\n  - url: mezon.dmg\n";
        assert!(parse_version_from_manifest(manifest).is_err());
    }

    #[test]
    fn parse_version_invalid_semver_returns_err() {
        let manifest = "version: not-a-version\n";
        assert!(parse_version_from_manifest(manifest).is_err());
    }

    #[test]
    fn verify_file_checksum_accepts_correct_hash() {
        let data = b"hello mezon update";
        let mut hasher = Sha512::new();
        hasher.update(data);
        let digest = hasher.finalize();
        let expected = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            digest.as_slice(),
        );
        assert!(verify_file_checksum(data, &expected).is_ok());
    }

    #[test]
    fn verify_file_checksum_rejects_wrong_hash() {
        let data = b"hello mezon update";
        assert!(verify_file_checksum(data, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").is_err());
    }

    #[test]
    fn verify_file_checksum_rejects_empty_expected() {
        assert!(verify_file_checksum(b"data", "").is_err());
    }

    #[test]
    fn parse_manifest_extracts_all_fields() {
        let body = "version: 1.5.0\npath: mezon-1.5.0-mac.dmg\nsha512: abc123=\nreleaseDate: '2024-01-01'\n";
        let m = parse_manifest(body).unwrap();
        assert_eq!(m.version, "1.5.0");
        assert_eq!(m.path, "mezon-1.5.0-mac.dmg");
        assert_eq!(m.sha512, "abc123=");
    }

    #[test]
    fn parse_manifest_missing_sha512_returns_err() {
        let body = "version: 1.5.0\npath: mezon.dmg\n";
        assert!(parse_manifest(body).is_err());
    }

    #[test]
    fn parse_manifest_missing_path_returns_err() {
        let body = "version: 1.5.0\nsha512: abc=\n";
        assert!(parse_manifest(body).is_err());
    }
}

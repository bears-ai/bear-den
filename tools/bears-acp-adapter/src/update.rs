use anyhow::{anyhow, bail, Context, Result};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

const DEFAULT_UPDATE_BASE_URL: &str = "https://theartificial.github.io/BEARS/bears-acp-adapter";
const MACOS_PACKAGE_IDENTIFIER: &str = "ai.bears.acp-adapter";

#[derive(Clone, Debug)]
pub enum UpdateCommand {
    Check(UpdateOptions),
    Update(UpdateOptions),
}

#[derive(Clone, Debug)]
pub struct UpdateOptions {
    pub channel: String,
    pub manifest_url: Option<String>,
    pub yes: bool,
    pub install_mode: UpdateInstallMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateInstallMode {
    OpenInstaller,
    InstallWithSudo,
    DownloadOnly,
}

impl Default for UpdateOptions {
    fn default() -> Self {
        Self {
            channel: env::var("BEARS_ACP_UPDATE_CHANNEL")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "stable".to_string()),
            manifest_url: env::var("BEARS_ACP_UPDATE_MANIFEST_URL")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            yes: false,
            install_mode: UpdateInstallMode::OpenInstaller,
        }
    }
}

impl UpdateOptions {
    pub fn from_args(mut args: impl Iterator<Item = String>) -> Result<Self> {
        let mut options = Self::default();
        let mut install_mode_explicit = false;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--channel" => options.channel = require_arg_value("--channel", args.next())?,
                "--manifest-url" => {
                    options.manifest_url = Some(require_arg_value("--manifest-url", args.next())?)
                }
                "--yes" | "-y" => {
                    options.yes = true;
                    if !install_mode_explicit {
                        options.install_mode = UpdateInstallMode::InstallWithSudo;
                    }
                }
                "--open" => {
                    options.install_mode = UpdateInstallMode::OpenInstaller;
                    install_mode_explicit = true;
                }
                "--install" | "--cli" => {
                    options.install_mode = UpdateInstallMode::InstallWithSudo;
                    install_mode_explicit = true;
                }
                "--download-only" => {
                    options.install_mode = UpdateInstallMode::DownloadOnly;
                    install_mode_explicit = true;
                }
                "--help" | "-h" => {
                    print_update_help_to_stderr();
                    std::process::exit(0);
                }
                unknown => {
                    bail!(
                        "unknown update argument {unknown:?}; use bears-acp-adapter update --help"
                    )
                }
            }
        }
        options.channel = normalize_update_channel(&options.channel)?;
        Ok(options)
    }
}

#[derive(Debug, Deserialize)]
struct UpdateManifest {
    channel: Option<String>,
    version: String,
    platforms: HashMap<String, UpdatePlatform>,
    release_notes_url: Option<String>,
    mandatory: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
struct UpdatePlatform {
    pkg_url: Option<String>,
    binary_url: Option<String>,
    sha256: String,
    min_macos: Option<String>,
    size: Option<u64>,
    package_identifier: Option<String>,
}

#[derive(Debug)]
struct UpdateStatus {
    manifest_url: String,
    target: String,
    manifest: UpdateManifest,
    platform: Option<UpdatePlatform>,
    update_available: bool,
}

pub async fn run_update_command(http: &reqwest::Client, command: UpdateCommand) -> Result<()> {
    match command {
        UpdateCommand::Check(options) => {
            let status = fetch_update_status(http, &options).await?;
            print_update_status(&status);
            Ok(())
        }
        UpdateCommand::Update(options) => run_update(http, &options).await,
    }
}

pub async fn update_doctor_line(http: &reqwest::Client) -> String {
    let options = UpdateOptions::default();
    match fetch_update_status(http, &options).await {
        Ok(status) if status.update_available => format!(
            "• Adapter update available: {} -> {}\n  Run: bears-acp-adapter update\n  Manifest: {}",
            crate::adapter_version(),
            status.manifest.version,
            status.manifest_url
        ),
        Ok(status) => format!(
            "✓ Adapter update check: current version {} is up to date for {} ({})",
            crate::adapter_version(),
            status.channel_label(),
            status.target
        ),
        Err(err) => format!("• Adapter update check unavailable: {err:#}"),
    }
}

async fn run_update(http: &reqwest::Client, options: &UpdateOptions) -> Result<()> {
    if env::consts::OS != "macos" {
        bail!("package self-update is currently implemented for macOS .pkg installs only");
    }

    let status = fetch_update_status(http, options).await?;
    print_update_status(&status);
    if !status.update_available {
        eprintln!("No update is available.");
        return Ok(());
    }

    let platform = status
        .platform
        .as_ref()
        .ok_or_else(|| anyhow!("manifest does not contain a package for {}", status.target))?;

    if !options.yes && !confirm_update(options, &status)? {
        eprintln!("Update cancelled.");
        return Ok(());
    }

    let pkg_path = download_update_pkg(http, platform, &status.target).await?;
    verify_macos_package(&pkg_path, platform)?;

    match options.install_mode {
        UpdateInstallMode::DownloadOnly => {
            eprintln!("Downloaded and verified package: {}", pkg_path.display());
            eprintln!("Install it later by opening the package, or run:");
            eprintln!(
                "  sudo /usr/sbin/installer -pkg {} -target /",
                pkg_path.display()
            );
        }
        UpdateInstallMode::OpenInstaller => {
            open_installer_gui(&pkg_path)?;
            eprintln!("Opened macOS Installer for {}", pkg_path.display());
            eprintln!("After installation, run: bears-acp-adapter doctor");
        }
        UpdateInstallMode::InstallWithSudo => {
            run_sudo_installer(&pkg_path)?;
            eprintln!("Update installed. Validate with: bears-acp-adapter doctor");
        }
    }

    Ok(())
}

async fn fetch_update_status(
    http: &reqwest::Client,
    options: &UpdateOptions,
) -> Result<UpdateStatus> {
    let target = update_target_triple();
    let manifest_url = options
        .manifest_url
        .clone()
        .unwrap_or_else(|| default_manifest_url(&options.channel, &target));
    let response = http
        .get(&manifest_url)
        .send()
        .await
        .with_context(|| format!("could not fetch update manifest from {manifest_url}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!(
            "update manifest fetch failed with HTTP {status}: {}",
            body.trim()
        );
    }

    let manifest: UpdateManifest = serde_json::from_str(&body).with_context(|| {
        format!(
            "update manifest from {manifest_url} was not valid JSON: {}",
            body.trim()
        )
    })?;
    let platform = manifest.platforms.get(&target).cloned();
    let update_available = if platform.is_some() {
        version_is_newer(&manifest.version, crate::adapter_version())?
    } else {
        false
    };

    Ok(UpdateStatus {
        manifest_url,
        target,
        manifest,
        platform,
        update_available,
    })
}

fn print_update_status(status: &UpdateStatus) {
    eprintln!("BEARS ACP adapter update check\n");
    eprintln!("Current version: {}", crate::adapter_version());
    eprintln!("Latest version:  {}", status.manifest.version);
    eprintln!("Channel:         {}", status.channel_label());
    eprintln!("Platform:        {}", status.target);
    eprintln!("Manifest:        {}", status.manifest_url);
    if let Some(notes) = status.manifest.release_notes_url.as_deref() {
        eprintln!("Release notes:   {notes}");
    }
    if status.manifest.mandatory.unwrap_or(false) {
        eprintln!("Mandatory:       yes");
    }
    if let Some(platform) = status.platform.as_ref() {
        if let Some(pkg_url) = platform.pkg_url.as_deref() {
            eprintln!("Package URL:     {pkg_url}");
        }
        if let Some(binary_url) = platform.binary_url.as_deref() {
            eprintln!("Binary URL:      {binary_url}");
        }
        eprintln!("Asset SHA256:    {}", platform.sha256);
        if let Some(size) = platform.size {
            eprintln!("Asset size:      {size} bytes");
        }
        if let Some(min_macos) = platform.min_macos.as_deref() {
            eprintln!("Minimum macOS:   {min_macos}");
        }
        if let Some(identifier) = platform.package_identifier.as_deref() {
            eprintln!("Package ID:      {identifier}");
        }
    } else {
        eprintln!("Package:         no package listed for this platform");
    }
    eprintln!(
        "Status:          {}",
        if status.update_available {
            "update available"
        } else {
            "up to date"
        }
    );
}

impl UpdateStatus {
    fn channel_label(&self) -> String {
        self.manifest
            .channel
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("unknown")
            .to_string()
    }
}

async fn download_update_pkg(
    http: &reqwest::Client,
    platform: &UpdatePlatform,
    target: &str,
) -> Result<PathBuf> {
    let raw_pkg_url = platform
        .pkg_url
        .as_deref()
        .ok_or_else(|| anyhow!("macOS package manifest is missing pkg_url"))?;
    let pkg_url = reqwest::Url::parse(raw_pkg_url)
        .with_context(|| format!("invalid package URL in update manifest: {raw_pkg_url}"))?;
    let filename = pkg_url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.trim().is_empty())
        .unwrap_or("bears-acp-adapter.pkg");
    let filename = sanitize_pkg_filename(filename, target);
    let dir = create_update_download_dir()?;
    let pkg_path = dir.join(filename);

    eprintln!("Downloading {raw_pkg_url}");
    let response = http
        .get(pkg_url)
        .send()
        .await
        .context("could not download update package")?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!(
            "update package download failed with HTTP {status}: {}",
            body.trim()
        );
    }
    let bytes = response.bytes().await.context("read update package body")?;
    if let Some(expected_size) = platform.size {
        let actual_size = bytes.len() as u64;
        if actual_size != expected_size {
            bail!("downloaded package size mismatch: expected {expected_size} bytes, got {actual_size}");
        }
    }
    fs::write(&pkg_path, &bytes)
        .with_context(|| format!("write downloaded package to {}", pkg_path.display()))?;
    eprintln!("Downloaded package: {}", pkg_path.display());
    Ok(pkg_path)
}

fn verify_macos_package(pkg_path: &Path, platform: &UpdatePlatform) -> Result<()> {
    eprintln!("Verifying package before installation...");
    verify_sha256(pkg_path, &platform.sha256)?;

    let expected_identifier = platform
        .package_identifier
        .as_deref()
        .unwrap_or(MACOS_PACKAGE_IDENTIFIER);
    if expected_identifier != MACOS_PACKAGE_IDENTIFIER {
        bail!(
            "unexpected package identifier in manifest: expected {MACOS_PACKAGE_IDENTIFIER}, got {expected_identifier}"
        );
    }

    let pkgutil = run_capture("pkgutil", &["--check-signature", &path_arg(pkg_path)])
        .context("pkgutil signature check failed")?;
    enforce_expected_signer(&pkgutil)?;
    eprintln!("✓ Package signature is trusted by macOS");

    run_capture(
        "spctl",
        &[
            "--assess",
            "--type",
            "install",
            "--verbose=4",
            &path_arg(pkg_path),
        ],
    )
    .context("Gatekeeper assessment failed")?;
    eprintln!("✓ Gatekeeper install assessment passed");

    run_capture("xcrun", &["stapler", "validate", &path_arg(pkg_path)])
        .context("notarization stapler validation failed")?;
    eprintln!("✓ Notarization ticket is stapled and valid");

    Ok(())
}

fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let expected = expected.trim().to_ascii_lowercase();
    if expected.is_empty() {
        bail!("update manifest did not include a SHA-256 digest");
    }
    let actual = sha256_file(path)?;
    if actual != expected {
        bail!(
            "SHA-256 mismatch for {}: expected {expected}, got {actual}",
            path.display()
        );
    }
    eprintln!("✓ SHA-256 digest matches manifest");
    Ok(())
}

fn enforce_expected_signer(pkgutil_output: &str) -> Result<()> {
    let expected_identity = runtime_or_build_value(
        "BEARS_ACP_UPDATE_INSTALLER_IDENTITY",
        option_env!("BEARS_ACP_ADAPTER_MACOS_INSTALLER_IDENTITY"),
    );
    let expected_team_id = runtime_or_build_value(
        "BEARS_ACP_UPDATE_INSTALLER_TEAM_ID",
        option_env!("BEARS_ACP_ADAPTER_MACOS_INSTALLER_TEAM_ID"),
    );

    if let Some(identity) = expected_identity.as_deref() {
        if !pkgutil_output.contains(identity) {
            bail!(
                "package was not signed by expected Developer ID Installer identity {identity:?}"
            );
        }
        eprintln!("✓ Package signer identity matches {identity}");
    }

    if let Some(team_id) = expected_team_id.as_deref() {
        if !pkgutil_output.contains(team_id) {
            bail!("package signer did not include expected Apple Team ID {team_id:?}");
        }
        eprintln!("✓ Package signer Team ID matches {team_id}");
    }

    if expected_identity.is_none() && expected_team_id.is_none() {
        eprintln!(
            "• No expected Developer ID Installer identity/team is compiled or configured; relying on pkgutil, Gatekeeper, and stapler validation"
        );
        eprintln!(
            "  For stricter verification, set BEARS_ACP_ADAPTER_MACOS_INSTALLER_TEAM_ID at build time or BEARS_ACP_UPDATE_INSTALLER_TEAM_ID at runtime."
        );
    }

    Ok(())
}

fn runtime_or_build_value(runtime_env: &str, build_value: Option<&'static str>) -> Option<String> {
    env::var(runtime_env)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| build_value.map(str::to_string))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn run_capture(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("run {program}"))?;
    let mut combined = String::new();
    combined.push_str(&String::from_utf8_lossy(&output.stdout));
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        bail!(
            "{program} exited with {}: {}",
            output.status,
            combined.trim()
        );
    }
    Ok(combined)
}

fn open_installer_gui(pkg_path: &Path) -> Result<()> {
    let status = Command::new("open")
        .arg(pkg_path)
        .status()
        .context("open macOS Installer")?;
    if !status.success() {
        bail!("open exited with {status}");
    }
    Ok(())
}

fn run_sudo_installer(pkg_path: &Path) -> Result<()> {
    eprintln!("Running macOS installer. You may be prompted for your macOS password.");
    let status = Command::new("/usr/bin/sudo")
        .args([
            "/usr/sbin/installer",
            "-pkg",
            &path_arg(pkg_path),
            "-target",
            "/",
        ])
        .status()
        .context("run sudo installer")?;
    if !status.success() {
        bail!("installer exited with {status}");
    }
    Ok(())
}

fn confirm_update(options: &UpdateOptions, status: &UpdateStatus) -> Result<bool> {
    eprintln!();
    eprintln!(
        "Update {} -> {} is available for {}.",
        crate::adapter_version(),
        status.manifest.version,
        status.target
    );
    match options.install_mode {
        UpdateInstallMode::OpenInstaller => {
            eprintln!("The verified .pkg will be opened in macOS Installer.")
        }
        UpdateInstallMode::InstallWithSudo => {
            eprintln!("The verified .pkg will be installed with sudo /usr/sbin/installer.")
        }
        UpdateInstallMode::DownloadOnly => {
            eprintln!("The verified .pkg will be downloaded but not installed.")
        }
    }
    eprint!("Continue? [y/N] ");
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("read update confirmation")?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("read {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn create_update_download_dir() -> Result<PathBuf> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let dir = env::temp_dir().join(format!(
        "bears-acp-adapter-update-{}-{millis}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
}

fn sanitize_pkg_filename(raw: &str, target: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|ch| match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '-' | '_' => ch,
            _ => '-',
        })
        .collect();
    if sanitized.ends_with(".pkg") && !sanitized.trim_matches('-').is_empty() {
        sanitized
    } else {
        format!("bears-acp-adapter-{target}.pkg")
    }
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn default_manifest_url(channel: &str, target: &str) -> String {
    format!("{DEFAULT_UPDATE_BASE_URL}/{channel}/{target}.json")
}

fn update_target_triple() -> String {
    match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin".to_string(),
        ("macos", "x86_64") => "x86_64-apple-darwin".to_string(),
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu".to_string(),
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu".to_string(),
        (os, arch) => format!("{arch}-{os}"),
    }
}

fn version_is_newer(latest: &str, current: &str) -> Result<bool> {
    let latest =
        parse_semver(latest).with_context(|| format!("invalid latest version {latest:?}"))?;
    let current =
        parse_semver(current).with_context(|| format!("invalid current version {current:?}"))?;
    Ok(latest > current)
}

fn parse_semver(raw: &str) -> Result<Version> {
    Version::parse(raw.trim().trim_start_matches('v')).map_err(Into::into)
}

fn normalize_update_channel(raw: &str) -> Result<String> {
    let channel = raw.trim().to_ascii_lowercase();
    if channel.is_empty() {
        bail!("update channel cannot be empty");
    }
    if !channel
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        bail!("update channel may only contain letters, digits, dash, and underscore");
    }
    Ok(channel)
}

fn require_arg_value(flag: &str, value: Option<String>) -> Result<String> {
    value.ok_or_else(|| anyhow!("{flag} requires a value"))
}

fn print_update_help_to_stderr() {
    eprintln!(
        "bears-acp-adapter update\n\nUsage:\n  bears-acp-adapter update-check [--channel stable] [--manifest-url <url>]\n  bears-acp-adapter update [--channel stable] [--manifest-url <url>] [--open|--install|--download-only] [--yes]\n\nOptions:\n  --channel <name>       Update channel, default BEARS_ACP_UPDATE_CHANNEL or stable\n  --manifest-url <url>   Override the update manifest URL\n  --open                 Download, verify, and open the .pkg in macOS Installer (default)\n  --install, --cli       Download, verify, and run sudo /usr/sbin/installer\n  --download-only        Download and verify the .pkg without installing\n  --yes, -y              Skip confirmation; defaults to --install unless --open or --download-only is also passed\n  --help                 Show this help\n\nEnvironment:\n  BEARS_ACP_UPDATE_CHANNEL\n  BEARS_ACP_UPDATE_MANIFEST_URL\n  BEARS_ACP_UPDATE_INSTALLER_TEAM_ID      optional strict runtime signer check\n  BEARS_ACP_UPDATE_INSTALLER_IDENTITY     optional strict runtime signer check"
    );
}

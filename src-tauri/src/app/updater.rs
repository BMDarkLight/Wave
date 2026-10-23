// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Looks for a newer Wave release on GitHub. On Android it also downloads the
//! release's APK for this phone's CPU and hands it to the system installer.
//! Desktop builds report the release and open its page, since each desktop
//! package format installs its own way.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use reqwest::header::ACCEPT;
use reqwest::StatusCode;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};

use crate::dto::{UpdateCheckDto, UpdateProgressDto};

const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/BMDarkLight/Wave/releases/latest";
/// The only pages Wave opens. A release URL from the API that is not under
/// this prefix is replaced with the latest-release page.
const RELEASES_PAGE_PREFIX: &str = "https://github.com/BMDarkLight/Wave/releases/";
const LATEST_RELEASE_PAGE: &str = "https://github.com/BMDarkLight/Wave/releases/latest";
const REPOSITORY_PAGE: &str = "https://github.com/BMDarkLight/Wave";
const NEW_ISSUE_PAGE: &str = "https://github.com/BMDarkLight/Wave/issues/new";
/// GitHub turns away issue links much past 8 KB, so a long report is cut to fit.
const MAX_ISSUE_QUERY_BYTES: usize = 7000;
const PROGRESS_EVENT: &str = "update-download-progress";
const UPDATES_DIR: &str = "updates";
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
    /// `sha256:<hex>`. GitHub fills this in for assets uploaded since 2025;
    /// older assets leave it out.
    #[serde(default)]
    digest: Option<String>,
}

/// What the last check found, so the download and the release page need no
/// second request.
#[derive(Debug, Clone)]
struct Found {
    version: Version,
    release_url: String,
    apk: Option<Asset>,
}

#[derive(Default)]
pub struct UpdaterState {
    found: Mutex<Option<Found>>,
    downloading: AtomicBool,
}

impl UpdaterState {
    fn found(&self) -> Result<Option<Found>, String> {
        self.found
            .lock()
            .map(|found| found.clone())
            .map_err(|_| "Update state lock poisoned".to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version(u64, u64, u64);

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// `1.2.3`, `v1.2.3`, `1.2` or `1.2.3-beta.1`. A pre-release suffix is
/// dropped: GitHub's latest release never points at a pre-release.
fn parse_version(raw: &str) -> Option<Version> {
    let raw = raw.trim();
    let raw = raw.strip_prefix(['v', 'V']).unwrap_or(raw);
    let core = raw.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |p| p.parse().ok())?;
    let patch = parts.next().map_or(Some(0), |p| p.parse().ok())?;
    if parts.next().is_some() {
        return None;
    }
    Some(Version(major, minor, patch))
}

/// The ABI name the release workflow puts in each APK's file name.
fn device_abi() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64-v8a",
        "arm" => "armeabi-v7a",
        "x86_64" => "x86_64",
        "x86" => "x86",
        _ => "",
    }
}

/// The APK built for `abi`, or one built for every ABI. Installing an APK
/// for another CPU fails, so there is no looser fallback.
fn pick_apk<'a>(assets: &'a [Asset], abi: &str) -> Option<&'a Asset> {
    let is_apk = |a: &&Asset| a.name.to_ascii_lowercase().ends_with(".apk");
    let own = (!abi.is_empty())
        .then(|| {
            let suffix = format!("_{abi}.apk").to_ascii_lowercase();
            assets
                .iter()
                .filter(is_apk)
                .find(|a| a.name.to_ascii_lowercase().ends_with(&suffix))
        })
        .flatten();
    own.or_else(|| {
        assets
            .iter()
            .filter(is_apk)
            .find(|a| a.name.to_ascii_lowercase().contains("universal"))
    })
}

fn client() -> Result<&'static Client, String> {
    static CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .connect_timeout(Duration::from_secs(15))
                // Applies to each read, so a slow but steady download is fine.
                .timeout(Duration::from_secs(30))
                // GitHub's API turns away requests without a user agent.
                .user_agent(concat!("Wave/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|e| format!("Could not set up the update check: {e}"))
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn updates_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_cache_dir()
        .map(|dir| dir.join(UPDATES_DIR))
        .map_err(|e| format!("Could not find the cache folder: {e}"))
}

fn apk_path(app: &AppHandle, version: Version) -> Result<PathBuf, String> {
    Ok(updates_dir(app)?.join(format!("Wave_{version}.apk")))
}

/// Downloads are only renamed into place once complete and checked, so a
/// file of the right size is a finished one.
fn is_downloaded(path: &Path, asset: &Asset) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.len() == asset.size)
}

pub fn check(app: &AppHandle, state: &UpdaterState) -> Result<UpdateCheckDto, String> {
    let current_text = app.package_info().version.to_string();
    let current = parse_version(&current_text)
        .ok_or_else(|| format!("Wave's own version {current_text} is not readable"))?;

    let response = client()?
        .get(LATEST_RELEASE_URL)
        .header(ACCEPT, "application/vnd.github+json")
        .send()
        .map_err(|e| format!("Could not reach GitHub: {e}"))?;
    let release: Option<Release> = match response.status() {
        // No published release yet.
        StatusCode::NOT_FOUND => None,
        status if status.is_success() => Some(
            response
                .json()
                .map_err(|e| format!("GitHub sent an unreadable release: {e}"))?,
        ),
        StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS => {
            return Err("GitHub is limiting requests right now. Try again later.".into());
        }
        status => return Err(format!("GitHub answered with {status}")),
    };

    let newer = release.and_then(|release| {
        let version = parse_version(&release.tag_name)?;
        (version > current).then_some((version, release))
    });

    let Some((version, release)) = newer else {
        *state
            .found
            .lock()
            .map_err(|_| "Update state lock poisoned")? = None;
        // A "ready to install" left from the update that is now running.
        #[cfg(target_os = "android")]
        crate::android::update_notice::cancel();
        // Whatever is left in the folder was installed already, or is older.
        if let Ok(dir) = updates_dir(app) {
            let _ = fs::remove_dir_all(dir);
        }
        return Ok(UpdateCheckDto {
            current_version: current.to_string(),
            latest_version: None,
            release_url: LATEST_RELEASE_PAGE.to_string(),
            download_size: None,
            downloaded: false,
            notifications_blocked: false,
        });
    };

    let apk = if cfg!(target_os = "android") {
        pick_apk(&release.assets, device_abi()).cloned()
    } else {
        None
    };
    let release_url = if release.html_url.starts_with(RELEASES_PAGE_PREFIX) {
        release.html_url
    } else {
        LATEST_RELEASE_PAGE.to_string()
    };
    let downloaded = match &apk {
        Some(asset) => is_downloaded(&apk_path(app, version)?, asset),
        None => false,
    };
    // Downloaded on an earlier launch and not installed yet: say so again.
    // Not downloaded and nothing downloading: a progress notification left by
    // a download the system killed would otherwise stay up for good, since it
    // can't be swiped away.
    #[cfg(target_os = "android")]
    if downloaded {
        crate::android::update_notice::ready(&version.to_string(), &apk_path(app, version)?);
    } else if !state.downloading.load(Ordering::SeqCst) {
        crate::android::update_notice::cancel();
    }
    let dto = UpdateCheckDto {
        current_version: current.to_string(),
        latest_version: Some(version.to_string()),
        release_url: release_url.clone(),
        download_size: apk.as_ref().map(|asset| asset.size),
        downloaded,
        notifications_blocked: notifications_blocked(),
    };
    *state
        .found
        .lock()
        .map_err(|_| "Update state lock poisoned")? = Some(Found {
        version,
        release_url,
        apk,
    });
    Ok(dto)
}

struct ClearOnDrop<'a>(&'a AtomicBool);

impl Drop for ClearOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Whether the update notifications can't be shown, so the toast must stay.
fn notifications_blocked() -> bool {
    #[cfg(target_os = "android")]
    {
        !crate::android::update_notice::enabled()
    }
    #[cfg(not(target_os = "android"))]
    {
        false
    }
}

/// Download the APK the last check found, reporting progress as
/// `update-download-progress` events.
pub fn download(app: &AppHandle, state: &UpdaterState) -> Result<(), String> {
    let found = state
        .found()?
        .ok_or("There is no update to download. Check for updates first.")?;
    let asset = found
        .apk
        .ok_or("This release has no download for this device.")?;
    if state.downloading.swap(true, Ordering::SeqCst) {
        return Err("The update is already downloading.".into());
    }
    let _busy = ClearOnDrop(&state.downloading);

    let path = apk_path(app, found.version)?;
    let version = found.version.to_string();
    if is_downloaded(&path, &asset) {
        #[cfg(target_os = "android")]
        crate::android::update_notice::ready(&version, &path);
        return Ok(());
    }
    let dir = updates_dir(app)?;
    // Only one update is ever kept: drop older ones and any broken download.
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).map_err(|e| format!("Could not make the updates folder: {e}"))?;

    let partial = path.with_extension("apk.part");
    let result = fetch(app, &version, &asset, &partial).and_then(|()| {
        fs::rename(&partial, &path).map_err(|e| format!("Could not save the update: {e}"))
    });
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    // On Android the notification takes over from the in-app toast, which
    // the phone layout has no room for: tapping it opens the installer.
    #[cfg(target_os = "android")]
    match &result {
        Ok(()) => crate::android::update_notice::ready(&version, &path),
        Err(e) => crate::android::update_notice::failed(&version, e),
    }
    result
}

fn fetch(app: &AppHandle, version: &str, asset: &Asset, dest: &Path) -> Result<(), String> {
    let mut response = client()?
        .get(&asset.browser_download_url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Could not download the update: {e}"))?;
    let mut file = File::create(dest).map_err(|e| format!("Could not save the update: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut received: u64 = 0;
    let mut last_report = Instant::now();
    let report = |received: u64| {
        let _ = app.emit(
            PROGRESS_EVENT,
            UpdateProgressDto {
                received,
                total: asset.size,
            },
        );
        #[cfg(target_os = "android")]
        crate::android::update_notice::progress(version, received, asset.size);
        #[cfg(not(target_os = "android"))]
        let _ = version;
    };
    report(0);
    loop {
        let n = response
            .read(&mut buf)
            .map_err(|e| format!("The download stopped: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("Could not save the update: {e}"))?;
        hasher.update(&buf[..n]);
        received += n as u64;
        if last_report.elapsed() >= PROGRESS_INTERVAL {
            last_report = Instant::now();
            report(received);
        }
    }
    file.sync_all()
        .map_err(|e| format!("Could not save the update: {e}"))?;
    report(received);

    if received != asset.size {
        return Err("The download was cut short. Try again.".into());
    }
    if let Some(expected) = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
    {
        let actual: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if !actual.eq_ignore_ascii_case(expected) {
            return Err("The download does not match the release. Try again.".into());
        }
    }
    Ok(())
}

/// Hand the downloaded APK to Android's installer, which asks the user to
/// confirm.
pub fn install(app: &AppHandle, state: &UpdaterState) -> Result<(), String> {
    let found = state
        .found()?
        .ok_or("There is no update to install. Check for updates first.")?;
    let asset = found
        .apk
        .ok_or("This release has no download for this device.")?;
    let path = apk_path(app, found.version)?;
    if !is_downloaded(&path, &asset) {
        return Err("The update has not finished downloading.".into());
    }
    #[cfg(target_os = "android")]
    {
        crate::android::app_updater::install(&path)
    }
    #[cfg(not(target_os = "android"))]
    {
        Err("Updates install from the release page on this platform.".into())
    }
}

/// Open the release the last check found, or the latest release page.
pub fn open_release_page(state: &UpdaterState) -> Result<(), String> {
    let url = state
        .found()?
        .map(|found| found.release_url)
        .unwrap_or_else(|| LATEST_RELEASE_PAGE.to_string());
    open_url(&url)
}

/// Open Wave's source repository, linked from the foot of Settings.
pub fn open_repository() -> Result<(), String> {
    open_url(REPOSITORY_PAGE)
}

/// Open a new GitHub issue filled in with a crash report. The user reads it
/// over and submits it in the browser; nothing is sent from here.
pub fn open_crash_issue(report: &str) -> Result<(), String> {
    open_url(&crash_issue_url(report))
}

fn crash_issue_url(report: &str) -> String {
    let report = report.trim();
    let field = |name: &str| {
        report
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let kind = field("Type: ")
        .map(|t| t.rsplit('.').next().unwrap_or(t))
        .or_else(|| field("Where: "))
        .unwrap_or("crash");
    let mut title = format!("Android crash: {kind}");
    if let Some(message) = field("Message: ") {
        let short: String = message.chars().take(100).collect();
        title.push_str(": ");
        title.push_str(&short);
        if short.len() < message.len() {
            title.push_str("...");
        }
    }

    let mut kept = report.chars().count();
    loop {
        let mut text: String = report.chars().take(kept).collect();
        if kept < report.chars().count() {
            text.push_str("\n[report cut to fit the link]");
        }
        let body = format!(
            "**What were you doing when it happened?**\n\n\n\n**Crash report**\n\n```\n{text}\n```\n"
        );
        let query: String = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("title", &title)
            .append_pair("labels", "bug")
            .append_pair("body", &body)
            .finish();
        if query.len() <= MAX_ISSUE_QUERY_BYTES || kept == 0 {
            return format!("{NEW_ISSUE_PAGE}?{query}");
        }
        kept = kept * 9 / 10;
    }
}

#[cfg(target_os = "android")]
pub(crate) fn open_url(url: &str) -> Result<(), String> {
    crate::android::app_updater::open_url(url)
}

#[cfg(not(target_os = "android"))]
pub(crate) fn open_url(url: &str) -> Result<(), String> {
    use std::process::Command;

    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = Command::new("explorer");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = Command::new("xdg-open");

    command
        .arg(url)
        .spawn()
        .map(drop)
        .map_err(|e| format!("Could not open the browser: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> Asset {
        Asset {
            name: name.to_string(),
            browser_download_url: format!("https://example.invalid/{name}"),
            size: 1,
            digest: None,
        }
    }

    #[test]
    fn parses_release_tags() {
        assert_eq!(parse_version("v1.0.0"), Some(Version(1, 0, 0)));
        assert_eq!(parse_version("1.2.3"), Some(Version(1, 2, 3)));
        assert_eq!(parse_version("V2.10"), Some(Version(2, 10, 0)));
        assert_eq!(parse_version("3"), Some(Version(3, 0, 0)));
        assert_eq!(parse_version("v1.4.0-beta.2"), Some(Version(1, 4, 0)));
        assert_eq!(parse_version("1.4.0+build.7"), Some(Version(1, 4, 0)));
        assert_eq!(parse_version("nightly"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn compares_versions_numerically() {
        assert!(parse_version("1.10.0") > parse_version("1.9.9"));
        assert!(parse_version("2.0.0") > parse_version("1.99.99"));
        assert!(parse_version("1.0.1") > parse_version("v1.0.0"));
        assert_eq!(parse_version("v1.0.0"), parse_version("1.0"));
    }

    #[test]
    fn picks_the_apk_for_this_abi() {
        let assets = [
            asset("Wave_1.1.0_amd64.deb"),
            asset("Wave_1.1.0_armeabi-v7a.apk"),
            asset("Wave_1.1.0_arm64-v8a.apk"),
        ];
        assert_eq!(
            pick_apk(&assets, "arm64-v8a").map(|a| a.name.as_str()),
            Some("Wave_1.1.0_arm64-v8a.apk")
        );
        assert_eq!(
            pick_apk(&assets, "armeabi-v7a").map(|a| a.name.as_str()),
            Some("Wave_1.1.0_armeabi-v7a.apk")
        );
        assert!(pick_apk(&assets, "x86_64").is_none());
        assert!(pick_apk(&assets, "").is_none());
    }

    #[test]
    fn falls_back_to_a_universal_apk() {
        let assets = [
            asset("Wave_1.1.0_arm64-v8a.apk"),
            asset("Wave_1.1.0_universal.apk"),
        ];
        assert_eq!(
            pick_apk(&assets, "x86_64").map(|a| a.name.as_str()),
            Some("Wave_1.1.0_universal.apk")
        );
        assert_eq!(
            pick_apk(&assets, "arm64-v8a").map(|a| a.name.as_str()),
            Some("Wave_1.1.0_arm64-v8a.apk")
        );
    }

    #[test]
    fn reads_a_github_release() {
        let json = r#"{
            "tag_name": "v1.1.0",
            "html_url": "https://github.com/BMDarkLight/Wave/releases/tag/v1.1.0",
            "draft": false,
            "assets": [{
                "name": "Wave_1.1.0_arm64-v8a.apk",
                "browser_download_url": "https://github.com/BMDarkLight/Wave/releases/download/v1.1.0/Wave_1.1.0_arm64-v8a.apk",
                "size": 31457280,
                "digest": "sha256:abc123",
                "content_type": "application/vnd.android.package-archive"
            }]
        }"#;
        let release: Release = serde_json::from_str(json).unwrap();
        assert_eq!(parse_version(&release.tag_name), Some(Version(1, 1, 0)));
        let apk = pick_apk(&release.assets, "arm64-v8a").unwrap();
        assert_eq!(apk.size, 31457280);
        assert_eq!(apk.digest.as_deref(), Some("sha256:abc123"));
    }

    #[test]
    fn crash_issue_url_carries_the_report_and_a_title() {
        let report = "Wave FATAL\nThread: Thread-20\nType: java.lang.RuntimeException\nMessage: MediaMetadataProbe failed\n\n at a.b(c)";
        let url = url::Url::parse(&crash_issue_url(report)).unwrap();
        assert!(url.as_str().starts_with(NEW_ISSUE_PAGE));
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(
            pairs["title"],
            "Android crash: RuntimeException: MediaMetadataProbe failed"
        );
        assert!(pairs["body"].contains("Thread: Thread-20"));
        assert!(pairs["body"].contains(" at a.b(c)"));
    }

    #[test]
    fn crash_issue_url_cuts_a_long_report_to_fit() {
        let report = format!(
            "Wave FATAL\nType: x.Boom\n{}",
            "at frame (File.java:1)\n".repeat(2000)
        );
        let url = crash_issue_url(&report);
        assert!(url.len() <= NEW_ISSUE_PAGE.len() + 1 + MAX_ISSUE_QUERY_BYTES);
        assert!(url::Url::parse(&url)
            .unwrap()
            .query_pairs()
            .any(|(k, v)| k == "body" && v.contains("[report cut to fit the link]")));
    }
}

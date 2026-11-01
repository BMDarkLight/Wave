// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Installs and updates the programs Online play runs: yt-dlp, and Deno,
//! which yt-dlp needs to answer YouTube's player challenges.
//!
//! Both come from the projects' own latest GitHub release and are kept only
//! when they match the SHA-256 digest GitHub lists for the asset. An asset
//! without a digest is refused. Files are written beside their final name and
//! renamed into place once checked, so a cut-off download never replaces a
//! working program.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::blocking::Client;
use reqwest::header::ACCEPT;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Folder under the app data dir that holds both programs.
pub const TOOLS_DIR: &str = "tools";
const MANIFEST: &str = "tools.json";
/// How long an install is trusted before the next use asks GitHub whether a
/// newer release is out. YouTube changes often enough that yt-dlp ships
/// fixes every few days.
pub const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    YtDlp,
    Deno,
}

impl Tool {
    pub const ALL: [Tool; 2] = [Tool::YtDlp, Tool::Deno];

    pub fn key(self) -> &'static str {
        match self {
            Tool::YtDlp => "yt-dlp",
            Tool::Deno => "deno",
        }
    }

    fn repo(self) -> &'static str {
        match self {
            Tool::YtDlp => "yt-dlp/yt-dlp",
            Tool::Deno => "denoland/deno",
        }
    }

    /// Where the program sits once installed: alone in `dir`, or inside its
    /// own folder there.
    pub fn path(self, dir: &Path) -> PathBuf {
        match asset_for_this_machine(self).map(|(_, layout)| layout) {
            Some(Layout::Member(member)) => dir.join(member),
            Some(Layout::Folder { exe }) => dir.join(self.key()).join(exe),
            None => dir.join(self.key()),
        }
    }
}

/// How a downloaded release asset becomes the installed program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// A zip holding the program alone, as `member`.
    Member(&'static str),
    /// A zip of the program's whole folder, unpacked as it is, with `exe` the
    /// program in it. yt-dlp ships this way too, and unlike its one-file
    /// build it doesn't unpack itself on every run: a call takes a fraction
    /// of a second instead of ten.
    Folder { exe: &'static str },
}

/// The release asset to fetch for `tool` on `os`/`arch` (as in
/// [`std::env::consts`]), and how it unpacks.
fn asset_for(tool: Tool, os: &str, arch: &str) -> Option<(String, Layout)> {
    match tool {
        Tool::YtDlp => {
            let (name, exe) = match (os, arch) {
                // A universal build.
                ("macos", _) => ("yt-dlp_macos.zip", "yt-dlp_macos"),
                ("linux", "x86_64") => ("yt-dlp_linux.zip", "yt-dlp_linux"),
                ("linux", "aarch64") => ("yt-dlp_linux_aarch64.zip", "yt-dlp_linux_aarch64"),
                ("windows", "x86_64") => ("yt-dlp_win.zip", "yt-dlp.exe"),
                ("windows", "aarch64") => ("yt-dlp_win_arm64.zip", "yt-dlp_arm64.exe"),
                _ => return None,
            };
            Some((name.to_string(), Layout::Folder { exe }))
        }
        Tool::Deno => {
            let (triple, member) = match os {
                "macos" => ("apple-darwin", "deno"),
                "linux" => ("unknown-linux-gnu", "deno"),
                "windows" => ("pc-windows-msvc", "deno.exe"),
                _ => return None,
            };
            if !matches!(arch, "x86_64" | "aarch64") {
                return None;
            }
            Some((format!("deno-{arch}-{triple}.zip"), Layout::Member(member)))
        }
    }
}

fn asset_for_this_machine(tool: Tool) -> Option<(String, Layout)> {
    asset_for(tool, std::env::consts::OS, std::env::consts::ARCH)
}

/// Whether Online play can run on this machine at all.
pub fn supported() -> bool {
    Tool::ALL
        .iter()
        .all(|&tool| asset_for_this_machine(tool).is_some())
}

// ── Manifest ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Installed {
    /// The release tag, shown as the version.
    pub tag: String,
    /// Digest of the downloaded asset, to tell whether the latest release is
    /// a different build.
    pub sha256: String,
    /// When GitHub was last asked about a newer release, in Unix seconds.
    pub checked_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    tools: HashMap<String, Installed>,
}

fn read_manifest(dir: &Path) -> Manifest {
    fs::read(dir.join(MANIFEST))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_manifest(dir: &Path, manifest: &Manifest) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(manifest).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(".{MANIFEST}.new"));
    fs::write(&tmp, json).map_err(|e| format!("Could not save the tools list: {e}"))?;
    fs::rename(&tmp, dir.join(MANIFEST)).map_err(|e| format!("Could not save the tools list: {e}"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The installed copy of `tool`, if its program is still on disk.
pub fn installed(dir: &Path, tool: Tool) -> Option<Installed> {
    if !tool.path(dir).is_file() {
        return None;
    }
    read_manifest(dir).tools.get(tool.key()).cloned()
}

pub fn all_installed(dir: &Path) -> bool {
    Tool::ALL.iter().all(|&tool| installed(dir, tool).is_some())
}

fn is_stale(entry: &Installed, now: u64) -> bool {
    now.saturating_sub(entry.checked_at) >= STALE_AFTER.as_secs()
}

// ── GitHub ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

impl Asset {
    fn sha256(&self) -> Option<String> {
        self.digest
            .as_deref()
            .and_then(|d| d.strip_prefix("sha256:"))
            .filter(|hex| hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()))
            .map(str::to_ascii_lowercase)
    }
}

fn client() -> Result<&'static Client, String> {
    static CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .connect_timeout(Duration::from_secs(15))
                // Applies to each read, so a slow but steady download is fine.
                .timeout(Duration::from_secs(30))
                .user_agent(super::super::USER_AGENT)
                .build()
                .map_err(|e| format!("Could not set up the download: {e}"))
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn latest_release(tool: Tool) -> Result<Release, String> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        tool.repo()
    );
    client()?
        .get(url)
        .header(ACCEPT, "application/vnd.github+json")
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.json())
        .map_err(|e| format!("Could not look up the latest {}: {e}", tool.key()))
}

// ── Install ───────────────────────────────────────────────────────────────

/// Download progress for one program.
#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub tool: Tool,
    pub received: u64,
    pub total: u64,
}

/// Make sure the latest release of `tool` is installed in `dir`, downloading
/// it only when the installed build differs. Returns the program's path.
pub fn ensure(
    dir: &Path,
    tool: Tool,
    progress: &mut dyn FnMut(Progress),
) -> Result<PathBuf, String> {
    let (asset_name, layout) = asset_for_this_machine(tool)
        .ok_or_else(|| format!("{} has no build for this computer", tool.key()))?;
    fs::create_dir_all(dir).map_err(|e| format!("Could not create the tools folder: {e}"))?;

    let release = latest_release(tool)?;
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .ok_or_else(|| format!("The latest {} release has no {asset_name}", tool.key()))?;
    let expected = asset.sha256().ok_or_else(|| {
        format!(
            "The latest {} release lists no checksum for {asset_name}, so it was not installed",
            tool.key()
        )
    })?;

    let mut manifest = read_manifest(dir);
    let target = tool.path(dir);
    let current = manifest
        .tools
        .get(tool.key())
        .filter(|entry| entry.sha256 == expected && target.is_file());
    if current.is_none() {
        let part = dir.join(format!(".{asset_name}.part"));
        let result = download_verified(asset, &expected, &part, &mut |received| {
            progress(Progress {
                tool,
                received,
                total: asset.size,
            })
        })
        .and_then(|()| place(&part, layout, dir, tool));
        let _ = fs::remove_file(&part);
        result?;
    }

    manifest.tools.insert(
        tool.key().to_string(),
        Installed {
            tag: release.tag_name,
            sha256: expected,
            checked_at: now(),
        },
    );
    write_manifest(dir, &manifest)?;
    Ok(target)
}

/// Install every program Online play needs.
pub fn install_all(dir: &Path, progress: &mut dyn FnMut(Progress)) -> Result<(), String> {
    for tool in Tool::ALL {
        ensure(dir, tool, progress)?;
    }
    Ok(())
}

/// Update each installed program that hasn't been checked for a day. Failures
/// are only logged: the copy already installed keeps working until YouTube
/// changes something.
pub fn refresh_if_stale(dir: &Path) {
    let manifest = read_manifest(dir);
    let now = now();
    for tool in Tool::ALL {
        let stale = manifest
            .tools
            .get(tool.key())
            .is_some_and(|entry| is_stale(entry, now));
        if stale {
            if let Err(e) = ensure(dir, tool, &mut |_| {}) {
                tracing::warn!("Online play: could not update {}: {e}", tool.key());
            }
        }
    }
}

/// Delete both programs and everything yt-dlp cached beside them.
pub fn remove_all(dir: &Path) -> Result<(), String> {
    match fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Could not remove the Online play tools: {e}")),
    }
}

fn download_verified(
    asset: &Asset,
    expected: &str,
    dest: &Path,
    progress: &mut dyn FnMut(u64),
) -> Result<(), String> {
    let mut response = client()?
        .get(&asset.browser_download_url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("Could not download {}: {e}", asset.name))?;
    let mut file = File::create(dest).map_err(|e| format!("Could not save {}: {e}", asset.name))?;
    let (received, actual) = copy_hashing(&mut response, &mut file, progress)
        .map_err(|e| format!("The download of {} stopped: {e}", asset.name))?;
    file.sync_all()
        .map_err(|e| format!("Could not save {}: {e}", asset.name))?;
    if received != asset.size {
        return Err(format!(
            "The download of {} was cut short. Try again.",
            asset.name
        ));
    }
    if actual != expected {
        return Err(format!(
            "{} does not match its published checksum, so it was not installed",
            asset.name
        ));
    }
    Ok(())
}

/// Copy `from` into `to`, returning the byte count and the SHA-256 hex.
fn copy_hashing(
    from: &mut dyn Read,
    to: &mut dyn Write,
    progress: &mut dyn FnMut(u64),
) -> io::Result<(u64, String)> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut received = 0u64;
    progress(0);
    loop {
        let n = from.read(&mut buf)?;
        if n == 0 {
            break;
        }
        to.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        received += n as u64;
        progress(received);
    }
    let hex = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok((received, hex))
}

/// Install a checked download into `dir` in place of what was there.
/// Everything is unpacked beside its final name first, so a failure halfway
/// leaves the old install working.
fn place(download: &Path, layout: Layout, dir: &Path, tool: Tool) -> Result<(), String> {
    let fail = |e: &dyn std::fmt::Display| format!("Could not install {}: {e}", tool.key());
    let (staged, target, program) = match layout {
        Layout::Member(member) => {
            let staged = dir.join(format!(".{member}.new"));
            extract_member(download, member, &staged)?;
            (staged.clone(), dir.join(member), staged)
        }
        Layout::Folder { exe } => {
            let staged = dir.join(format!(".{}.new", tool.key()));
            remove_any(&staged).map_err(|e| fail(&e))?;
            let file = File::open(download).map_err(|e| fail(&e))?;
            zip::ZipArchive::new(file)
                .and_then(|mut archive| archive.extract(&staged))
                .map_err(|e| fail(&e))?;
            let program = staged.join(exe);
            if !program.is_file() {
                let _ = remove_any(&staged);
                return Err(format!("The {} archive has no {exe}", tool.key()));
            }
            (staged, dir.join(tool.key()), program)
        }
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).map_err(|e| fail(&e))?;
    }
    #[cfg(not(unix))]
    let _ = program;
    // The old install, or a single file an earlier build left at that name.
    remove_any(&target).map_err(|e| fail(&e))?;
    fs::rename(&staged, &target).map_err(|e| {
        let _ = remove_any(&staged);
        fail(&e)
    })
}

/// Delete a file or a folder, and nothing if neither is there.
fn remove_any(path: &Path) -> io::Result<()> {
    let removed = if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    match removed {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn extract_member(zip_path: &Path, member: &str, dest: &Path) -> Result<(), String> {
    let file =
        File::open(zip_path).map_err(|e| format!("Could not open {}: {e}", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("Not a zip archive: {e}"))?;
    let mut entry = archive
        .by_name(member)
        .map_err(|_| format!("The archive has no {member}"))?;
    let mut out = File::create(dest).map_err(|e| format!("Could not unpack {member}: {e}"))?;
    io::copy(&mut entry, &mut out).map_err(|e| format!("Could not unpack {member}: {e}"))?;
    out.sync_all()
        .map_err(|e| format!("Could not unpack {member}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wave-tools-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn picks_the_asset_for_each_platform() {
        let yt = |os, arch| asset_for(Tool::YtDlp, os, arch);
        let folder = |name: &str, exe| Some((name.to_string(), Layout::Folder { exe }));
        assert_eq!(
            yt("macos", "aarch64"),
            folder("yt-dlp_macos.zip", "yt-dlp_macos")
        );
        assert_eq!(
            yt("macos", "x86_64"),
            folder("yt-dlp_macos.zip", "yt-dlp_macos")
        );
        assert_eq!(
            yt("linux", "x86_64"),
            folder("yt-dlp_linux.zip", "yt-dlp_linux")
        );
        assert_eq!(
            yt("linux", "aarch64"),
            folder("yt-dlp_linux_aarch64.zip", "yt-dlp_linux_aarch64")
        );
        assert_eq!(
            yt("windows", "x86_64"),
            folder("yt-dlp_win.zip", "yt-dlp.exe")
        );
        assert_eq!(
            yt("windows", "aarch64"),
            folder("yt-dlp_win_arm64.zip", "yt-dlp_arm64.exe")
        );
        assert_eq!(yt("linux", "arm"), None);

        assert_eq!(
            asset_for(Tool::Deno, "macos", "aarch64"),
            Some((
                "deno-aarch64-apple-darwin.zip".into(),
                Layout::Member("deno")
            ))
        );
        assert_eq!(
            asset_for(Tool::Deno, "windows", "x86_64"),
            Some((
                "deno-x86_64-pc-windows-msvc.zip".into(),
                Layout::Member("deno.exe")
            ))
        );
        assert_eq!(
            asset_for(Tool::Deno, "linux", "aarch64"),
            Some((
                "deno-aarch64-unknown-linux-gnu.zip".into(),
                Layout::Member("deno")
            ))
        );
        assert_eq!(asset_for(Tool::Deno, "android", "aarch64"), None);
    }

    #[test]
    fn only_a_well_formed_sha256_digest_counts() {
        let asset = |digest: Option<&str>| Asset {
            name: "a".into(),
            browser_download_url: String::new(),
            size: 0,
            digest: digest.map(String::from),
        };
        let hex = "AB".repeat(32);
        assert_eq!(
            asset(Some(&format!("sha256:{hex}"))).sha256(),
            Some("ab".repeat(32))
        );
        assert_eq!(asset(None).sha256(), None);
        assert_eq!(asset(Some("sha512:abcd")).sha256(), None);
        assert_eq!(asset(Some("sha256:not-hex")).sha256(), None);
    }

    #[test]
    fn hashes_while_copying() {
        let mut out = Vec::new();
        let mut seen = Vec::new();
        let (n, hex) = copy_hashing(&mut &b"abc"[..], &mut out, &mut |r| seen.push(r)).unwrap();
        assert_eq!(n, 3);
        assert_eq!(out, b"abc");
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(seen.first(), Some(&0));
        assert_eq!(seen.last(), Some(&3));
    }

    #[test]
    fn a_day_old_check_is_stale() {
        let entry = |checked_at| Installed {
            tag: "t".into(),
            sha256: "s".into(),
            checked_at,
        };
        let now = 10 * STALE_AFTER.as_secs();
        assert!(!is_stale(&entry(now - 60), now));
        assert!(is_stale(&entry(now - STALE_AFTER.as_secs()), now));
        // A clock that went backwards is not a reason to download.
        assert!(!is_stale(&entry(now + 60), now));
    }

    fn zip_of(path: &Path, files: &[(&str, &[u8])]) {
        let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in files {
            writer.start_file(*name, options).unwrap();
            writer.write_all(body).unwrap();
        }
        writer.finish().unwrap();
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn unpacks_the_program_from_a_zip_and_marks_it_runnable() {
        let dir = temp_dir("zip");
        let zip_path = dir.join("deno.zip");
        zip_of(&zip_path, &[("deno", b"#!/bin/sh\necho deno\n")]);
        place(&zip_path, Layout::Member("deno"), &dir, Tool::Deno).unwrap();
        assert_eq!(
            fs::read(dir.join("deno")).unwrap(),
            b"#!/bin/sh\necho deno\n"
        );
        #[cfg(unix)]
        assert_eq!(mode(&dir.join("deno")), 0o755);
        assert!(place(&zip_path, Layout::Member("missing"), &dir, Tool::Deno).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unpacks_a_whole_folder_over_the_old_install() {
        let dir = temp_dir("folder");
        // What an earlier one-file build left at the folder's name.
        fs::write(dir.join("yt-dlp"), b"old single file").unwrap();
        let zip_path = dir.join("yt-dlp.zip");
        zip_of(
            &zip_path,
            &[("prog", b"#!/bin/sh\n"), ("_internal/lib.so", b"library")],
        );
        place(&zip_path, Layout::Folder { exe: "prog" }, &dir, Tool::YtDlp).unwrap();
        assert_eq!(
            fs::read(dir.join("yt-dlp/_internal/lib.so")).unwrap(),
            b"library"
        );
        #[cfg(unix)]
        assert_eq!(mode(&dir.join("yt-dlp/prog")), 0o755);

        // An archive without the program leaves the working install alone.
        let broken = dir.join("broken.zip");
        zip_of(&broken, &[("other", b"x")]);
        assert!(place(&broken, Layout::Folder { exe: "prog" }, &dir, Tool::YtDlp).is_err());
        assert!(dir.join("yt-dlp/prog").is_file());
        assert!(!dir.join(".yt-dlp.new").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_program_counts_as_installed_only_when_its_file_is_there() {
        let dir = temp_dir("manifest");
        let mut manifest = Manifest::default();
        manifest.tools.insert(
            Tool::YtDlp.key().into(),
            Installed {
                tag: "2026.10.26".into(),
                sha256: "s".into(),
                checked_at: 1,
            },
        );
        write_manifest(&dir, &manifest).unwrap();
        assert_eq!(installed(&dir, Tool::YtDlp), None);

        let program = Tool::YtDlp.path(&dir);
        fs::create_dir_all(program.parent().unwrap()).unwrap();
        fs::write(&program, b"bin").unwrap();
        assert_eq!(
            installed(&dir, Tool::YtDlp).map(|i| i.tag).as_deref(),
            Some("2026.10.26")
        );
        assert!(!all_installed(&dir), "Deno is still missing");

        remove_all(&dir).unwrap();
        assert!(!dir.exists());
        remove_all(&dir).unwrap();
    }
}

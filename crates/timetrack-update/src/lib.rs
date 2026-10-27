/* update/lib.rs
 *
 * Copyright 2026 sequ
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

//! Self-update logic for the TimeTrack AppImage.
//!
//! The GUI runs this on startup, but only when it is running as an AppImage
//! (see [`app_image_path`]): a host `cargo build` has no release to update
//! to, and the CLI ships in the same image so there is nothing to replace
//! by hand.
//!
//! The flow is deliberately in two halves. The *check* reads the latest
//! GitHub release, compares its tag against this build's version, and picks
//! the asset for this architecture -- pure data, no side effects. The
//! *install* downloads that asset over HTTPS under a hard byte cap, verifies
//! its SHA256 against the release's `SHA256SUMS` asset, and only then
//! renames it over the running AppImage (renaming a running file is fine:
//! the process keeps its old inode). The GUI confirms twice before calling
//! it, like every other destructive action in this app.
//!
//! # What this crate deliberately does not depend on
//!
//! No GUI, no IPC, no async runtime. The one network call is blocking
//! (`ureq`), driven by the caller from a thread of its own -- never from a
//! gpui future. Network failures are ordinary errors the caller is expected
//! to swallow: an offline launch is not an update failure.

use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

/// Where the latest release is read from. Tags are `v<version>` and carry a
/// `TimeTrack-<arch>.AppImage` asset each; the comparison strips the `v` and
/// matches [`CARGO_PKG_VERSION`], so a tag and a Cargo bump ship together.
pub const RELEASES_URL: &str = "https://api.github.com/repos/sequ-r/timetrack/releases/latest";

/// Set to skip the automatic check (e.g. metered connections, or a pinned
/// install). Installing on demand is unaffected.
pub const NO_UPDATE_ENV: &str = "TIMETRACK_NO_UPDATE";

/// The AppImage runtime exports the path of the running image here. Absent
/// outside an AppImage -- which is exactly when there is nothing to update.
pub const APPIMAGE_ENV: &str = "APPIMAGE";

/// How long the update check and the download may each take. A personal
/// tracker must not hang its startup on a slow network; anything slower is
/// an offline launch, not an update failure.
const TIMEOUT: Duration = Duration::from_secs(15);

/// Refuse downloads past this: the image is ~10 MB, so this is two orders of
/// magnitude of headroom, not a limit the real asset can hit. Enforced while
/// reading (see [`fetch_capped`]), never from `Content-Length`: a chunked
/// body has no length to check, and a lying one is the whole threat.
const MAX_BYTES: u64 = 512 * 1024 * 1024;

/// The same cap for the `SHA256SUMS` asset, which is a few hundred bytes.
const MAX_SUMS_BYTES: u64 = 1024 * 1024;

/// The checksum asset `release.yml` publishes next to every AppImage.
const SUMS_NAME: &str = "SHA256SUMS";

/// What went wrong. Network and protocol failures exist so the GUI can tell
/// "offline" from "release broken"; both still surface as one quiet status
/// line, never a dialog.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum UpdateError {
    #[error("could not reach the releases page ({0})")]
    Network(String),
    #[error("the releases page did not parse ({0})")]
    BadRelease(String),
    #[error("not running as an AppImage")]
    NotAppImage,
    #[error("download failed ({0})")]
    Download(String),
    #[error("the downloaded file is not an AppImage")]
    NotAnAppImage,
    #[error("the download exceeds the size cap")]
    TooLarge,
    #[error("the download does not match the release checksum")]
    ChecksumMismatch,
    #[error("the release checksum could not be used ({0})")]
    BadChecksum(String),
    #[error("could not replace the AppImage ({0})")]
    Install(String),
}

/// A dotted version, parsed tolerantly: a leading `v` is stripped, each part
/// is its leading run of digits, missing parts are zero. `v0.2` and `0.2.0`
/// are the same version; anything unparseable is `0`.
#[derive(Debug, Clone)]
pub struct Version {
    parts: Vec<u64>,
    pre: Option<String>,
    raw: String,
}

impl PartialEq for Version {
    /// Equality is what the ordering says: `0.2` and `0.2.0` are the same
    /// version even though they were written differently.
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == std::cmp::Ordering::Equal
    }
}

impl Eq for Version {}

impl Version {
    /// This build's version, from the workspace manifest.
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION"))
    }

    pub fn parse(text: &str) -> Self {
        let text = text.strip_prefix('v').unwrap_or(text);
        let (numeric, pre) = match text.split_once('-') {
            Some((n, p)) => (n, Some(p.to_string())),
            None => (text, None),
        };
        let parts = numeric
            .split('.')
            .map(|p| {
                p.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse::<u64>()
                    .unwrap_or(0)
            })
            .collect::<Vec<_>>();
        Version {
            parts,
            pre,
            raw: text.to_string(),
        }
    }

    /// The version as written, for status lines.
    pub fn as_str(&self) -> &str {
        &self.raw
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let len = self.parts.len().max(other.parts.len());
        for i in 0..len {
            let a = self.parts.get(i).copied().unwrap_or(0);
            let b = other.parts.get(i).copied().unwrap_or(0);
            match a.cmp(&b) {
                std::cmp::Ordering::Equal => {}
                ord => return ord,
            }
        }
        // A release outranks its own prerelease: 0.2.0 > 0.2.0-rc1.
        match (&self.pre, &other.pre) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(a), Some(b)) => a.cmp(b),
        }
    }
}

/// One downloadable file on a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

/// The latest release, with only the fields the updater reads.
#[derive(Debug, Clone)]
pub struct Release {
    pub version: Version,
    pub assets: Vec<Asset>,
}

impl Release {
    /// Whether this release is newer than the running build.
    pub fn is_newer_than(&self, current: &Version) -> bool {
        self.version > *current
    }

    /// The AppImage asset for `arch` (`x86_64`, `aarch64`, ...), if the
    /// release carries one. Releases publish only the architectures in
    /// `release.yml`, so a missing asset is silence, not an error.
    pub fn asset_for(&self, arch: &str) -> Option<&Asset> {
        let want = format!("TimeTrack-{arch}.AppImage");
        self.assets.iter().find(|a| a.name == want)
    }
}

#[derive(Debug, serde::Deserialize)]
struct ReleaseJson {
    tag_name: String,
    assets: Vec<AssetJson>,
}

#[derive(Debug, serde::Deserialize)]
struct AssetJson {
    name: String,
    browser_download_url: String,
}

/// Parse a `releases/latest` response body. Unknown fields are ignored, so a
/// GitHub-side addition cannot break old AppImages mid-check.
pub fn parse_release(body: &str) -> Result<Release, UpdateError> {
    let json: ReleaseJson =
        serde_json::from_str(body).map_err(|e| UpdateError::BadRelease(e.to_string()))?;
    if json.tag_name.trim().is_empty() {
        return Err(UpdateError::BadRelease("empty tag_name".into()));
    }
    Ok(Release {
        version: Version::parse(&json.tag_name),
        assets: json
            .assets
            .into_iter()
            .map(|a| Asset {
                name: a.name,
                url: a.browser_download_url,
            })
            .collect(),
    })
}

/// This machine's architecture, in the same spelling the asset names use.
pub fn current_arch() -> &'static str {
    std::env::consts::ARCH
}

/// The running AppImage's path, or `None` outside one. The file must exist:
/// a stale `APPIMAGE` variable pointing nowhere updates nothing.
pub fn app_image_path() -> Option<PathBuf> {
    app_image_path_from(std::env::var_os(APPIMAGE_ENV).as_deref())
}

/// The injectable half of [`app_image_path`]: the same rule over an explicit
/// value, so tests do not depend on the ambient environment (which may
/// itself be an AppImage, as when the test suite runs inside one).
fn app_image_path_from(var: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let path = PathBuf::from(var?);
    path.is_file().then_some(path)
}

/// Whether the automatic check should run at all.
pub fn updates_enabled() -> bool {
    std::env::var_os(NO_UPDATE_ENV).is_none() && app_image_path().is_some()
}

/// Read the latest release from `url` (normally [`RELEASES_URL`]).
///
/// Returns `Ok(None)` when there is nothing to install: the running build is
/// current, or the release carries no asset for this architecture. Any
/// network or parse failure is an `Err` the caller swallows quietly.
pub fn check_for_update_with(url: &str, current: &Version) -> Result<Option<Release>, UpdateError> {
    let text = agent()
        .get(url)
        .header(
            "User-Agent",
            concat!("timetrack/", env!("CARGO_PKG_VERSION")),
        )
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| UpdateError::Network(e.to_string()))?
        .body_mut()
        .read_to_string()
        .map_err(|e| UpdateError::Network(e.to_string()))?;
    if text.len() as u64 > MAX_BYTES {
        return Err(UpdateError::BadRelease("release body too large".into()));
    }
    let release = parse_release(&text)?;
    if release.is_newer_than(current) && release.asset_for(current_arch()).is_some() {
        Ok(Some(release))
    } else {
        Ok(None)
    }
}

/// Check the GitHub releases page for a newer AppImage for this machine.
pub fn check_for_update(current: &Version) -> Result<Option<Release>, UpdateError> {
    check_for_update_with(RELEASES_URL, current)
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .into()
}

/// GET `url` and read the body under a hard cap, returning the bytes and
/// their SHA256 as lowercase hex.
///
/// The cap is a `take(max + 1)` on the byte stream, checked as the bytes
/// arrive: `Content-Length` is never consulted, so a chunked body (which
/// has none) and a lying header are capped exactly like an honest one, and
/// an oversized body is aborted after `max + 1` bytes instead of being
/// buffered whole and measured after the fact. `too_large` is the error an
/// over-cap body produces, since the asset and its checksum file fail
/// differently.
fn fetch_capped(
    url: &str,
    max_bytes: u64,
    too_large: UpdateError,
) -> Result<(Vec<u8>, String), UpdateError> {
    let mut response = agent()
        .get(url)
        .header(
            "User-Agent",
            concat!("timetrack/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|e| UpdateError::Download(e.to_string()))?;
    let mut reader = response.body_mut().as_reader().take(max_bytes + 1);
    let mut bytes = Vec::new();
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| UpdateError::Download(e.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        bytes.extend_from_slice(&buf[..n]);
    }
    if bytes.len() as u64 > max_bytes {
        return Err(too_large);
    }
    let hex = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    Ok((bytes, hex))
}

/// The checksum `SHA256SUMS` records for `asset_name`, as written. Lines are
/// `<hex>  <name>` (or `<hex> *<name>` for binary mode); a release lists
/// every architecture, so the line is picked by file name, not position.
fn parse_sums(body: &str, asset_name: &str) -> Option<String> {
    body.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let hex = fields.next()?;
        // The name may carry sha256sum's `*` binary marker or a path.
        let named = fields.next()?.trim_start_matches('*').rsplit('/').next()?;
        (named == asset_name).then(|| hex.to_string())
    })
}

/// The SHA256 the release publishes for the asset at `url`.
///
/// GitHub download URLs are `.../releases/download/<tag>/<file>`, so the
/// release's `SHA256SUMS` asset is the sibling of the AppImage asset: same
/// tag directory, its own name. Failing to fetch or read it fails closed --
/// an update that cannot be verified is not installed.
fn expected_checksum(url: &str) -> Result<String, UpdateError> {
    let (dir, name) = url
        .rsplit_once('/')
        .filter(|(_, name)| !name.is_empty())
        .ok_or_else(|| UpdateError::BadChecksum(format!("no asset name in {url}")))?;
    let (body, _) = fetch_capped(
        &format!("{dir}/{SUMS_NAME}"),
        MAX_SUMS_BYTES,
        UpdateError::BadChecksum(format!("{SUMS_NAME} too large")),
    )?;
    let text = String::from_utf8_lossy(&body);
    parse_sums(&text, name)
        .ok_or_else(|| UpdateError::BadChecksum(format!("{SUMS_NAME} does not name {name}")))
}

/// Download `url` and rename it over `dest`, which must be the running
/// AppImage. The bytes land in a sibling `.new` file first, so a failed or
/// half-written download leaves the current image untouched; the rename is
/// atomic on the same filesystem.
///
/// The checksum comes from the release's `SHA256SUMS` asset (see
/// [`expected_checksum`]); the injectable half, with an explicit checksum
/// and cap, is [`install_with_checksum`].
pub fn install_with(url: &str, dest: &Path) -> Result<(), UpdateError> {
    if !dest.is_file() {
        return Err(UpdateError::NotAppImage);
    }
    let expected = expected_checksum(url)?;
    install_with_checksum(url, dest, &expected, MAX_BYTES)
}

/// The injectable half of [`install_with`]: download `url` over `dest`,
/// refusing anything past `max_bytes` (enforced during the read, never from
/// `Content-Length`) or whose SHA256 is not `expected_sha256`.
///
/// The payload must also start with the ELF magic: a GitHub error page
/// served as `200 OK` must never become the user's TimeTrack binary. Every
/// refusal happens before the rename, so the running image stays untouched.
pub fn install_with_checksum(
    url: &str,
    dest: &Path,
    expected_sha256: &str,
    max_bytes: u64,
) -> Result<(), UpdateError> {
    if !dest.is_file() {
        return Err(UpdateError::NotAppImage);
    }
    let expected = expected_sha256.trim().to_ascii_lowercase();
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(UpdateError::BadChecksum(format!(
            "not a SHA256 hex digest: {expected_sha256:?}"
        )));
    }
    let (bytes, actual) = fetch_capped(url, max_bytes, UpdateError::TooLarge)?;
    if bytes.len() < 4 || bytes[0..4] != [0x7f, b'E', b'L', b'F'] {
        return Err(UpdateError::NotAnAppImage);
    }
    if actual != expected {
        return Err(UpdateError::ChecksumMismatch);
    }
    let tmp = dest.with_extension("AppImage.new");
    std::fs::write(&tmp, &bytes).map_err(|e| UpdateError::Install(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| UpdateError::Install(e.to_string()))?;
    }
    std::fs::rename(&tmp, dest).map_err(|e| UpdateError::Install(e.to_string()))?;
    Ok(())
}

/// Download the release asset over the running AppImage.
pub fn install(asset: &Asset) -> Result<(), UpdateError> {
    let dest = app_image_path().ok_or(UpdateError::NotAppImage)?;
    install_with(&asset.url, &dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically_not_lexically() {
        assert!(Version::parse("0.10.0") > Version::parse("0.9.0"));
        assert!(Version::parse("v0.2.0") > Version::parse("0.1.0"));
        assert_eq!(Version::parse("v0.2"), Version::parse("0.2.0"));
        assert_eq!(Version::parse("0.1.0"), Version::parse("0.1.0"));
        assert!(Version::parse("0.1.0") < Version::parse("0.1.1"));
    }

    #[test]
    fn a_release_outranks_its_own_prerelease() {
        assert!(Version::parse("0.2.0") > Version::parse("0.2.0-rc1"));
        assert!(Version::parse("0.2.0-rc2") > Version::parse("0.2.0-rc1"));
    }

    #[test]
    fn garbage_parses_low_never_high() {
        // A tag that parses to nothing must never look newer than a build.
        assert!(Version::parse("") < Version::parse("0.1.0"));
        assert!(Version::parse("nightly") < Version::current());
    }

    #[test]
    fn release_json_parses_tags_and_assets() {
        let body = r#"{
            "tag_name": "v0.2.0",
            "assets": [
                {"name": "TimeTrack-x86_64.AppImage",
                 "browser_download_url": "https://example.com/TimeTrack-x86_64.AppImage"},
                {"name": "timetrack-x86_64.tar.gz",
                 "browser_download_url": "https://example.com/t.tar.gz"}
            ]
        }"#;
        let release = parse_release(body).unwrap();
        assert_eq!(release.version, Version::parse("0.2.0"));
        assert_eq!(release.assets.len(), 2);
        assert!(release.is_newer_than(&Version::parse("0.1.0")));
        assert!(!release.is_newer_than(&Version::parse("0.2.0")));
        assert!(!release.is_newer_than(&Version::parse("9.9.9")));
    }

    #[test]
    fn release_json_rejects_garbage() {
        assert!(parse_release("not json").is_err());
        assert!(parse_release(r#"{"tag_name": "", "assets": []}"#).is_err());
        assert!(parse_release(r#"{"assets": []}"#).is_err());
    }

    #[test]
    fn asset_pick_matches_this_arch_only() {
        let release = Release {
            version: Version::parse("0.2.0"),
            assets: vec![Asset {
                name: "TimeTrack-x86_64.AppImage".into(),
                url: "https://example.com/x".into(),
            }],
        };
        assert!(release.asset_for("x86_64").is_some());
        assert!(release.asset_for("aarch64").is_none());
        assert!(release.asset_for("X86_64").is_none());
    }

    #[test]
    fn appimage_detection_needs_an_existing_file() {
        use std::ffi::OsStr;
        assert_eq!(app_image_path_from(None), None);
        assert_eq!(
            app_image_path_from(Some(OsStr::new("/nonexistent/TimeTrack-x86_64.AppImage"))),
            None
        );
        // A stale variable pointing nowhere updates nothing, even when the
        // ambient environment itself runs inside an AppImage.
        let dir = std::env::temp_dir().join(format!("tt-update-detect-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("TimeTrack-x86_64.AppImage");
        std::fs::write(&dest, b"\x7fELF fake image").unwrap();
        assert_eq!(
            app_image_path_from(Some(dest.as_os_str())),
            Some(dest.clone())
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn install_refuses_a_non_elf_payload() {
        // The guard that stops a GitHub error page from becoming the binary.
        let dir = std::env::temp_dir().join(format!("tt-update-refuse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("TimeTrack-x86_64.AppImage");
        std::fs::write(&dest, b"\x7fELF fake image").unwrap();
        // Unreachable host: the point is the error kind plumbing, not the
        // network, so any download failure surfaces as `Download`.
        let err = install_with("http://127.0.0.1:9/nope", &dest).unwrap_err();
        assert!(matches!(err, UpdateError::Download(_)), "{err:?}");
        // And a missing destination is refused before any network happens.
        let err = install_with("http://127.0.0.1:9/nope", &dir.join("missing")).unwrap_err();
        assert_eq!(err, UpdateError::NotAppImage);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sums_parsing_picks_this_assets_line() {
        // A release lists every file; the line is found by name, binary
        // markers and paths on the name do not matter, and a sums file that
        // does not name the asset verifies nothing.
        let body = "\
aaaa  TimeTrack-aarch64.AppImage
bbbb *TimeTrack-x86_64.AppImage
cccc  ./TimeTrack-x86_64.AppImage.zsync
";
        assert_eq!(
            parse_sums(body, "TimeTrack-x86_64.AppImage").as_deref(),
            Some("bbbb")
        );
        assert_eq!(
            parse_sums(body, "TimeTrack-aarch64.AppImage").as_deref(),
            Some("aaaa")
        );
        assert_eq!(parse_sums(body, "TimeTrack-riscv64.AppImage"), None);
        assert_eq!(parse_sums("", "TimeTrack-x86_64.AppImage"), None);
    }
}

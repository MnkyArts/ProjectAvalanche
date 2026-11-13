use std::sync::{Arc, OnceLock};
use std::time::Duration;

use semver::Version;
use serde::Deserialize;
use ureq::Agent;
use ureq::tls::{Certificate, RootCerts, TlsConfig};

use super::{Asset, LATEST_URL, REPO, Release, asset_name, tag_url};

const MAX_API_BYTES: u64 = 1024 * 1024;
const MAX_HEADERS: usize = 16 * 1024;
pub(crate) const MAX_ARCHIVE: u64 = 512 * 1024 * 1024;
// PORT(choice): 30 s for the API and 10 s to connect; tuning: the checks against GitHub from slow links (a release is a
// few kilobytes, the archive is streamed separately); the original has no launcher.
const API_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Debug, Deserialize)]
struct ApiAsset {
    name: String,
    size: u64,
    #[serde(default)]
    digest: Option<String>,
    browser_download_url: String,
}

pub fn check(current: &Version) -> Result<Option<Release>, String> {
    let body = fetch(LATEST_URL, "the project has no published release yet")?;
    read_release(&body, Want::Newer(current))
}

pub fn release_of(current: &Version) -> Result<Release, String> {
    let missing = format!("GitHub has no release v{current}: reinstall this launcher from the project's releases page");
    let body = fetch(&tag_url(current), &missing)?;
    read_release(&body, Want::Same(current))?.ok_or(missing)
}

#[derive(Clone, Copy, Debug)]
enum Want<'a> {
    Newer(&'a Version),
    Same(&'a Version),
}

fn fetch(url: &str, not_found: &str) -> Result<Vec<u8>, String> {
    let agent = agent(API_TIMEOUT, CONNECT_TIMEOUT, 3)?;
    let mut response = agent
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(|e| format!("GitHub could not be asked: {e}"))?;
    let status = response.status().as_u16();
    match status {
        200 => {}
        404 => return Err(not_found.into()),
        403 | 429 => return Err("GitHub asks the launcher to wait (rate limit); try again later".into()),
        other => return Err(format!("GitHub answered HTTP {other}")),
    }
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_API_BYTES + 1)
        .read_to_vec()
        .map_err(|e| format!("GitHub's answer does not read: {e}"))?;
    Ok(body)
}

fn read_release(body: &[u8], want: Want<'_>) -> Result<Option<Release>, String> {
    if body.len() as u64 > MAX_API_BYTES {
        return Err("GitHub's answer is larger than a release should be".into());
    }
    let api: ApiRelease = serde_json::from_slice(body).map_err(|e| format!("GitHub's answer is not a release: {e}"))?;
    let tag = ssx_core::text::quoted(&api.tag_name);
    if api.draft {
        return match want {
            Want::Newer(_) => Ok(None),
            Want::Same(_) => Err(format!("the release {tag} is a draft")),
        };
    }
    let version = parse_tag(&api.tag_name).ok_or_else(|| format!("the release's tag {tag} is not a version"))?;
    match want {
        Want::Newer(current) if !is_newer(&version, current) => return Ok(None),
        Want::Same(current) if version != *current => {
            return Err(format!("GitHub's release {tag} is not this launcher's version (v{current})"));
        }
        _ => {}
    }
    let name = asset_name();
    let asset = api
        .assets
        .iter()
        .find(|a| a.name == name)
        .ok_or_else(|| format!("release {} has no {name} for this system", api.tag_name))?;
    if asset.size == 0 || asset.size > MAX_ARCHIVE {
        return Err(format!("the release's {name} is {} bytes, which the launcher does not accept", asset.size));
    }
    validate_download_url(&asset.browser_download_url, &api.tag_name, &asset.name)?;
    let sha256 = parse_digest(asset.digest.as_deref())?;
    Ok(Some(Release {
        version,
        tag: api.tag_name,
        title: api.name.unwrap_or_default(),
        notes: api.body.unwrap_or_default(),
        page: api.html_url.filter(|u| u.starts_with("https://github.com/")).unwrap_or_default(),
        asset: Asset { name: asset.name.clone(), url: asset.browser_download_url.clone(), size: asset.size, sha256 },
        reinstall: Vec::new(),
    }))
}

pub(crate) fn agent(global: Duration, connect: Duration, redirects: u32) -> Result<Agent, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let roots = system_roots();
    if roots.is_empty() {
        return Err("no trusted certificates were found on this system".into());
    }
    let tls = TlsConfig::builder().root_certs(RootCerts::Specific(roots)).build();
    let user_agent = format!("ProjectAvalanche-Launcher/{}", env!("CARGO_PKG_VERSION"));
    Ok(Agent::config_builder()
        .timeout_global(Some(global))
        .timeout_connect(Some(connect))
        .max_response_header_size(MAX_HEADERS)
        .http_status_as_error(false)
        .max_redirects(redirects)
        .proxy(None)
        .user_agent(&user_agent)
        .tls_config(tls)
        .build()
        .into())
}

fn system_roots() -> Arc<Vec<Certificate<'static>>> {
    static ROOTS: OnceLock<Arc<Vec<Certificate<'static>>>> = OnceLock::new();
    Arc::clone(ROOTS.get_or_init(|| {
        let found = rustls_native_certs::load_native_certs();
        Arc::new(found.certs.iter().map(|c| Certificate::from_der(c.as_ref()).to_owned()).collect())
    }))
}

pub(crate) fn check_final_url(uri: &ureq::http::Uri) -> Result<(), String> {
    if uri.scheme_str() != Some("https") {
        return Err(format!("the download left HTTPS ({uri})"));
    }
    match uri.host() {
        Some("github.com" | "objects.githubusercontent.com" | "release-assets.githubusercontent.com") => Ok(()),
        Some(other) => Err(format!("the download came from an unexpected host: {other}")),
        None => Err("the download has no host".into()),
    }
}

fn parse_tag(tag: &str) -> Option<Version> {
    Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()
}

fn is_newer(latest: &Version, current: &Version) -> bool {
    latest > current && (latest.pre.is_empty() || !current.pre.is_empty())
}

fn validate_download_url(url: &str, tag: &str, name: &str) -> Result<(), String> {
    let expected = format!("https://github.com/{REPO}/releases/download/{tag}/{name}");
    if url != expected {
        return Err(format!("the release's download address is not {expected}"));
    }
    Ok(())
}

fn parse_digest(digest: Option<&str>) -> Result<[u8; 32], String> {
    let digest = digest.ok_or("GitHub published no SHA-256 for the file; refusing an unverified update")?;
    let hex = digest.strip_prefix("sha256:").ok_or_else(|| format!("the file's digest is not SHA-256: {digest}"))?;
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("the file's SHA-256 is not 64 hex digits: {digest}"));
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).map_err(|_| format!("bad SHA-256: {digest}"))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_versions() {
        assert_eq!(parse_tag("v0.2.0"), Some(Version::new(0, 2, 0)));
        assert_eq!(parse_tag("0.2.0"), Some(Version::new(0, 2, 0)));
        assert_eq!(parse_tag("release-2"), None);
        assert_eq!(parse_tag("v0.2"), None);
    }

    #[test]
    fn only_a_newer_version_is_offered() {
        let v = |s: &str| Version::parse(s).unwrap();
        assert!(is_newer(&v("0.2.0"), &v("0.1.0")));
        assert!(!is_newer(&v("0.2.0"), &v("0.2.0")));
        assert!(!is_newer(&v("0.1.9"), &v("0.2.0")));
        assert!(is_newer(&v("0.3.0-beta.1"), &v("0.3.0-beta.0")));
        assert!(!is_newer(&v("0.3.0-beta.1"), &v("0.2.0")));
    }

    #[test]
    fn a_digest_is_sha256_or_nothing() {
        let hex = "0123456789abcdef".repeat(4);
        assert!(parse_digest(Some(&format!("sha256:{hex}"))).is_ok());
        assert!(parse_digest(None).is_err());
        assert!(parse_digest(Some(&format!("sha1:{hex}"))).is_err());
        assert!(parse_digest(Some("sha256:abc")).is_err());
        assert!(parse_digest(Some(&format!("sha256:{}", "z".repeat(64)))).is_err());
    }

    #[test]
    fn the_download_address_is_the_release_file() {
        let tag = "v0.2.0";
        let name = "ProjectAvalanche-linux-x64.tar.gz";
        let good = format!("https://github.com/{REPO}/releases/download/{tag}/{name}");
        assert!(validate_download_url(&good, tag, name).is_ok());
        assert!(validate_download_url(&good.replace("github.com", "example.com"), tag, name).is_err());
        assert!(validate_download_url(&good.replace(name, "other.tar.gz"), tag, name).is_err());
        assert!(validate_download_url(&good.replace(tag, "v9.9.9"), tag, name).is_err());
    }

    fn answer(tag: &str, draft: bool) -> Vec<u8> {
        let name = asset_name();
        serde_json::json!({
            "tag_name": tag, "name": tag, "body": "notes", "draft": draft,
            "html_url": format!("https://github.com/{REPO}/releases/tag/{tag}"),
            "assets": [{
                "name": name, "size": 42,
                "digest": format!("sha256:{}", "ab".repeat(32)),
                "browser_download_url": format!("https://github.com/{REPO}/releases/download/{tag}/{name}"),
            }]
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn a_repair_takes_this_versions_own_release() {
        let v = |s: &str| Version::parse(s).unwrap();
        let current = v("0.3.0");
        let same = read_release(&answer("v0.3.0", false), Want::Same(&current)).unwrap().unwrap();
        assert_eq!((same.version.clone(), same.asset.sha256), (current.clone(), [0xab; 32]));
        assert!(same.reinstall.is_empty());
        assert!(read_release(&answer("v0.3.1", false), Want::Same(&current)).unwrap_err().contains("not this"));
        assert!(read_release(&answer("v0.3.0", true), Want::Same(&current)).unwrap_err().contains("draft"));
        let mut moved = String::from_utf8(answer("v0.3.0", false)).unwrap();
        moved = moved.replace("https://github.com/", "https://example.com/");
        assert!(read_release(moved.as_bytes(), Want::Same(&current)).is_err(), "another address");
        let unsigned = String::from_utf8(answer("v0.3.0", false)).unwrap().replace("sha256:", "md5:");
        assert!(read_release(unsigned.as_bytes(), Want::Same(&current)).is_err(), "no SHA-256");
        assert!(read_release(&answer("v0.3.0", false), Want::Newer(&current)).unwrap().is_none());
        assert!(read_release(&answer("v0.3.1", true), Want::Newer(&current)).unwrap().is_none());
        assert_eq!(read_release(&answer("v0.3.1", false), Want::Newer(&current)).unwrap().unwrap().version, v("0.3.1"));
        assert!(read_release(&vec![b' '; MAX_API_BYTES as usize + 1], Want::Newer(&current)).is_err());
        assert_eq!(
            super::super::tag_url(&current),
            format!("https://api.github.com/repos/{REPO}/releases/tags/v0.3.0")
        );
    }

    #[test]
    fn a_release_answer_is_read() {
        let json = r#"{
            "tag_name": "v0.2.0", "name": "0.2.0", "body": "notes", "draft": false,
            "html_url": "https://github.com/x/y/releases/tag/v0.2.0",
            "assets": [{
                "name": "ProjectAvalanche-linux-x64.tar.gz", "size": 42,
                "digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "browser_download_url": "https://github.com/x/y/releases/download/v0.2.0/a.tar.gz"
            }]
        }"#;
        let api: ApiRelease = serde_json::from_str(json).unwrap();
        assert_eq!((api.tag_name.as_str(), api.assets.len()), ("v0.2.0", 1));
        assert_eq!(api.assets[0].size, 42);
        assert!(api.assets[0].digest.is_some());
    }
}

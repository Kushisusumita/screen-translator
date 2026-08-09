use tracing::{info, warn};

use crate::shared::i18n::t;
use crate::shared::logging::clip;

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub url: String,
    pub size: u64,
}

const RELEASES: &str =
    "https://api.github.com/repos/Kushisusumita/screen-translator/releases/latest";

/// Hosts a release asset is allowed to come from.
///
/// The download URL arrives from a network response and is then handed to a
/// routine that overwrites the running executable, so it does not get to point
/// anywhere it likes.
pub const ALLOWED_HOSTS: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

pub fn url_is_allowed(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split('/').next().unwrap_or("");
    // Strip any userinfo — `https://github.com@evil.example/x` is not GitHub.
    if host.contains('@') {
        return false;
    }
    let host = host.split(':').next().unwrap_or("");
    ALLOWED_HOSTS.contains(&host)
}

pub async fn check_for_update() -> Result<Option<UpdateInfo>, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("screen-translator/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .get(RELEASES)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| {
            // The URL and the transport error belong in the log. "Check"
            // returning a GitHub API address at the user helps nobody.
            warn!(error = %e, "Update check could not reach GitHub");
            t("Could not reach the update server").to_string()
        })?;

    let status = resp.status();
    if status.as_u16() == 404 {
        info!("No GitHub releases published yet");
        return Ok(None);
    }
    if !status.is_success() {
        warn!(status = status.as_u16(), "Update check refused");
        return Err(t("Could not check for updates right now").to_string());
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    let tag = json["tag_name"]
        .as_str()
        .ok_or_else(|| t("The release has no tag_name").to_string())?;
    let release_ver = tag.trim_start_matches('v');
    info!(
        latest = release_ver,
        current = env!("CARGO_PKG_VERSION"),
        "Checked for updates"
    );

    if !version_is_newer(release_ver, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }

    let assets = json["assets"]
        .as_array()
        .ok_or_else(|| t("The release has no files").to_string())?;

    let asset = assets
        .iter()
        .find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(".exe")))
        .ok_or_else(|| t("The release has no .exe file").to_string())?;

    let url = asset["browser_download_url"]
        .as_str()
        .ok_or_else(|| t("The file has no download link").to_string())?
        .to_string();

    if !url_is_allowed(&url) {
        return Err(t(
            "The update link does not point to GitHub, so the install was cancelled: {url}",
        )
        .replace("{url}", clip(&url, 120)));
    }

    let size = asset["size"].as_u64().unwrap_or(0);

    Ok(Some(UpdateInfo {
        version: release_ver.to_string(),
        url,
        size,
    }))
}

/// Compares `major.minor.patch`, ignoring any pre-release suffix.
///
/// `1.2.3-rc1` and `1.2.3` compare equal, which is the conservative answer: a
/// release candidate is not offered as an upgrade over the final build.
fn version_is_newer(new: &str, current: &str) -> bool {
    parse_version(new) > parse_version(current)
}

fn parse_version(v: &str) -> (u32, u32, u32) {
    let core = v
        .trim()
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()
        .unwrap_or("");
    let mut it = core.split('.');
    let mut next = || {
        it.next()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0)
    };
    (next(), next(), next())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_version_comparison() {
        assert!(version_is_newer("0.2.0", "0.1.9"));
        assert!(version_is_newer("1.0.0", "0.99.99"));
        assert!(!version_is_newer("0.1.0", "0.1.0"));
        assert!(!version_is_newer("0.0.1", "0.1.0"));
    }

    #[test]
    fn a_v_prefix_on_either_side_is_ignored() {
        assert!(version_is_newer("v2.0.0", "1.0.0"));
        assert!(!version_is_newer("v1.0.0", "v1.0.0"));
    }

    #[test]
    fn a_prerelease_is_not_offered_over_the_matching_release() {
        assert!(!version_is_newer("1.0.0-rc1", "1.0.0"));
        assert!(version_is_newer("1.0.1-rc1", "1.0.0"));
    }

    #[test]
    fn a_short_or_junk_tag_does_not_panic() {
        assert!(version_is_newer("2", "1.9.9"));
        assert!(!version_is_newer("bananas", "0.1.0"));
    }

    #[test]
    fn only_github_hosts_are_accepted() {
        assert!(url_is_allowed(
            "https://github.com/Kushisusumita/screen-translator/releases/download/v1/a.exe"
        ));
        assert!(url_is_allowed(
            "https://objects.githubusercontent.com/x.exe"
        ));
    }

    #[test]
    fn plain_http_is_refused() {
        assert!(!url_is_allowed("http://github.com/a.exe"));
    }

    #[test]
    fn a_lookalike_host_is_refused() {
        assert!(!url_is_allowed("https://github.com.evil.example/a.exe"));
        assert!(!url_is_allowed("https://notgithub.com/a.exe"));
        assert!(!url_is_allowed("https://evil.example/github.com/a.exe"));
    }

    #[test]
    fn userinfo_cannot_be_used_to_fake_the_host() {
        assert!(!url_is_allowed("https://github.com@evil.example/a.exe"));
    }
}

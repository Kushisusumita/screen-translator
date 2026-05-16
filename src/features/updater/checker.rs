use tracing::info;

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub url: String,
}

/// Checks GitHub releases for a version newer than the running binary.
/// Returns `Ok(None)` when already up to date or no releases exist.
pub async fn check_for_update() -> Result<Option<UpdateInfo>, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("screen-translator/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .get("https://api.github.com/repos/Kushisusumita/screen-translator/releases/latest")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    let status = resp.status();
    if status.as_u16() == 404 {
        info!("No GitHub releases found");
        return Ok(None);
    }
    if !status.is_success() {
        return Err(format!("GitHub API returned HTTP {}", status));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    let tag = json["tag_name"]
        .as_str()
        .ok_or_else(|| "No tag_name in release".to_string())?;
    let release_ver = tag.trim_start_matches('v');
    info!("Latest release: {}  current: {}", release_ver, env!("CARGO_PKG_VERSION"));

    if !version_is_newer(release_ver, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }

    let assets = json["assets"]
        .as_array()
        .ok_or_else(|| "No assets in release".to_string())?;

    let exe_asset = assets
        .iter()
        .find(|a| a["name"].as_str().map(|n| n.ends_with(".exe")).unwrap_or(false))
        .ok_or_else(|| "No .exe asset found in the latest release".to_string())?;

    let url = exe_asset["browser_download_url"]
        .as_str()
        .ok_or_else(|| "Missing download URL".to_string())?
        .to_string();

    Ok(Some(UpdateInfo { version: release_ver.to_string(), url }))
}

fn version_is_newer(new: &str, current: &str) -> bool {
    let parse = |v: &str| -> (u32, u32, u32) {
        let parts: Vec<&str> = v.trim_start_matches('v').splitn(3, '.').collect();
        let n = |i: usize| -> u32 { parts.get(i).and_then(|s| s.parse().ok()).unwrap_or(0) };
        (n(0), n(1), n(2))
    };
    parse(new) > parse(current)
}

use std::os::windows::process::CommandExt;
use tracing::info;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Downloads the new binary, writes a self-replacing batch script, launches it,
/// then exits the current process so the script can overwrite the running exe.
/// On failure returns an error string (process is NOT exited).
pub async fn download_and_apply(url: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("screen-translator/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;

    info!("Downloading update from {}", url);
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Download failed: HTTP {}", resp.status()));
    }

    let bytes = resp.bytes().await.map_err(|e| format!("Read failed: {}", e))?;

    let temp = std::env::temp_dir();
    let new_exe = temp.join("screen-translator-update.exe");
    std::fs::write(&new_exe, &bytes)
        .map_err(|e| format!("Failed to save update file: {}", e))?;

    let current_exe = std::env::current_exe()
        .map_err(|e| format!("Cannot find current exe: {}", e))?;

    let pid = std::process::id();
    let batch_path = temp.join("screen-translator-update.bat");

    // Wait for PID to disappear, copy new exe over old one, then restart.
    let script = format!(
        "@echo off\r\n\
         :wait\r\n\
         tasklist /fi \"pid eq {pid}\" 2>NUL | find \"{pid}\" >NUL\r\n\
         if not errorlevel 1 (\r\n\
             timeout /t 1 /nobreak >NUL\r\n\
             goto wait\r\n\
         )\r\n\
         copy /y \"{new}\" \"{cur}\"\r\n\
         start \"\" \"{cur}\"\r\n\
         del \"%~f0\"\r\n",
        pid = pid,
        new = new_exe.display(),
        cur = current_exe.display(),
    );
    std::fs::write(&batch_path, script.as_bytes())
        .map_err(|e| format!("Failed to write update script: {}", e))?;

    info!("Launching update script: {}", batch_path.display());
    std::process::Command::new("cmd")
        .args(["/c", batch_path.to_str().unwrap_or("")])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("Failed to launch update script: {}", e))?;

    // Hard exit — the batch script will restart the app after copying.
    std::process::exit(0);
}

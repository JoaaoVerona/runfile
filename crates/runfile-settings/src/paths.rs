use std::path::PathBuf;

/// Get the platform-appropriate settings directory for Runfile.
///
/// - Linux/macOS: `~/.config/runfile/`
/// - Windows: `%APPDATA%\runfile\`
pub fn settings_dir() -> Option<PathBuf> {
	dirs::config_dir().map(|d| d.join("runfile"))
}

/// Get the full path to the settings file.
pub fn settings_file_path() -> Option<PathBuf> {
	settings_dir().map(|d| d.join("settings.json"))
}

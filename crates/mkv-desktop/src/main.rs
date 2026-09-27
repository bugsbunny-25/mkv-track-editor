mod backend;
mod file_drop;

use backend::DesktopBackend;
use mkv_core::{config_store, ToolPaths};
use std::path::PathBuf;

/// `MKV_CONFIG_DIR` if set, else the OS config directory
/// (e.g. `~/Library/Application Support/mkv-track-editor` on macOS,
/// `~/.config/mkv-track-editor` on Linux).
fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MKV_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    directories::ProjectDirs::from("", "", "mkv-track-editor")
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Apps started from Finder or the Dock get a bare PATH (/usr/bin:/bin:…),
/// and LaunchServices doesn't always apply the bundle's LSEnvironment, so
/// Homebrew's mkvmerge/mediainfo wouldn't be found. Append Homebrew's bin
/// directories when they're missing.
#[cfg(target_os = "macos")]
fn add_homebrew_to_path() {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let mut paths: Vec<PathBuf> = std::env::split_paths(&current).collect();
    for dir in ["/opt/homebrew/bin", "/usr/local/bin"] {
        if !paths.iter().any(|p| p == std::path::Path::new(dir)) {
            paths.push(PathBuf::from(dir));
        }
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        // Called first thing in `main`, before any other thread (the tokio
        // runtime included) exists.
        std::env::set_var("PATH", joined);
    }
}

/// Reports a fatal startup error on stderr and, since a Finder launch has no
/// terminal, in a dialog.
fn fatal(message: &str) -> ! {
    eprintln!("error: {message}");
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("MKV Track Editor can't start")
        .set_description(message)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
    std::process::exit(1);
}

fn main() {
    #[cfg(target_os = "macos")]
    add_homebrew_to_path();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to start background runtime");

    let tools = ToolPaths::from_env();
    match rt.block_on(mkv_core::check_dependencies(&tools)) {
        Ok(warnings) => {
            for warning in warnings {
                tracing::warn!("{warning}");
            }
        }
        Err(e) => fatal(&format!(
            "{e}\n\nmkvmerge and mkvpropedit (MKVToolNix) and mediainfo (MediaInfo CLI) must be installed and on \
             PATH (or set MKV_MKVMERGE_BIN / MKV_MKVPROPEDIT_BIN / MKV_MEDIAINFO_BIN)."
        )),
    }

    let config_path = config_store::config_file(&config_dir());
    tracing::info!(config = %config_path.display(), "using settings file");
    let backend = DesktopBackend::new(rt.handle().clone(), tools, config_path);

    let result = mkv_ui::App::new(backend).and_then(|app| {
        file_drop::install(&app);
        app.run()
    });
    if let Err(e) = result {
        tracing::error!(error = %e, "UI exited with an error");
        std::process::exit(1);
    }
}

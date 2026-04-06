mod path;

pub(crate) use path::find_binary_on_path;

#[cfg(target_os = "linux")]
mod flatpak;
#[cfg(target_os = "linux")]
pub(crate) use flatpak::flatpak_app_installed;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::{macos_app_executable, macos_app_exists, macos_common_cli_candidates};

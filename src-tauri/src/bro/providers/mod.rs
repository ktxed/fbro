use crate::bro::model::{BroInputItem, LaunchArg, ResolvedBro};

pub mod copy_paths;
pub mod extract;
pub mod fedup;
pub mod kill_proc;
#[cfg(target_os = "linux")]
pub mod linux_media;
#[cfg(target_os = "macos")]
pub mod macos_media;
pub mod seven_zip;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod tar;
#[cfg(target_os = "windows")]
pub mod windows_media;
pub mod zip;

pub struct BroDiscoveryContext<'a> {
    pub inputs: &'a [BroInputItem],
    pub suggested_output_dir: Option<&'a str>,
    pub source_id: Option<&'a str>,
}

pub trait BroProvider: Send + Sync {
    fn list(&self, ctx: &BroDiscoveryContext<'_>) -> Vec<ResolvedBro>;
}

pub(super) fn mpv_playlist_args() -> Vec<LaunchArg> {
    vec![
        LaunchArg::Literal("--force-window=yes".to_string()),
        LaunchArg::TargetPlaylistFile {
            flag: "--playlist=".to_string(),
        },
    ]
}

pub fn builtin_providers() -> Vec<Box<dyn BroProvider>> {
    let mut providers: Vec<Box<dyn BroProvider>> = Vec::new();

    providers.push(Box::new(copy_paths::CopyPathsProvider));
    providers.push(Box::new(fedup::FedupProvider));
    providers.push(Box::new(kill_proc::KillProcProvider));
    #[cfg(target_os = "linux")]
    providers.push(Box::new(linux_media::LinuxMediaProvider));
    #[cfg(target_os = "macos")]
    providers.push(Box::new(macos_media::MacOSMediaProvider));
    #[cfg(target_os = "windows")]
    providers.push(Box::new(windows_media::WindowsMediaProvider));
    providers.push(Box::new(seven_zip::SevenZipProvider));
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    providers.push(Box::new(tar::TarProvider));
    providers.push(Box::new(zip::ZipProvider));
    providers.push(Box::new(extract::ExtractProvider));

    providers
}

#[cfg(test)]
mod tests {
    use super::mpv_playlist_args;
    use crate::bro::model::LaunchArg;

    #[test]
    fn mpv_forces_a_window_before_opening_the_playlist() {
        assert_eq!(
            mpv_playlist_args(),
            vec![
                LaunchArg::Literal("--force-window=yes".to_string()),
                LaunchArg::TargetPlaylistFile {
                    flag: "--playlist=".to_string(),
                },
            ]
        );
    }
}

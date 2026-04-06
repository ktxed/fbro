use crate::bro::model::{BroInputItem, ResolvedBro};

#[cfg(target_os = "linux")]
pub mod linux_media;
#[cfg(target_os = "macos")]
pub mod macos_media;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod seven_zip;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod tar;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod zip;

pub struct BroDiscoveryContext<'a> {
    pub inputs: &'a [BroInputItem],
    pub suggested_output_dir: Option<&'a str>,
}

pub trait BroProvider: Send + Sync {
    fn list(&self, ctx: &BroDiscoveryContext<'_>) -> Vec<ResolvedBro>;
}

pub fn builtin_providers() -> Vec<Box<dyn BroProvider>> {
    let mut providers: Vec<Box<dyn BroProvider>> = Vec::new();

    #[cfg(target_os = "linux")]
    providers.push(Box::new(linux_media::LinuxMediaProvider));
    #[cfg(target_os = "macos")]
    providers.push(Box::new(macos_media::MacOSMediaProvider));
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    providers.push(Box::new(seven_zip::SevenZipProvider));
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    providers.push(Box::new(tar::TarProvider));
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    providers.push(Box::new(zip::ZipProvider));

    providers
}

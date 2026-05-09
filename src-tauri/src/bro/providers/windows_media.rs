use std::env;
use std::ffi::c_void;
use std::path::PathBuf;

use crate::bro::model::{
    BroOption, BroPre, BroProtocol, BroSource, LaunchArg, LaunchRecipe, ResolvedBro,
};
use crate::bro::probe::find_binary_on_path;
use crate::bro::providers::{BroDiscoveryContext, BroProvider};
use crate::bro::version::native_version;

// ── PE VERSIONINFO reader (version.dll) ─────────────────────────────────────

#[link(name = "version")]
extern "system" {
    fn GetFileVersionInfoSizeW(lptstr_filename: *const u16, lpdw_handle: *mut u32) -> u32;
    fn GetFileVersionInfoW(
        lptstr_filename: *const u16,
        dw_handle: u32,
        dw_len: u32,
        lp_data: *mut c_void,
    ) -> i32;
    fn VerQueryValueW(
        p_block: *const c_void,
        lp_sub_block: *const u16,
        lp_buffer: *mut *mut c_void,
        pu_len: *mut u32,
    ) -> i32;
}

#[repr(C)]
struct VsFixedFileInfo {
    dw_signature: u32,
    dw_struc_version: u32,
    dw_file_version_ms: u32,
    dw_file_version_ls: u32,
    _rest: [u32; 9],
}

fn pe_file_version(path: &str) -> Option<String> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut dummy = 0u32;
    let size = unsafe { GetFileVersionInfoSizeW(wide.as_ptr(), &mut dummy) };
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    let ok = unsafe {
        GetFileVersionInfoW(wide.as_ptr(), 0, size, buf.as_mut_ptr() as *mut c_void)
    };
    if ok == 0 {
        return None;
    }
    let query: Vec<u16> = "\\".encode_utf16().chain(std::iter::once(0)).collect();
    let mut info_ptr: *mut c_void = std::ptr::null_mut();
    let mut info_len = 0u32;
    let ok = unsafe {
        VerQueryValueW(
            buf.as_ptr() as *const c_void,
            query.as_ptr(),
            &mut info_ptr,
            &mut info_len,
        )
    };
    if ok == 0 || info_ptr.is_null() {
        return None;
    }
    let info = unsafe { &*(info_ptr as *const VsFixedFileInfo) };
    let major = (info.dw_file_version_ms >> 16) & 0xffff;
    let minor = info.dw_file_version_ms & 0xffff;
    let patch = (info.dw_file_version_ls >> 16) & 0xffff;
    Some(format!("{major}.{minor}.{patch}"))
}

pub struct WindowsMediaProvider;

impl BroProvider for WindowsMediaProvider {
    fn list(&self, ctx: &BroDiscoveryContext<'_>) -> Vec<ResolvedBro> {
        let _ = ctx.inputs;
        let mut bros = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        for program in find_exe("mpv.exe", &mpv_install_candidates()) {
            if seen.insert(program.clone()) {
                // mpv is a console app: --version works correctly headless
                let version = native_version(&program);
                bros.push(media_bro(
                    &format!("native:mpv:{program}"),
                    "mpv",
                    program,
                    version,
                    vec![LaunchArg::TargetPlaylistFile { flag: "--playlist=".to_string() }],
                ));
            }
        }

        for program in find_exe("vlc.exe", &vlc_install_candidates()) {
            if seen.insert(program.clone()) {
                // VLC is a GUI-subsystem app: running vlc --version opens a window.
                // Read the version from the PE VERSIONINFO resource instead.
                let version = pe_file_version(&program)
                    .unwrap_or_else(|| "installed".to_string());
                bros.push(media_bro(
                    &format!("native:vlc:{program}"),
                    "VLC",
                    program,
                    version,
                    vec![LaunchArg::TargetPlaylistFile { flag: "".to_string() }],
                ));
            }
        }

        bros
    }
}

fn media_bro(id: &str, name: &str, program: String, version: String, args: Vec<LaunchArg>) -> ResolvedBro {
    ResolvedBro {
        option: BroOption {
            id: id.to_string(),
            name: name.to_string(),
            version,
            source: BroSource::Native,
            detail: Some(program.clone()),
            protocol: BroProtocol::Fnf,
            output: None,
            params: vec![],
            pre: BroPre { score: 0, reason: String::new() },
        },
        launch: LaunchRecipe::Command {
            program,
            cwd: None,
            args,
        },
    }
}

/// Search PATH first, then the given install-directory candidates.
fn find_exe(exe_name: &str, install_candidates: &[PathBuf]) -> Vec<String> {
    let mut results = Vec::new();

    if let Some(p) = find_binary_on_path(&[exe_name]) {
        results.push(p);
    }

    for candidate in install_candidates {
        if candidate.is_file() {
            let s = candidate.to_string_lossy().to_string();
            if !results.contains(&s) {
                results.push(s);
            }
        }
    }

    results
}

fn mpv_install_candidates() -> Vec<PathBuf> {
    let mut c = Vec::new();

    for env_var in &["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(pf) = env::var_os(env_var) {
            c.push(PathBuf::from(pf).join("mpv").join("mpv.exe"));
        }
    }
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        c.push(PathBuf::from(local).join("Programs").join("mpv").join("mpv.exe"));
    }
    // Scoop
    if let Some(home) = env::var_os("USERPROFILE") {
        c.push(
            PathBuf::from(home)
                .join("scoop")
                .join("apps")
                .join("mpv")
                .join("current")
                .join("mpv.exe"),
        );
    }

    c
}

fn vlc_install_candidates() -> Vec<PathBuf> {
    let mut c = Vec::new();

    for env_var in &["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(pf) = env::var_os(env_var) {
            c.push(PathBuf::from(pf).join("VideoLAN").join("VLC").join("vlc.exe"));
        }
    }
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        c.push(
            PathBuf::from(local)
                .join("Programs")
                .join("VideoLAN")
                .join("VLC")
                .join("vlc.exe"),
        );
    }
    // Scoop
    if let Some(home) = env::var_os("USERPROFILE") {
        c.push(
            PathBuf::from(home)
                .join("scoop")
                .join("apps")
                .join("vlc")
                .join("current")
                .join("vlc.exe"),
        );
    }

    c
}

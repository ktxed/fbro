use std::env;
use std::ffi::c_void;
use std::path::{Path, PathBuf};

use crate::bro::model::{
    BroOption, BroPre, BroProtocol, BroSource, LaunchArg, LaunchRecipe, ResolvedBro,
};
use crate::bro::probe::find_binary_on_path;
use crate::bro::providers::{mpv_playlist_args, BroDiscoveryContext, BroProvider};
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
    let ok =
        unsafe { GetFileVersionInfoW(wide.as_ptr(), 0, size, buf.as_mut_ptr() as *mut c_void) };
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

        for program in mpv_programs() {
            if seen.insert(program.clone()) {
                // mpv is a console app: --version works correctly headless
                let version = native_version(&program);
                bros.push(media_bro(
                    &format!("native:mpv:{program}"),
                    "mpv",
                    program,
                    version,
                    mpv_playlist_args(),
                ));
            }
        }

        for program in find_exe("vlc.exe", &vlc_install_candidates()) {
            if seen.insert(program.clone()) {
                // VLC is a GUI-subsystem app: running vlc --version opens a window.
                // Read the version from the PE VERSIONINFO resource instead.
                let version = pe_file_version(&program).unwrap_or_else(|| "installed".to_string());
                bros.push(media_bro(
                    &format!("native:vlc:{program}"),
                    "VLC",
                    program,
                    version,
                    vec![LaunchArg::TargetPlaylistFile {
                        flag: "".to_string(),
                    }],
                ));
            }
        }

        bros
    }
}

fn media_bro(
    id: &str,
    name: &str,
    program: String,
    version: String,
    args: Vec<LaunchArg>,
) -> ResolvedBro {
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
            pre: BroPre {
                score: 0,
                reason: String::new(),
            },
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

fn mpv_programs() -> Vec<String> {
    let mut results = find_exe("mpv.exe", &mpv_install_candidates());
    for path in registered_mpv_exes() {
        if !results
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&path))
        {
            results.push(path);
        }
    }
    results
}

fn mpv_install_candidates() -> Vec<PathBuf> {
    let mut c = Vec::new();

    for env_var in &["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(pf) = env::var_os(env_var) {
            let pf = PathBuf::from(pf);
            c.push(pf.join("mpv").join("mpv.exe"));
            // Inno Setup layout used by the shinchiro.mpv WinGet package.
            c.push(pf.join("MPV Player").join("mpv.exe"));
        }
    }
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        let programs = PathBuf::from(local).join("Programs");
        c.push(programs.join("mpv").join("mpv.exe"));
        c.push(programs.join("MPV Player").join("mpv.exe"));
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
            c.push(
                PathBuf::from(pf)
                    .join("VideoLAN")
                    .join("VLC")
                    .join("vlc.exe"),
            );
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

// Predefined HKEY values are sign-extended 32-bit constants. Casting through
// i32 keeps HKEY_CURRENT_USER / HKEY_LOCAL_MACHINE correct on 64-bit.
const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u32 as i32 as isize;
const RRF_RT_REG_SZ: u32 = 0x2;
const RRF_RT_REG_EXPAND_SZ: u32 = 0x4;
const MPV_OPEN_COMMAND: &str = r"Software\Classes\Applications\mpv.exe\shell\open\command";

#[link(name = "advapi32")]
extern "system" {
    fn RegGetValueW(
        hkey: isize,
        lp_sub_key: *const u16,
        lp_value: *const u16,
        dw_flags: u32,
        pdw_type: *mut u32,
        pv_data: *mut c_void,
        pcb_data: *mut u32,
    ) -> i32;
}

/// Paths recorded by the shinchiro installer and by `mpv-install.bat`.
fn registered_mpv_exes() -> Vec<String> {
    let mut found = Vec::new();
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let Some(command) = reg_default_sz(hive, MPV_OPEN_COMMAND) else {
            continue;
        };
        let Some(path) = exe_from_open_command(&command) else {
            continue;
        };
        if Path::new(&path).is_file()
            && !found
                .iter()
                .any(|existing: &String| existing.eq_ignore_ascii_case(&path))
        {
            found.push(path);
        }
    }
    found
}

fn reg_default_sz(hkey: isize, subkey: &str) -> Option<String> {
    let sub: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
    let mut size = 0u32;
    let status = unsafe {
        RegGetValueW(
            hkey,
            sub.as_ptr(),
            std::ptr::null(),
            flags,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    // A null buffer returns ERROR_SUCCESS (0) with the required byte count.
    // ERROR_MORE_DATA (234) is accepted for the same query.
    if (status != 0 && status != 234) || size < 2 || size > 32 * 1024 {
        return None;
    }
    let mut buf = vec![0u16; (size as usize) / 2 + 1];
    let mut size = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            hkey,
            sub.as_ptr(),
            std::ptr::null(),
            flags,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut c_void,
            &mut size,
        )
    };
    if status != 0 {
        return None;
    }
    let nchars = (size as usize) / 2;
    let end = buf[..nchars.min(buf.len())]
        .iter()
        .position(|&ch| ch == 0)
        .unwrap_or(nchars.min(buf.len()));
    let value = String::from_utf16_lossy(&buf[..end]);
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// First executable in a shell open command, when that executable is `mpv.exe`.
fn exe_from_open_command(command: &str) -> Option<String> {
    let command = command.trim();
    if command.is_empty() {
        return None;
    }
    let path = if let Some(rest) = command.strip_prefix('"') {
        let end = rest.find('"')?;
        rest[..end].to_string()
    } else {
        command.split_whitespace().next()?.to_string()
    };
    let name = Path::new(&path).file_name()?.to_str()?;
    name.eq_ignore_ascii_case("mpv.exe").then_some(path)
}

#[cfg(test)]
mod tests {
    use super::exe_from_open_command;

    #[test]
    fn quoted_open_command_keeps_the_mpv_path() {
        assert_eq!(
            exe_from_open_command(r#""C:\Program Files\MPV Player\mpv.exe" "%1""#),
            Some(r"C:\Program Files\MPV Player\mpv.exe".to_string())
        );
    }

    #[test]
    fn unquoted_open_command_keeps_the_mpv_path() {
        assert_eq!(
            exe_from_open_command(r"C:\mpv\mpv.exe"),
            Some(r"C:\mpv\mpv.exe".to_string())
        );
    }

    #[test]
    fn open_command_for_another_executable_is_ignored() {
        assert_eq!(
            exe_from_open_command(r#""C:\Program Files\MPV Player\mpvnet.exe" "%1""#),
            None
        );
    }
}

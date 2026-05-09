use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BroInputItem {
    pub path: String,
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BroProtocol {
    Fnf,
    Fp,
}

impl BroProtocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fnf => "fnf",
            Self::Fp => "fp",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BroSource {
    Native,
    Flatpak,
}

impl BroSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Flatpak => "flatpak",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BroLaunchInputKind {
    Int,
    Password,
}

impl BroLaunchInputKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Int => "int",
            Self::Password => "password",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BroParam {
    pub id: String,
    pub label: String,
    pub kind: BroLaunchInputKind,
    pub required: bool,
    pub default_value: Option<String>,
    pub min_value: Option<i32>,
    pub max_value: Option<i32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BroOutput {
    pub label: String,
    pub default_value: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BroPre {
    pub score: i32,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BroOption {
    pub id: String,
    pub name: String,
    pub version: String,
    pub source: BroSource,
    pub detail: Option<String>,
    pub protocol: BroProtocol,
    pub output: Option<BroOutput>,
    pub params: Vec<BroParam>,
    pub pre: BroPre,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LaunchRecipe {
    Command {
        program: String,
        cwd: Option<LaunchCwd>,
        args: Vec<LaunchArg>,
    },
    BuiltInSevenZip,
    BuiltInZip,
    BuiltInCopyPaths,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LaunchCwd {
    FirstInputParent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum LaunchArg {
    Literal(String),
    TargetPaths,
    TargetNames,
    /// Writes all target paths to a temp M3U playlist file and passes `<flag><path>` as a
    /// single argument. Use this instead of TargetPaths for players that accept --playlist=.
    TargetPlaylistFile { flag: String },
    OutputPath,
    PrefixedParam { id: String, prefix: String },
    OptionalPrefixedParam { id: String, prefix: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedBro {
    pub option: BroOption,
    pub(crate) launch: LaunchRecipe,
}

impl ResolvedBro {
    pub(crate) fn build_command(
        &self,
        target_inputs: &[BroInputItem],
        output_path: Option<&str>,
        launch_params: &HashMap<String, String>,
    ) -> Result<(String, Vec<String>, Option<PathBuf>), String> {
        match &self.launch {
            LaunchRecipe::Command { program, cwd, args } => {
                let cwd = match cwd {
                    Some(LaunchCwd::FirstInputParent) => {
                        let first = target_inputs.first().ok_or_else(|| {
                            "No input entries were resolved for this bro".to_string()
                        })?;
                        let parent = Path::new(&first.path).parent().ok_or_else(|| {
                            "Failed to resolve a working directory for this bro".to_string()
                        })?;
                        Some(parent.to_path_buf())
                    }
                    None => None,
                };

                let mut resolved_args = Vec::new();
                for arg in args {
                    match arg {
                        LaunchArg::Literal(value) => resolved_args.push(value.clone()),
                        LaunchArg::TargetPaths => resolved_args
                            .extend(target_inputs.iter().map(|input| input.path.clone())),
                        LaunchArg::TargetNames => resolved_args
                            .extend(target_inputs.iter().map(|input| input.name.clone())),
                        LaunchArg::TargetPlaylistFile { flag } => {
                            let playlist_path = write_m3u_playlist(target_inputs)?;
                            resolved_args.push(format!("{flag}{playlist_path}"));
                        }
                        LaunchArg::OutputPath => {
                            let value = output_path
                                .map(str::trim)
                                .filter(|value| !value.is_empty())
                                .ok_or_else(|| missing_output_message(&self.option))?;
                            resolved_args.push(value.to_string());
                        }
                        LaunchArg::PrefixedParam { id, prefix } => {
                            let value = launch_params
                                .get(id)
                                .map(|value| value.trim())
                                .filter(|value| !value.is_empty())
                                .ok_or_else(|| missing_param_message(&self.option, id))?;
                            resolved_args.push(format!("{prefix}{value}"));
                        }
                        LaunchArg::OptionalPrefixedParam { id, prefix } => {
                            let value = launch_params
                                .get(id)
                                .map(|value| value.trim())
                                .unwrap_or("");
                            if !value.is_empty() {
                                resolved_args.push(format!("{prefix}{value}"));
                            }
                        }
                    }
                }

                Ok((program.clone(), resolved_args, cwd))
            }
            LaunchRecipe::BuiltInSevenZip
            | LaunchRecipe::BuiltInZip
            | LaunchRecipe::BuiltInCopyPaths => Err(
                "This bro uses a built-in runtime and cannot be launched as a command".to_string(),
            ),
        }
    }

    pub(crate) fn output_path(&self, output_path: Option<&str>) -> Result<Option<String>, String> {
        let Some(output) = &self.option.output else {
            return Ok(None);
        };
        let value = output_path
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| missing_output_message_for_label(&output.label))?;
        Ok(Some(value.to_string()))
    }
}

fn write_m3u_playlist(inputs: &[BroInputItem]) -> Result<String, String> {
    let path = std::env::temp_dir().join("fbro_playlist.m3u");
    let mut file = std::fs::File::create(&path).map_err(|e| format!("Failed to create playlist: {e}"))?;
    writeln!(file, "#EXTM3U").map_err(|e| format!("Failed to write playlist: {e}"))?;
    for input in inputs {
        // Use file:/// URIs so players like VLC don't misparse "C:/..." as a URL scheme.
        // Paths are already forward-slash normalised; strip any leading slash before
        // prepending the three-slash prefix so both Unix (/home/…) and Windows (C:/…)
        // produce valid file:/// URIs.
        let uri = format!("file:///{}", input.path.trim_start_matches('/'));
        writeln!(file, "{uri}").map_err(|e| format!("Failed to write playlist: {e}"))?;
    }
    path.to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Playlist path is not valid UTF-8".to_string())
}

fn missing_output_message(option: &BroOption) -> String {
    option
        .output
        .as_ref()
        .map(|output| missing_output_message_for_label(&output.label))
        .unwrap_or_else(|| "Missing required output".to_string())
}

fn missing_output_message_for_label(label: &str) -> String {
    format!("Missing required output: {label}")
}

fn missing_param_message(option: &BroOption, input_id: &str) -> String {
    option
        .params
        .iter()
        .find(|input| input.id == input_id)
        .map(|input| format!("Missing required input: {}", input.label))
        .unwrap_or_else(|| format!("Missing required input: {input_id}"))
}

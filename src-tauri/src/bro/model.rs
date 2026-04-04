use std::collections::HashMap;
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
    Path,
}

impl BroLaunchInputKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Path => "path",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BroLaunchInput {
    pub id: String,
    pub label: String,
    pub kind: BroLaunchInputKind,
    pub required: bool,
    pub default_value: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BroOption {
    pub id: String,
    pub name: String,
    pub version: String,
    pub source: BroSource,
    pub detail: Option<String>,
    pub protocol: BroProtocol,
    pub inputs: Vec<BroLaunchInput>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LaunchRecipe {
    Command {
        program: String,
        cwd: Option<LaunchCwd>,
        args: Vec<LaunchArg>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LaunchCwd {
    FirstInputParent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LaunchArg {
    Literal(String),
    TargetPaths,
    TargetNames,
    UserInput(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedBro {
    pub option: BroOption,
    pub(crate) launch: LaunchRecipe,
    pub(crate) output_path_input_id: Option<String>,
}

impl ResolvedBro {
    pub(crate) fn build_command(
        &self,
        target_inputs: &[BroInputItem],
        launch_inputs: &HashMap<String, String>,
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
                        LaunchArg::UserInput(id) => {
                            let value = launch_inputs
                                .get(id)
                                .map(|value| value.trim())
                                .filter(|value| !value.is_empty())
                                .ok_or_else(|| missing_input_message(&self.option, id))?;
                            resolved_args.push(value.to_string());
                        }
                    }
                }

                Ok((program.clone(), resolved_args, cwd))
            }
        }
    }

    pub(crate) fn output_path(
        &self,
        launch_inputs: &HashMap<String, String>,
    ) -> Result<Option<String>, String> {
        let Some(input_id) = &self.output_path_input_id else {
            return Ok(None);
        };
        let value = launch_inputs
            .get(input_id)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| missing_input_message(&self.option, input_id))?;
        Ok(Some(value.to_string()))
    }
}

fn missing_input_message(option: &BroOption, input_id: &str) -> String {
    option
        .inputs
        .iter()
        .find(|input| input.id == input_id)
        .map(|input| format!("Missing required input: {}", input.label))
        .unwrap_or_else(|| format!("Missing required input: {input_id}"))
}

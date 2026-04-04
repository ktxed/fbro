#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BroInputItem {
    pub path: String,
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BroProtocol {
    Fnf,
}

impl BroProtocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fnf => "fnf",
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
pub struct BroOption {
    pub id: String,
    pub name: String,
    pub version: String,
    pub source: BroSource,
    pub detail: Option<String>,
    pub protocol: BroProtocol,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LaunchRecipe {
    Command {
        program: String,
        args_before_inputs: Vec<String>,
        args_after_inputs: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedBro {
    pub option: BroOption,
    pub(crate) launch: LaunchRecipe,
}

impl ResolvedBro {
    pub(crate) fn build_command(&self, inputs: &[BroInputItem]) -> (String, Vec<String>) {
        match &self.launch {
            LaunchRecipe::Command {
                program,
                args_before_inputs,
                args_after_inputs,
            } => {
                let mut args = args_before_inputs.clone();
                args.extend(inputs.iter().map(|input| input.path.clone()));
                args.extend(args_after_inputs.clone());
                (program.clone(), args)
            }
        }
    }
}

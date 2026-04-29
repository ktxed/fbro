use std::collections::HashMap;
use std::process::Command;

use crate::bro::model::{BroInputItem, ResolvedBro};
use crate::bro::pre::rank_bros;
use crate::bro::providers::{builtin_providers, BroDiscoveryContext};

pub fn list_bros(inputs: &[BroInputItem], suggested_output_dir: Option<&str>) -> Vec<ResolvedBro> {
    let ctx = BroDiscoveryContext {
        inputs,
        suggested_output_dir,
    };
    let bros = builtin_providers()
        .into_iter()
        .flat_map(|provider| provider.list(&ctx))
        .collect();
    rank_bros(inputs, bros)
}

pub fn launch_bro(
    inputs: &[BroInputItem],
    bro: &ResolvedBro,
    output_path: Option<&str>,
    params: &HashMap<String, String>,
) -> Result<std::process::Child, String> {
    if inputs.is_empty() {
        return Err("No input entries were resolved for this bro".to_string());
    }

    let (program, args, cwd) = bro.build_command(inputs, output_path, params)?;
    let mut cmd = Command::new(&program);
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    cmd.spawn().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;

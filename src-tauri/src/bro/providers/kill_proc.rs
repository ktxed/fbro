use crate::bro::model::{BroOption, BroPre, BroProtocol, BroSource, LaunchRecipe, ResolvedBro};

use super::{BroDiscoveryContext, BroProvider};

pub struct KillProcProvider;

impl BroProvider for KillProcProvider {
    fn list(&self, ctx: &BroDiscoveryContext<'_>) -> Vec<ResolvedBro> {
        if ctx.source_id != Some("procs") {
            return vec![];
        }
        vec![ResolvedBro {
            option: BroOption {
                id: "builtin:kill-proc".to_string(),
                name: "Kill (SIGKILL)".to_string(),
                version: "builtin".to_string(),
                source: BroSource::Native,
                detail: Some("Force-terminate without grace period".to_string()),
                protocol: BroProtocol::Fnf,
                output: None,
                params: vec![],
                pre: BroPre {
                    score: 95,
                    reason: "Force-kill selected processes".to_string(),
                },
            },
            launch: LaunchRecipe::BuiltInKillProc,
        }]
    }
}

use super::{BroDiscoveryContext, BroProvider};
use crate::bro::model::{BroOption, BroPre, BroProtocol, BroSource, LaunchRecipe, ResolvedBro};

pub struct FedupProvider;

impl BroProvider for FedupProvider {
    fn list(&self, ctx: &BroDiscoveryContext<'_>) -> Vec<ResolvedBro> {
        if ctx.inputs.is_empty() || ctx.source_id != Some(crate::vfs::global_selection::SOURCE_ID) {
            return Vec::new();
        }

        vec![ResolvedBro {
            option: BroOption {
                id: "builtin:fedup".to_string(),
                name: "Fedup Bro".to_string(),
                version: "builtin".to_string(),
                source: BroSource::Native,
                detail: Some("Find exact duplicate files with BLAKE3".to_string()),
                protocol: BroProtocol::Fnf,
                output: None,
                params: Vec::new(),
                pre: BroPre {
                    score: 0,
                    reason: String::new(),
                },
            },
            launch: LaunchRecipe::BuiltInFedupBro,
        }]
    }
}

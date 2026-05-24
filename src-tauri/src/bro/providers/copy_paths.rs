use super::{BroDiscoveryContext, BroProvider};
use crate::bro::model::{BroOption, BroPre, BroProtocol, BroSource, LaunchRecipe, ResolvedBro};

pub struct CopyPathsProvider;

impl BroProvider for CopyPathsProvider {
    fn list(&self, _ctx: &BroDiscoveryContext<'_>) -> Vec<ResolvedBro> {
        vec![ResolvedBro {
            option: BroOption {
                id: "builtin:copy-paths".to_string(),
                name: "Copy Paths".to_string(),
                version: "builtin".to_string(),
                source: BroSource::Native,
                detail: Some("Copy file paths to clipboard".to_string()),
                protocol: BroProtocol::Fnf,
                output: None,
                params: vec![],
                pre: BroPre {
                    score: 0,
                    reason: String::new(),
                },
            },
            launch: LaunchRecipe::BuiltInCopyPaths,
        }]
    }
}

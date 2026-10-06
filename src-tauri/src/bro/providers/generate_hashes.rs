use super::{BroDiscoveryContext, BroProvider};
use crate::bro::model::{
    BroLaunchInputKind, BroOption, BroParam, BroPre, BroProtocol, BroSource, LaunchRecipe,
    ResolvedBro,
};

pub struct GenerateHashesProvider;

impl BroProvider for GenerateHashesProvider {
    fn list(&self, ctx: &BroDiscoveryContext<'_>) -> Vec<ResolvedBro> {
        if !ctx.hash_generate_available {
            return Vec::new();
        }
        vec![ResolvedBro {
            option: BroOption {
                id: "builtin:generate-hashes".to_string(),
                name: "Generate hashes".to_string(),
                version: "builtin".to_string(),
                source: BroSource::Native,
                detail: Some("Create a .b3 BLAKE3 sidecar for each selected file".to_string()),
                protocol: BroProtocol::Fnf,
                output: None,
                params: vec![BroParam {
                    id: "overwriteExisting".to_string(),
                    label: "Overwrite existing".to_string(),
                    kind: BroLaunchInputKind::Bool,
                    required: false,
                    default_value: Some("false".to_string()),
                    min_value: None,
                    max_value: None,
                }],
                pre: BroPre {
                    score: 0,
                    reason: String::new(),
                },
            },
            launch: LaunchRecipe::BuiltInGenerateHashes,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_selection_eligibility_and_default_preserve_existing_hashes() {
        let mut ctx = BroDiscoveryContext {
            inputs: &[],
            suggested_output_dir: None,
            source_id: None,
            hash_check_available: false,
            hash_generate_available: false,
        };
        assert!(GenerateHashesProvider.list(&ctx).is_empty());
        ctx.hash_generate_available = true;
        let bros = GenerateHashesProvider.list(&ctx);
        assert_eq!(bros[0].option.id, "builtin:generate-hashes");
        assert_eq!(bros[0].option.params[0].id, "overwriteExisting");
        assert_eq!(
            bros[0].option.params[0].default_value.as_deref(),
            Some("false")
        );
    }
}

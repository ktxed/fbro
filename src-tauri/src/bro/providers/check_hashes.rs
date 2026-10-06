use super::{BroDiscoveryContext, BroProvider};
use crate::bro::model::{
    BroLaunchInputKind, BroOption, BroParam, BroPre, BroProtocol, BroSource, LaunchRecipe,
    ResolvedBro,
};

pub struct CheckHashesProvider;

impl BroProvider for CheckHashesProvider {
    fn list(&self, ctx: &BroDiscoveryContext<'_>) -> Vec<ResolvedBro> {
        if !ctx.hash_check_available {
            return Vec::new();
        }
        vec![ResolvedBro {
            option: BroOption {
                id: "builtin:check-hashes".to_string(),
                name: "Check hashes".to_string(),
                version: "builtin".to_string(),
                source: BroSource::Native,
                detail: Some("Verify files against their .b3 sidecars".to_string()),
                protocol: BroProtocol::Fnf,
                output: None,
                params: vec![BroParam {
                    id: "saveResult".to_string(),
                    label: "Save result".to_string(),
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
            launch: LaunchRecipe::BuiltInCheckHashes,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eligibility_uses_full_selection_flag_even_when_sample_has_no_sidecar() {
        let mut ctx = BroDiscoveryContext {
            inputs: &[],
            suggested_output_dir: None,
            source_id: None,
            hash_check_available: false,
            hash_generate_available: false,
        };
        assert!(CheckHashesProvider.list(&ctx).is_empty());
        ctx.hash_check_available = true;
        let bros = CheckHashesProvider.list(&ctx);
        assert_eq!(bros[0].option.id, "builtin:check-hashes");
        assert_eq!(
            bros[0].option.params[0].default_value.as_deref(),
            Some("false")
        );
    }
}

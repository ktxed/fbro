use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub const FP_HOST_PROTOCOL_VERSION: &str = "fp-host-stdio-v1";

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FpHostInput {
    pub path: String,
    pub name: String,
    pub kind: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct FpHostCapabilities {
    pub progress: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct FpHostProgress {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_files: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_files: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_bytes_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_bytes_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_file_percent: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overall_percent: Option<u8>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FpHostToRunnerMessage {
    Hello {
        protocol_version: String,
        capabilities: FpHostCapabilities,
    },
    Progress(FpHostProgress),
    Completed {
        #[serde(skip_serializing_if = "Option::is_none")]
        result_path: Option<String>,
    },
    Failed {
        error: String,
    },
    Log {
        level: String,
        message: String,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FpRunnerToHostMessage {
    Start {
        job_id: String,
        bro_id: String,
        bro_name: String,
        inputs: Vec<FpHostInput>,
        output_path: String,
        params: HashMap<String, String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        working_dir: Option<String>,
    },
    Cancel,
}

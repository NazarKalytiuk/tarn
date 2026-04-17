//! Verifies `docs/commands.json` and the in-code command registry
//! stay in lockstep.
//!
//! Every advertised command id must appear both in
//! `commands::ALL_COMMAND_IDS` and in the JSON manifest that external
//! LLM tooling reads. Shipping one without the other is a regression
//! because the manifest is the documented API surface.

use std::path::PathBuf;

#[test]
fn commands_json_contains_every_registered_id() {
    let manifest_path = repo_root().join("docs/commands.json");
    let raw = std::fs::read(&manifest_path).expect("docs/commands.json must exist");
    let manifest: serde_json::Value =
        serde_json::from_slice(&raw).expect("docs/commands.json must parse as JSON");

    assert_eq!(
        manifest["schema_version"], serde_json::json!(1),
        "docs/commands.json schema_version must match tarn_lsp::envelope::COMMAND_SCHEMA_VERSION"
    );

    let manifest_ids: Vec<String> = manifest["commands"]
        .as_array()
        .expect("commands must be an array")
        .iter()
        .map(|c| c["id"].as_str().expect("each command has id").to_owned())
        .collect();

    for registered in tarn_lsp::commands::ALL_COMMAND_IDS {
        assert!(
            manifest_ids.iter().any(|m| m == *registered),
            "command {registered} is registered in ALL_COMMAND_IDS but missing from docs/commands.json",
        );
    }

    for manifest_id in &manifest_ids {
        assert!(
            tarn_lsp::commands::ALL_COMMAND_IDS.contains(&manifest_id.as_str()),
            "command {manifest_id} is in docs/commands.json but not registered in ALL_COMMAND_IDS",
        );
    }
}

fn repo_root() -> PathBuf {
    // The integration test runs from the `tarn-lsp` crate. The
    // workspace root is one directory up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}

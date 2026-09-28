use super::*;
use crate::data::{assets, edit_recovery::model::digest};
use std::path::Path;

fn put_asset(root: &Path, name: &str, bytes: &[u8]) -> assets::Metadata {
    let metadata = assets::Metadata {
        schema_version: 1,
        id: uuid::Uuid::new_v4().to_string(),
        name: name.to_owned(),
        size: bytes.len() as u64,
        sha256: digest(bytes),
        image: false,
        width: None,
        height: None,
    };
    assets::Store::open(root, true)
        .unwrap()
        .put(&metadata, bytes)
        .unwrap();
    metadata
}

#[test]
fn m545_request_boundary_inspects_moves_restores_and_exports_safe_diagnostics() {
    let h = Harness::new();
    let project = h.open();
    let bytes = b"unreferenced asset bytes".to_vec();
    let metadata = put_asset(&h.root, "unused.txt", &bytes);
    let id = metadata.id.clone();

    let inspected = h.work(json!({"kind":"asset_inspect","project":project}));
    assert_eq!(inspected["kind"], "asset_maintenance", "{inspected}");
    assert_eq!(inspected["action"], "inspect");
    assert_eq!(inspected["inspection"]["complete"], true);
    assert_eq!(inspected["inspection"]["unusedAssets"], 1);
    assert_eq!(inspected["inspection"]["rows"][0]["status"], "unused");
    let token = inspected["inspection"]["token"]
        .as_str()
        .unwrap()
        .to_owned();

    let moved = h.work(json!({
        "kind":"asset_trash_move",
        "project":project,
        "inspection_token":token,
        "assets":[id]
    }));
    assert_eq!(moved["action"], "trash_move", "{moved}");
    assert_eq!(moved["completed"].as_array().unwrap().len(), 1);
    assert!(moved["failures"].as_array().unwrap().is_empty());
    assert_eq!(moved["inspection"]["trash"].as_array().unwrap().len(), 1);

    let unreadable = h.work(json!({
        "kind":"document_workspace",
        "project":project,
        "request":{"action":"asset_read","asset":metadata.id}
    }));
    assert_eq!(unreadable["value"]["kind"], "asset_error", "{unreadable}");
    assert_eq!(unreadable["value"]["assetName"], "unused.txt");
    assert_eq!(unreadable["value"]["assetState"], "trashed");

    let restored = h.work(json!({
        "kind":"asset_trash_restore",
        "project":project,
        "assets":[metadata.id]
    }));
    assert_eq!(restored["action"], "trash_restore", "{restored}");
    assert!(restored["failures"].as_array().unwrap().is_empty());
    assert_eq!(
        assets::Store::open(&h.root, false)
            .unwrap()
            .read(&metadata.id)
            .unwrap(),
        (metadata.clone(), bytes)
    );

    let inspected_again = h.work(json!({"kind":"asset_inspect","project":project}));
    let moved_again = h.work(json!({
        "kind":"asset_trash_move",
        "project":project,
        "inspection_token":inspected_again["inspection"]["token"],
        "assets":[metadata.id]
    }));
    let purged = h.work(json!({
        "kind":"asset_trash_purge",
        "project":project,
        "inspection_token":moved_again["inspection"]["token"],
        "assets":[metadata.id],
        "empty":false
    }));
    assert_eq!(purged["action"], "trash_purge", "{purged}");
    assert_eq!(purged["completedCount"], 1);
    assert!(purged["inspection"]["trash"].as_array().unwrap().is_empty());

    let destination = h.base.join("safe-diagnostics.json");
    let exported = h.work(json!({
        "kind":"diagnostic_export",
        "project":project,
        "destination":destination
    }));
    assert_eq!(exported["kind"], "diagnostic_export", "{exported}");
    assert!(exported["result"]["size"].as_u64().unwrap() > 0);
    let body = fs::read_to_string(&destination).unwrap();
    assert!(body.contains("document_content"));
    assert!(!body.contains(h.root.to_string_lossy().as_ref()));
    assert!(!body.contains("unreferenced asset bytes"));
    assert!(!body.contains("unused.txt"));

    assert!(h.control(json!({"kind":"close","project":project}))["error"].is_null());
    h.close_clean();
}

#[test]
fn m545_active_editor_owner_makes_unused_assets_uncertain_and_blocks_movement() {
    let h = Harness::new();
    let project = h.open();
    let (_, session) = h.template(&project);
    let metadata = put_asset(&h.root, "protected.txt", b"protected while editor is open");

    let inspected = h.work(json!({"kind":"asset_inspect","project":project}));
    assert_eq!(inspected["inspection"]["complete"], false, "{inspected}");
    assert_eq!(inspected["inspection"]["unusedAssets"], 0);
    assert_eq!(inspected["inspection"]["uncertainAssets"], 1);
    assert_eq!(inspected["inspection"]["rows"][0]["status"], "uncertain");

    let rejected = h.work(json!({
        "kind":"asset_trash_move",
        "project":project,
        "inspection_token":inspected["inspection"]["token"],
        "assets":[metadata.id]
    }));
    assert_eq!(rejected["kind"], "rejected", "{rejected}");
    assert_eq!(rejected["error"]["code"], "owners_remain");
    assert!(h.control(json!({
        "kind":"session_control",
        "project":project,
        "session":session,
        "control":"end"
    }))["error"]
        .is_null());
    assert!(h.control(json!({"kind":"close","project":project}))["error"].is_null());
    h.close_clean();
}

use super::*;

#[test]
fn preview_reservation_abandonment_cannot_cancel_accepted_or_control_work() {
    let h = Harness::new();
    let project = h.open();
    let reserved = h.reserve("ordinary");
    assert_eq!(
        h.call(json!({"action":"abandon_reservation","operation":reserved}))["kind"],
        "acknowledged"
    );
    assert_eq!(
        h.ipc(json!({"action":"operation","operation":reserved}))
            .unwrap_err()["code"],
        "unknown_id"
    );
    assert_eq!(h.ipc(json!({"action":"submit","operation":reserved,"input":{"kind":"document_workspace","project":project,"request":{"action":"list"}}})).unwrap_err()["code"], "unknown_id");
    let control = h.reserve("control");
    assert_eq!(
        h.ipc(json!({"action":"abandon_reservation","operation":control}))
            .unwrap_err()["code"],
        "not_terminal"
    );
    let accepted = h
        .submit(json!({"kind":"document_workspace","project":project,"request":{"action":"list"}}));
    assert_eq!(
        h.ipc(json!({"action":"abandon_reservation","operation":accepted}))
            .unwrap_err()["code"],
        "not_terminal"
    );
    assert_eq!(h.result(&accepted)["kind"], "document_workspace");
    h.ack(&accepted);
    h.close_clean();
}

#[test]
fn closed_preview_reservation_is_reclaimable_after_project_close() {
    let h = Harness::new();
    let project = h.open();
    let reserved = h.reserve("ordinary");
    h.control(json!({"kind":"close","project":project}));
    assert_eq!(h.ipc(json!({"action":"submit","operation":reserved,"input":{"kind":"document_workspace","project":project,"request":{"action":"asset_read","asset":"00000000-0000-4000-8000-000000000001"}}})).unwrap_err()["code"], "closed");
    assert_eq!(
        h.call(json!({"action":"operation","operation":reserved}))["state"],
        "reserved"
    );
    h.call(json!({"action":"abandon_reservation","operation":reserved}));
    assert_eq!(
        h.ipc(json!({"action":"operation","operation":reserved}))
            .unwrap_err()["code"],
        "unknown_id"
    );
    h.close_clean();
}

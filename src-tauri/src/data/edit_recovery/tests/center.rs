use super::*;

#[test]
fn recovery_center_cursor_reaches_past_4096_nodes_and_discard_checks_raw_version() {
    let fixture = Fixture::new();
    let mut store = Store::open(&fixture.root()).unwrap();
    for n in 0..4200 {
        fs::create_dir(fixture.root().join(format!("{n:064x}"))).unwrap();
    }
    let deposit = Deposit::freeze(sample()).unwrap();
    store.accept(&deposit).unwrap();
    let first = store.page(None).unwrap();
    assert!(first.next.is_some());
    let mut visited = first.visited_nodes;
    let mut cursor = first.next;
    let mut rows = first.entries;
    while let Some(token) = cursor {
        let next = store.page(Some(&token)).unwrap();
        assert!(next.visited_nodes <= MAX_LIST_ENTRIES);
        assert!(next.bytes_read <= MAX_LIST_READ_BYTES);
        visited += next.visited_nodes;
        rows.extend(next.entries);
        cursor = next.next;
    }
    assert!(visited > 4200);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.row.key.as_ref(), Some(deposit.key()));
    let target = file(&fixture.root(), deposit.key());
    let mut bytes = fs::read(&target).unwrap();
    bytes.push(b' ');
    fs::write(&target, &bytes).unwrap();
    assert_eq!(
        store
            .discard(deposit.key(), row.version.as_ref().unwrap())
            .unwrap_err()
            .category,
        Category::Conflict
    );
    assert!(target.exists());
    store.fault = Some(Stage::Discard);
    assert!(store.discard(deposit.key(), &digest(&bytes)).is_err());
    assert_eq!(fs::read(&target).unwrap(), bytes);
    store.fault = None;
    store.discard(deposit.key(), &digest(&bytes)).unwrap();
    assert!(!target.exists());
}

#[test]
fn recovery_center_corrupt_and_future_files_are_visible_without_granting_restore() {
    let fixture = Fixture::new();
    let mut store = Store::open(&fixture.root()).unwrap();
    let deposit = Deposit::freeze(sample()).unwrap();
    store.accept(&deposit).unwrap();
    let target = file(&fixture.root(), deposit.key());
    for bytes in [
        b"{broken PRIVATE".as_slice(),
        br#"{"recoverySchemaVersion":999}"#,
    ] {
        fs::write(&target, bytes).unwrap();
        let page = store.page(None).unwrap();
        assert_eq!(page.entries.len(), 1);
        let row = &page.entries[0];
        assert!(row.row.error.is_some());
        assert!(row.row.deposit_id.is_none());
        assert_eq!(row.version.as_deref(), Some(digest(bytes).as_str()));
        assert!(!serde_json::to_string(&page).unwrap().contains("PRIVATE"));
        store
            .discard(deposit.key(), row.version.as_ref().unwrap())
            .unwrap();
        assert!(!target.exists());
        if bytes.starts_with(b"{broken") {
            fs::write(&target, deposit.bytes()).unwrap();
        }
    }
}

#[test]
fn recovery_center_page_read_budget_continues_after_deferred_file() {
    let fixture = Fixture::new();
    let mut store = Store::open(&fixture.root()).unwrap();
    let deposit = Deposit::freeze(sample()).unwrap();
    store.accept(&deposit).unwrap();
    let parent = file(&fixture.root(), deposit.key())
        .parent()
        .unwrap()
        .to_owned();
    let bytes = vec![b' '; 24 * 1024 * 1024];
    for g in 2..=4 {
        fs::write(parent.join(format!("{g}.json")), &bytes).unwrap();
    }
    let mut next = None;
    let mut rows = 0;
    let mut pages = 0;
    loop {
        let page = store.page(next.as_deref()).unwrap();
        assert!(page.bytes_read <= MAX_LIST_READ_BYTES);
        rows += page.entries.len();
        pages += 1;
        next = page.next;
        if next.is_none() {
            break;
        }
    }
    assert_eq!(rows, 4);
    assert!(pages >= 2);
}

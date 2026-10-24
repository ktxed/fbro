use super::*;

fn database() -> Connection {
    open_path(std::path::Path::new(":memory:")).unwrap()
}

fn insert(conn: &Connection, number: u32, timestamp: i64, payload: Value) {
    conn.execute(
        "INSERT INTO journal_events
         (event_id, timestamp_ms, event_type, version, payload_json)
         VALUES (?1, ?2, 'intent.command.v1', 1, ?3)",
        params![id(number), timestamp, payload.to_string()],
    )
    .unwrap();
}

fn id(number: u32) -> String {
    format!("00000000-0000-4000-8000-{number:012}")
}

fn listing(conn: &Connection, by: &str, order: &str) -> Vec<VfsEntry> {
    list_from_connection(
        conn,
        &SortSpec {
            by: by.into(),
            order: order.into(),
        },
        None,
    )
    .unwrap()
}

fn numbers(entries: &[VfsEntry]) -> Vec<u32> {
    entries
        .iter()
        .map(|entry| {
            entry
                .source_path
                .as_ref()
                .unwrap()
                .rsplit('-')
                .next()
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect()
}

macro_rules! name_sort_test {
    ($test:ident, $column:literal, $order:literal, $expected:expr) => {
        #[test]
        fn $test() {
            let conn = database();
            // Insertion/time order deliberately differs from both requested orders.
            insert(&conn, 4, 1004, json!({ "kind": "Zulu" }));
            insert(&conn, 3, 1003, json!({ "kind": "beta" }));
            insert(&conn, 2, 1001, json!({ "kind": "alpha" }));
            insert(&conn, 1, 1002, json!({ "kind": "Alpha" }));
            assert_eq!(numbers(&listing(&conn, $column, $order)), $expected);
        }
    };
}

name_sort_test!(
    name_ascending_is_case_insensitive,
    "name",
    "asc",
    vec![1, 2, 3, 4]
);
name_sort_test!(
    name_descending_is_case_insensitive,
    "name",
    "desc",
    vec![4, 3, 2, 1]
);
name_sort_test!(
    kind_ascending_uses_name_for_log_files,
    "kind",
    "asc",
    vec![1, 2, 3, 4]
);
name_sort_test!(
    kind_descending_uses_name_for_log_files,
    "kind",
    "desc",
    vec![4, 3, 2, 1]
);

macro_rules! size_sort_test {
    ($test:ident, $order:literal, $expected:expr) => {
        #[test]
        fn $test() {
            let conn = database();
            // Two characters occupy more UTF-8 bytes than three ASCII characters.
            insert(&conn, 1, 1000, json!({ "kind": "item", "text": "éé" }));
            insert(&conn, 2, 1000, json!({ "kind": "item", "text": "aaa" }));
            insert(&conn, 3, 1000, json!({ "kind": "item", "text": "bbb" }));
            let entries = listing(&conn, "size", $order);
            assert_eq!(numbers(&entries), $expected);
            let entry = entries.iter().find(|entry| entry.source_path == Some(id(1))).unwrap();
            let expected_detail = concat!(
                "fbro journal event\n\nID:       00000000-0000-4000-8000-000000000001\n",
                "Timestamp: 1000\nType:     intent.command.v1\nOutcome:  —\n\nPayload:\n",
                "{\n  \"kind\": \"item\",\n  \"text\": \"éé\"\n}\n"
            );
            assert_eq!(entry.size, Some(expected_detail.len() as u64));
        }
    };
}

size_sort_test!(
    size_ascending_uses_detail_bytes_then_name,
    "asc",
    vec![2, 3, 1]
);
size_sort_test!(
    size_descending_uses_detail_bytes_then_name,
    "desc",
    vec![1, 3, 2]
);

macro_rules! date_sort_test {
    ($test:ident, $order:literal, $expected:expr) => {
        #[test]
        fn $test() {
            let conn = database();
            for (number, timestamp, kind) in [
                (1, -1001, "z"), (2, 0, "z"), (3, 999, "z"),
                (4, 1000, "z"), (5, 1001, "z"), (6, 9000, "z"),
                (7, 10000, "z"), (8, 1_700_000_000_000, "z"),
                // Same millisecond: case-insensitive name, exact name, then ID.
                (10, 1002, "Alpha"), (12, 1002, "alpha"),
                (11, 1002, "Alpha"),
            ] {
                insert(&conn, number, timestamp, json!({ "kind": kind }));
            }
            let entries = listing(&conn, "date", $order);
            assert_eq!(numbers(&entries), $expected);
            let entry = entries.iter().find(|entry| entry.source_path == Some(id(5))).unwrap();
            assert_eq!(entry.mtime.as_deref(), Some("1"));
            assert_eq!(entry.kind, VfsKind::File);
            assert_eq!(entry.icon_name.as_deref(), Some("text-x-generic"));
            assert_eq!(JournalDriver.resolve_child("", &entry.name), id(5));
        }
    };
}

date_sort_test!(
    date_ascending_preserves_numeric_milliseconds_and_stable_ties,
    "asc",
    vec![1, 2, 3, 4, 5, 10, 11, 12, 6, 7, 8]
);
date_sort_test!(
    date_descending_preserves_numeric_milliseconds_and_stable_ties,
    "desc",
    vec![8, 7, 6, 12, 11, 10, 5, 4, 3, 2, 1]
);

#[test]
fn filtering_payload_and_name_keeps_requested_order_and_stable_paths() {
    let conn = database();
    insert(&conn, 1, 1001, json!({ "kind": "Zulu", "text": "MATCH" }));
    insert(&conn, 2, 1000, json!({ "kind": "match" }));
    insert(&conn, 3, 1002, json!({ "kind": "unrelated" }));
    let filter = FilterSpec {
        mode: "contains".into(),
        value: "mAtCh".into(),
    };
    let entries = list_from_connection(
        &conn,
        &SortSpec {
            by: "date".into(),
            order: "asc".into(),
        },
        Some(&filter),
    )
    .unwrap();
    assert_eq!(numbers(&entries), vec![2, 1]);
    assert!(entries
        .iter()
        .all(|entry| uuid::Uuid::parse_str(entry.source_path.as_ref().unwrap()).is_ok()));
}

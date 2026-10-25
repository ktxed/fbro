use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use rusqlite::{params, Connection};

use crate::ws::protocol::SortSpec;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocationSettings {
    pub sort: Option<SortSpec>,
}

pub type LocationSettingsMap = HashMap<String, LocationSettings>;
pub const RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1_000;
pub const JOURNAL_SORT_KEY: &str = "source:journal";
const SCHEMA_VERSION: i64 = 2;

const CREATE_TABLE: &str = "CREATE TABLE IF NOT EXISTS settings_location (
    path TEXT PRIMARY KEY NOT NULL,
    sort_by TEXT,
    sort_order TEXT,
    CHECK (
        (sort_by IS NULL AND sort_order IS NULL)
        OR (sort_by IN ('name', 'kind', 'size', 'date') AND sort_order IN ('asc', 'desc'))
    )
)";

fn open(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection
        .busy_timeout(Duration::from_secs(2))
        .map_err(|error| error.to_string())?;
    connection
        .execute(CREATE_TABLE, [])
        .map_err(|error| error.to_string())?;
    Ok(connection)
}

pub(crate) fn database_path() -> std::path::PathBuf {
    crate::config::config_dir().join("fbro.sqlite3")
}

fn migrate_legacy_table(connection: &mut Connection, legacy_path: &Path) -> Result<(), String> {
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if version >= SCHEMA_VERSION {
        return Ok(());
    }

    if legacy_path.exists() {
        connection
            .execute(
                "ATTACH DATABASE ?1 AS legacy_journal",
                params![legacy_path.to_string_lossy()],
            )
            .map_err(|error| error.to_string())?;

        let migration = (|| -> Result<(), rusqlite::Error> {
            let has_legacy_table: bool = connection.query_row(
                "SELECT EXISTS (
                    SELECT 1 FROM legacy_journal.sqlite_master
                    WHERE type = 'table' AND name = 'location_settings'
                )",
                [],
                |row| row.get(0),
            )?;
            if has_legacy_table {
                let transaction = connection.transaction()?;
                transaction.execute(
                    "INSERT OR REPLACE INTO main.settings_location (path, sort_by, sort_order)
                     SELECT path, sort_by, sort_order
                     FROM legacy_journal.location_settings
                     WHERE sort_by IN ('name', 'kind', 'size', 'date')
                       AND sort_order IN ('asc', 'desc')",
                    [],
                )?;
                transaction.execute("DROP TABLE legacy_journal.location_settings", [])?;
                transaction.commit()?;
            }
            Ok(())
        })();

        let detach = connection.execute("DETACH DATABASE legacy_journal", []);
        migration.map_err(|error| error.to_string())?;
        detach.map_err(|error| error.to_string())?;
    }

    let has_v1_table: bool = connection
        .query_row(
            "SELECT EXISTS (
                SELECT 1 FROM main.sqlite_master
                WHERE type = 'table' AND name = 'location_settings'
            )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    if has_v1_table {
        transaction
            .execute(
                "INSERT OR REPLACE INTO settings_location (path, sort_by, sort_order)
                 SELECT path, sort_by, sort_order
                 FROM location_settings
                 WHERE sort_by IN ('name', 'kind', 'size', 'date')
                   AND sort_order IN ('asc', 'desc')",
                [],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute("DROP TABLE location_settings", [])
            .map_err(|error| error.to_string())?;
    }
    transaction
        .pragma_update(None, "user_version", SCHEMA_VERSION)
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(())
}

fn open_database() -> Result<Connection, String> {
    let mut connection = open(&database_path())?;
    migrate_legacy_table(&mut connection, &crate::journal::database_path())?;
    Ok(connection)
}

fn load_from(path: &Path) -> Result<LocationSettingsMap, String> {
    let connection = open(path)?;
    load_from_connection(&connection)
}

fn load_from_connection(connection: &Connection) -> Result<LocationSettingsMap, String> {
    let mut statement = connection
        .prepare("SELECT path, sort_by, sort_order FROM settings_location")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|error| error.to_string())?;

    let mut settings = HashMap::new();
    for row in rows {
        let Ok((path, Some(by), Some(order))) = row else {
            continue;
        };
        if !valid_sort(&by, &order) {
            continue;
        }
        settings.insert(
            normalize_local_path(&path),
            LocationSettings {
                sort: Some(SortSpec { by, order }),
            },
        );
    }
    Ok(settings)
}

fn save_to(path: &Path, location: &str, settings: &LocationSettings) -> Result<(), String> {
    let connection = open(path)?;
    save_to_connection(&connection, location, settings)
}

fn save_to_connection(
    connection: &Connection,
    location: &str,
    settings: &LocationSettings,
) -> Result<(), String> {
    let location = normalize_local_path(location);
    let (sort_by, sort_order) = settings
        .sort
        .as_ref()
        .filter(|sort| valid_sort(&sort.by, &sort.order))
        .map(|sort| (Some(sort.by.as_str()), Some(sort.order.as_str())))
        .unwrap_or((None, None));
    connection
        .execute(
            "INSERT INTO settings_location (path, sort_by, sort_order)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET
                 sort_by = excluded.sort_by,
                 sort_order = excluded.sort_order",
            params![location, sort_by, sort_order],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn clear_sorts_from(path: &Path) -> Result<usize, String> {
    let connection = open(path)?;
    clear_sorts_from_connection(&connection)
}

fn clear_sorts_from_connection(connection: &Connection) -> Result<usize, String> {
    connection
        .execute("DELETE FROM settings_location", [])
        .map_err(|error| error.to_string())
}

pub fn load() -> Result<LocationSettingsMap, String> {
    let connection = open_database()?;
    load_from_connection(&connection)
}

pub fn save(location: &str, settings: &LocationSettings) -> Result<(), String> {
    let connection = open_database()?;
    save_to_connection(&connection, location, settings)
}

pub fn clear_sorts() -> Result<usize, String> {
    let connection = open_database()?;
    clear_sorts_from_connection(&connection)
}

pub fn normalize_local_path(path: &str) -> String {
    #[cfg(windows)]
    let normalized = path.replace('\\', "/");
    #[cfg(not(windows))]
    let normalized = path.to_string();

    let trimmed = normalized.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    #[cfg(windows)]
    if trimmed.len() == 2 && trimmed.as_bytes()[1] == b':' {
        return format!("{trimmed}/");
    }
    trimmed.to_string()
}

/// Journal has one logical root; its key cannot collide with a host path.
pub fn key_for(source_id: &str, path: &str, is_local: bool) -> Option<String> {
    if source_id == crate::journal::SOURCE_ID {
        Some(JOURNAL_SORT_KEY.to_string())
    } else {
        is_local.then(|| normalize_local_path(path))
    }
}

pub fn default_sort() -> SortSpec {
    SortSpec {
        by: "name".to_string(),
        order: "asc".to_string(),
    }
}

pub fn sort_for(settings: &LocationSettingsMap, path: &str) -> SortSpec {
    settings
        .get(&normalize_local_path(path))
        .and_then(|settings| settings.sort.clone())
        .filter(|sort| valid_sort(&sort.by, &sort.order))
        .unwrap_or_else(default_sort)
}

pub fn valid_sort(by: &str, order: &str) -> bool {
    matches!(by, "name" | "kind" | "size" | "date") && matches!(order, "asc" | "desc")
}

pub fn retention_expired(disabled_at_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(disabled_at_ms) >= RETENTION_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_key_is_source_scoped_and_other_virtual_sources_stay_excluded() {
        assert_eq!(key_for("journal", "", false).as_deref(), Some("source:journal"));
        assert_eq!(key_for("journal", "/", false).as_deref(), Some("source:journal"));
        assert_eq!(key_for("volume", "/", true).as_deref(), Some("/"));
        for source in ["remote:peer:share", "search:job-1", "zip:/tmp/a.zip", "procs"] {
            assert_eq!(key_for(source, "/", false), None, "{source}");
        }
    }

    #[test]
    fn journal_and_filesystem_preferences_reload_independently_and_reset_together() {
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup { fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); } }
        let database = Cleanup(temp_database("journal"));
        let root = LocationSettings { sort: Some(SortSpec { by: "size".into(), order: "desc".into() }) };
        let journal = LocationSettings { sort: Some(SortSpec { by: "date".into(), order: "asc".into() }) };
        save_to(&database.0, "/", &root).unwrap();
        save_to(&database.0, &key_for("journal", "", false).unwrap(), &journal).unwrap();
        let loaded = load_from(&database.0).unwrap();
        assert_eq!(loaded.get("/"), Some(&root));
        assert_eq!(loaded.get("source:journal"), Some(&journal));
        assert_eq!(sort_for(&loaded, "source:journal"), journal.sort.unwrap());
        assert_eq!(clear_sorts_from(&database.0).unwrap(), 2);
        assert!(load_from(&database.0).unwrap().is_empty());
    }

    fn temp_database(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "fbro-location-settings-{name}-{}.sqlite3",
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn creates_round_trips_and_upserts_location_settings() {
        let database = temp_database("round-trip");
        let location = normalize_local_path("/tmp/example/");
        let first = LocationSettings {
            sort: Some(SortSpec {
                by: "size".to_string(),
                order: "desc".to_string(),
            }),
        };
        save_to(&database, &location, &first).unwrap();
        assert_eq!(load_from(&database).unwrap().get(&location), Some(&first));

        let second = LocationSettings {
            sort: Some(SortSpec {
                by: "date".to_string(),
                order: "asc".to_string(),
            }),
        };
        save_to(&database, &location, &second).unwrap();
        assert_eq!(load_from(&database).unwrap().get(&location), Some(&second));

        let _ = std::fs::remove_file(database);
    }

    #[test]
    fn migrates_the_legacy_table_out_of_the_journal_database_once() {
        let database = temp_database("app-state");
        let journal = temp_database("journal");
        let connection = Connection::open(&journal).unwrap();
        connection
            .execute(
                "CREATE TABLE journal_events (event_id TEXT PRIMARY KEY)",
                [],
            )
            .unwrap();
        connection
            .execute("INSERT INTO journal_events VALUES ('event-1')", [])
            .unwrap();
        connection
            .execute(
                "CREATE TABLE location_settings (
                    path TEXT PRIMARY KEY NOT NULL,
                    sort_by TEXT,
                    sort_order TEXT
                )",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO location_settings (path, sort_by, sort_order)
                 VALUES ('/tmp/example', 'size', 'desc')",
                [],
            )
            .unwrap();
        drop(connection);

        let mut connection = open(&database).unwrap();
        migrate_legacy_table(&mut connection, &journal).unwrap();
        assert_eq!(
            load_from_connection(&connection)
                .unwrap()
                .get("/tmp/example"),
            Some(&LocationSettings {
                sort: Some(SortSpec {
                    by: "size".to_string(),
                    order: "desc".to_string(),
                }),
            })
        );
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        let journal_connection = Connection::open(&journal).unwrap();
        let journal_table_count: i64 = journal_connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'journal_events'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(journal_table_count, 1);
        let journal_event_count: i64 = journal_connection
            .query_row("SELECT COUNT(*) FROM journal_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_event_count, 1);
        let legacy_table_count: i64 = journal_connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'location_settings'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(legacy_table_count, 0);

        clear_sorts_from_connection(&connection).unwrap();
        migrate_legacy_table(&mut connection, &journal).unwrap();
        assert!(load_from_connection(&connection).unwrap().is_empty());

        drop(journal_connection);
        drop(connection);
        let _ = std::fs::remove_file(database);
        let _ = std::fs::remove_file(journal);
    }

    #[test]
    fn renames_the_version_one_fbro_table() {
        let database = temp_database("version-one");
        let missing_journal = temp_database("missing-journal");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "CREATE TABLE location_settings (
                    path TEXT PRIMARY KEY NOT NULL,
                    sort_by TEXT,
                    sort_order TEXT
                )",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO location_settings (path, sort_by, sort_order)
                 VALUES ('/tmp/version-one', 'date', 'asc')",
                [],
            )
            .unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        drop(connection);

        let mut connection = open(&database).unwrap();
        migrate_legacy_table(&mut connection, &missing_journal).unwrap();
        assert!(load_from_connection(&connection)
            .unwrap()
            .contains_key("/tmp/version-one"));
        let old_table_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'location_settings'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(old_table_count, 0);
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        drop(connection);
        let _ = std::fs::remove_file(database);
    }

    #[test]
    fn unseen_locations_use_name_ascending() {
        assert_eq!(sort_for(&HashMap::new(), "/tmp/unseen"), default_sort());
    }

    #[test]
    fn clears_all_remembered_sorts() {
        let database = temp_database("clear");
        for location in ["/tmp/one", "/tmp/two"] {
            save_to(
                &database,
                location,
                &LocationSettings {
                    sort: Some(SortSpec {
                        by: "size".to_string(),
                        order: "desc".to_string(),
                    }),
                },
            )
            .unwrap();
        }

        assert_eq!(clear_sorts_from(&database).unwrap(), 2);
        assert!(load_from(&database).unwrap().is_empty());

        let _ = std::fs::remove_file(database);
    }

    #[test]
    fn ignores_invalid_sort_values_from_an_existing_database() {
        let database = temp_database("invalid");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "CREATE TABLE settings_location (
                    path TEXT PRIMARY KEY NOT NULL,
                    sort_by TEXT,
                    sort_order TEXT
                )",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO settings_location (path, sort_by, sort_order)
                 VALUES ('/tmp/example', 'owner', 'sideways')",
                [],
            )
            .unwrap();
        drop(connection);

        assert!(load_from(&database).unwrap().is_empty());

        let _ = std::fs::remove_file(database);
    }

    #[test]
    fn normalizes_trailing_separators_without_resolving_aliases() {
        assert_eq!(normalize_local_path("/tmp/example/"), "/tmp/example");
        assert_eq!(normalize_local_path("/"), "/");
        assert_ne!(
            normalize_local_path("/tmp/link"),
            normalize_local_path("/tmp/real")
        );
    }

    #[test]
    fn retention_expires_after_thirty_days() {
        assert!(!retention_expired(1_000, 1_000 + RETENTION_MS - 1));
        assert!(retention_expired(1_000, 1_000 + RETENTION_MS));
    }
}

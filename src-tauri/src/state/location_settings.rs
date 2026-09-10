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

const CREATE_TABLE: &str = "CREATE TABLE IF NOT EXISTS location_settings (
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

fn load_from(path: &Path) -> Result<LocationSettingsMap, String> {
    let connection = open(path)?;
    let mut statement = connection
        .prepare("SELECT path, sort_by, sort_order FROM location_settings")
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
    let location = normalize_local_path(location);
    let (sort_by, sort_order) = settings
        .sort
        .as_ref()
        .filter(|sort| valid_sort(&sort.by, &sort.order))
        .map(|sort| (Some(sort.by.as_str()), Some(sort.order.as_str())))
        .unwrap_or((None, None));
    connection
        .execute(
            "INSERT INTO location_settings (path, sort_by, sort_order)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET
                 sort_by = excluded.sort_by,
                 sort_order = excluded.sort_order",
            params![location, sort_by, sort_order],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn load() -> Result<LocationSettingsMap, String> {
    load_from(&crate::journal::database_path())
}

pub fn save(location: &str, settings: &LocationSettings) -> Result<(), String> {
    save_to(&crate::journal::database_path(), location, settings)
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn coexists_with_an_existing_journal_database() {
        let database = temp_database("journal");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "CREATE TABLE journal_events (event_id TEXT PRIMARY KEY)",
                [],
            )
            .unwrap();
        drop(connection);

        assert!(load_from(&database).unwrap().is_empty());
        let connection = Connection::open(&database).unwrap();
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'journal_events'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        drop(connection);
        let _ = std::fs::remove_file(database);
    }

    #[test]
    fn unseen_locations_use_name_ascending() {
        assert_eq!(sort_for(&HashMap::new(), "/tmp/unseen"), default_sort());
    }

    #[test]
    fn ignores_invalid_sort_values_from_an_existing_database() {
        let database = temp_database("invalid");
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
}

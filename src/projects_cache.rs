use crate::errors::{Error, Result};
use sqlx::{migrate::MigrateDatabase, FromRow, QueryBuilder, Row, Sqlite, SqlitePool};
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};
use tracing::info;

/// A cache used to keep the list of known Project and
/// their relevant labels. Used only when the controller
/// is deployed inside of a downstream cluster.
///
/// It's leveraged when a Namespace is changed/created
/// and the connection towards the upstream cluster is broken.
///
/// The cache is backed by a sqlite database.
pub struct ProjectsCache {
    /// connection pool towards the the sqlite database
    pool: SqlitePool,
}

/// Internal struct, used to populate the results of a "get labels of project X"
/// sql query
#[derive(Clone, FromRow, Debug)]
struct Label {
    id: i64,
    key: String,
    value: String,
}

/// Internal struct, used when inserting data into the `project_labels`
/// table
struct LabelInsert {
    project_id: i64,
    key: String,
    value: String,
}

impl ProjectsCache {
    /// Create a new Cache object.
    ///
    /// Note: the unit tests will ignore the given `data_path` and use
    /// an in-memory sqlite database
    #[allow(unused_variables)]
    pub async fn init(data_path: &Path) -> Result<Self> {
        cfg_if::cfg_if! {
            if #[cfg(test)] {
                let db_url = ":memory:";
            } else {
                let db_url = Path::new("sqlite://").join(data_path).join("cache.sqlite");
                let db_url = db_url
                    .to_str()
                    .ok_or_else(|| Error::Internal("Cannot create path to sqlite file".to_string()))?;
            }
        }

        let pool = Self::setup_database(db_url).await?;
        Ok(ProjectsCache { pool })
    }

    /// Internal function, takes care of the following actions:
    /// * Create database file when needed
    /// * Handle database schema
    /// * Return connection pool towards the database
    async fn setup_database(db_url: &str) -> Result<SqlitePool> {
        if !Sqlite::database_exists(db_url).await.unwrap_or(false) {
            info!("Creating database {}", db_url);
            Sqlite::create_database(db_url)
                .await
                .map_err(|e| Error::Sqlite("database creation".to_string(), e))?
        }

        let db = SqlitePool::connect(db_url)
            .await
            .map_err(|e| Error::Sqlite("pool creation".to_string(), e))?;
        sqlx::query(
            r#"
        CREATE TABLE IF NOT EXISTS projects (
            id INTEGER PRIMARY KEY NOT NULL,
            name VARCHAR(250) NOT NULL);
        CREATE UNIQUE INDEX IF NOT EXISTS project_name ON projects(name);

        CREATE TABLE IF NOT EXISTS project_labels (
            id INTEGER PRIMARY KEY NOT NULL,
            project_id INTEGER,
            key VARCHAR(250) NOT NULL,
            value VARCHAR(250) NOT NULL,
            FOREIGN KEY(project_id) REFERENCES projects(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS project_id ON project_labels(project_id);
    "#,
        )
        .execute(&db)
        .await
        .map_err(|e| Error::Sqlite("schema creation".to_string(), e))?;

        Ok(db)
    }

    /// Cache the details of the given project:
    /// * `project_name`: name of the project
    /// * `labels`: the relevant labels that have to be propated. Important: the `propate.` prefix
    /// must be removed by the label keys
    pub async fn cache_labels(
        &self,
        project_name: &str,
        labels: &BTreeMap<String, String>,
    ) -> Result<()> {
        // begin transaction
        let mut transaction = self.pool.begin().await.map_err(|e| {
            Error::Sqlite("Update project labels, begin transaction".to_string(), e)
        })?;

        let row = sqlx::query("SELECT id from projects WHERE name = ?")
            .bind(project_name)
            .fetch_optional(&mut transaction)
            .await
            .map_err(|e| Error::Sqlite("get project id".to_string(), e))?;

        let project_id: i64 = match row {
            Some(row) => row
                .try_get("id")
                .map_err(|e| Error::Sqlite("Get id of existing project".to_string(), e))?,
            None => {
                let row = sqlx::query("INSERT INTO projects(name) VALUES (?) RETURNING id")
                    .bind(project_name)
                    .fetch_one(&mut transaction)
                    .await
                    .map_err(|e| Error::Sqlite("insert of project".to_string(), e))?;

                row.try_get("id")
                    .map_err(|e| Error::Sqlite("Get project id".to_string(), e))?
            }
        };

        let current_labels: Vec<Label> = sqlx::query_as::<_, Label>(
            "SELECT id, key, value
            FROM project_labels
            WHERE project_id = ?",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Sqlite("Get project labels".to_string(), e))?;

        let mut labels_to_remove: Vec<String> = Vec::new();
        let mut labels_already_up_to_date: HashSet<String> = HashSet::new();
        for label in &current_labels {
            match labels.get(&label.key) {
                None => labels_to_remove.push(label.id.to_string()),
                Some(desired_value) => {
                    if desired_value.as_str() != label.value {
                        // the label needs to be updated, we will just remove
                        // it and insert it again
                        labels_to_remove.push(label.id.to_string())
                    } else {
                        _ = labels_already_up_to_date.insert(label.key.clone());
                    }
                }
            }
        }

        // First, delete all the labels that are not around anymore or that have
        // to be updated
        if !labels_to_remove.is_empty() {
            sqlx::query("DELETE FROM project_labels WHERE id IN (?)")
                .bind(labels_to_remove.join(", "))
                .execute(&mut transaction)
                .await
                .map_err(|e| Error::Sqlite("Delete old labels".to_string(), e))?;
        }

        // Insert all the new/updated labels
        let labels_to_insert: Vec<LabelInsert> = labels
            .iter()
            .filter_map(|(key, value)| {
                if labels_already_up_to_date.contains(key) {
                    None
                } else {
                    Some(LabelInsert {
                        project_id,
                        key: key.clone(),
                        value: value.clone(),
                    })
                }
            })
            .collect();

        if !labels_to_insert.is_empty() {
            let mut query_builder: QueryBuilder<Sqlite> = QueryBuilder::new(
                // Note the trailing space; most calls to `QueryBuilder` don't automatically insert
                // spaces as that might interfere with identifiers or quoted strings where exact
                // values may matter.
                "INSERT INTO project_labels (project_id, key, value) ",
            );
            // Note: sqlite has a limit on the number of variables that can be binded.
            // According to https://www.sqlite.org/c3ref/bind_blob.html this limit is
            // equal to 32766.
            // In our case, each label requires 3 binds -> 32766 / 3 = 10922
            // which means we can insert at most 10922 labels at the same time.
            // A single Kubernetes object will never exceed this number.
            query_builder.push_values(labels_to_insert, |mut b, label| {
                b.push_bind(label.project_id)
                    .push_bind(label.key)
                    .push_bind(label.value);
            });
            let query = query_builder.build();
            query
                .execute(&mut transaction)
                .await
                .map_err(|e| Error::Sqlite("insert labels".to_string(), e))?;
        }

        transaction.commit().await.map_err(|e| {
            Error::Sqlite("Update project labels, commit transaction".to_string(), e)
        })?;

        Ok(())
    }

    /// List of labels that belong to the given project that have to be propagated.
    /// Returns `None` when the project is not found inside of the cache
    pub async fn labels_to_propagate(
        &self,
        project_name: &str,
    ) -> Result<Option<BTreeMap<String, String>>> {
        let labels: Vec<Label> = sqlx::query_as::<_, Label>(
            "SELECT project_labels.id, project_labels.key, project_labels.value
            FROM projects JOIN project_labels ON projects.id = project_labels.project_id
            WHERE projects.name = ?",
        )
        .bind(project_name)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Sqlite("get project labels".to_string(), e))?;

        if labels.is_empty() {
            return Ok(None);
        }

        Ok(Some(
            labels
                .iter()
                .map(|label| (label.key.clone(), label.value.clone()))
                .collect(),
        ))
    }

    /// Remove the given project from the cache
    pub async fn delete_project(&self, project_name: &str) -> Result<()> {
        sqlx::query("DELETE FROM projects WHERE name = ?")
            .bind(project_name)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Sqlite("Delete project".to_string(), e))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_init() {
        assert!(ProjectsCache::init(Path::new("not relevant")).await.is_ok());
    }

    #[tokio::test]
    async fn cache_labels() {
        let project_name = "test";
        let cache = ProjectsCache::init(Path::new("not relevant"))
            .await
            .expect("cannot create cache");

        let labels_evolution = vec![
            json!({
                "hello": "world",
                "hola": "mundo",
            }),
            json!({
                "hello": "world",
                "hola": "mundo",
                "ciao": "mondo",
            }),
            json!({
                "hola": "mundo",
                "ciao": "mondo",
                "hallo": "wereld",
            }),
            json!({
                "hola": "mundo",
                "ciao": "globo terracqueo",
                "hallo": "wereld",
            }),
        ];

        let mut round = 0;
        for labels_json in labels_evolution {
            let labels: BTreeMap<String, String> = serde_json::from_value(labels_json)
                .expect(format!("{round} - cannot init map from json").as_str());
            cache
                .cache_labels(project_name, &labels)
                .await
                .expect(format!("{round} - cannot cache labels").as_str());

            let actual_labels = cache
                .labels_to_propagate(project_name)
                .await
                .expect(format!("{round} cannot get cached labels").as_str());

            assert!(actual_labels.is_some(), "round {round}");
            let actual_labels = actual_labels.unwrap();
            assert_eq!(
                labels, actual_labels,
                "round {round}, expected = '{labels:?}', got = '{actual_labels:?}')"
            );

            round += 1;
        }
    }

    #[tokio::test]
    async fn labels_of_non_existing_project() {
        let project_name = "test";
        let cache = ProjectsCache::init(Path::new("not relevant"))
            .await
            .expect("cannot create cache");

        let labels = cache.labels_to_propagate(project_name).await;

        assert!(labels.is_ok());
        assert!(labels.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_non_existing_project() {
        let project_name = "test";
        let cache = ProjectsCache::init(Path::new("not relevant"))
            .await
            .expect("cannot create cache");
        let result = cache.delete_project(project_name).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn delete_project() {
        let project_name = "test";
        let cache = ProjectsCache::init(Path::new("not relevant"))
            .await
            .expect("cannot create cache");

        let labels: BTreeMap<String, String> =
            serde_json::from_value(json!({"hello": "world"})).expect("cannot init map from json");
        cache
            .cache_labels(project_name, &labels)
            .await
            .expect("cannot cache labels");

        let row = sqlx::query("SELECT COUNT(*) as count from project_labels")
            .fetch_one(&cache.pool)
            .await
            .expect("count error");
        let label_count: i64 = row.get("count");
        assert_eq!(1, label_count, "got {label_count} instead of 1");

        let result = cache.delete_project(project_name).await;
        assert!(result.is_ok());

        // verify cascade delete on foreign key
        let row = sqlx::query("SELECT COUNT(*) as count from project_labels")
            .fetch_one(&cache.pool)
            .await
            .expect("count error");
        let label_count: i64 = row.get("count");
        assert_eq!(0, label_count, "got {label_count} instead of 0");
    }
}

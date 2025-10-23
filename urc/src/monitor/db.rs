#![allow(dead_code)]

use std::fs;

use anyhow::Error;
use sqlx::{Any, Pool, any::install_default_drivers, pool::PoolOptions};

use crate::monitor::config::DatabaseConfig;

#[derive(Copy, Clone, Eq, PartialEq)]
enum DatabaseKind {
    Sqlite,
    MySql,
}

pub struct DataBase {
    pool: Pool<Any>,
    kind: DatabaseKind,
}

impl DataBase {
    pub async fn new(config: &DatabaseConfig) -> Result<Self, Error> {
        install_default_drivers();

        let (database_url, kind) = match config {
            DatabaseConfig::Sqlite { path } => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let url = format!("sqlite://{}?mode=rwc", path.to_string_lossy());
                (url, DatabaseKind::Sqlite)
            }
            DatabaseConfig::MySql { url } => (url.clone(), DatabaseKind::MySql),
        };

        let max_connections = match kind {
            DatabaseKind::Sqlite => 1,
            DatabaseKind::MySql => 5,
        };

        let pool = PoolOptions::<Any>::new()
            .max_connections(max_connections)
            .connect(&database_url)
            .await?;

        let db = Self { pool, kind };
        db.configure().await?;
        db.initialize_schema().await?;
        db.ensure_status_row().await?;
        Ok(db)
    }

    async fn configure(&self) -> Result<(), Error> {
        if self.kind == DatabaseKind::Sqlite {
            sqlx::query("PRAGMA journal_mode = OFF")
                .execute(&self.pool)
                .await?;
            sqlx::query("PRAGMA synchronous = NORMAL")
                .execute(&self.pool)
                .await?;
            sqlx::query("PRAGMA foreign_keys = ON")
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    async fn initialize_schema(&self) -> Result<(), Error> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS operators (
                registration_root VARCHAR(66) NOT NULL PRIMARY KEY,
                owner VARCHAR(255) NOT NULL,
                registered_at BIGINT NOT NULL,
                unregistered_at BIGINT,
                slashed_at BIGINT
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS signed_registrations (
                registration_root VARCHAR(66) NOT NULL,
                idx BIGINT NOT NULL,
                pubkeyXA VARCHAR(255) NOT NULL,
                pubkeyXB VARCHAR(255) NOT NULL,
                pubkeyYA VARCHAR(255) NOT NULL,
                pubkeyYB VARCHAR(255) NOT NULL,
                PRIMARY KEY (registration_root, idx),
                FOREIGN KEY (registration_root) REFERENCES operators(registration_root)
                    ON DELETE CASCADE
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS protocols (
                slasher VARCHAR(255) NOT NULL,
                registration_root VARCHAR(66) NOT NULL,
                opted_in_at BIGINT NOT NULL,
                opted_out_at BIGINT NOT NULL DEFAULT 0,
                committer VARCHAR(255) NOT NULL,
                PRIMARY KEY (slasher, registration_root),
                FOREIGN KEY (registration_root) REFERENCES operators(registration_root)
                    ON DELETE CASCADE
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        match self.kind {
            DatabaseKind::Sqlite => {
                sqlx::query(
                    r#"
                    CREATE TABLE IF NOT EXISTS status (
                        id INTEGER PRIMARY KEY CHECK (id = 0),
                        indexed_block INTEGER NOT NULL
                    )
                    "#,
                )
                .execute(&self.pool)
                .await?;
            }
            DatabaseKind::MySql => {
                sqlx::query(
                    r#"
                    CREATE TABLE IF NOT EXISTS status (
                        id BIGINT NOT NULL PRIMARY KEY,
                        indexed_block BIGINT NOT NULL,
                        CHECK (id = 0)
                    )
                    "#,
                )
                .execute(&self.pool)
                .await?;

                // Ensure legacy deployments migrate away from TINYINT definitions.
                sqlx::query("ALTER TABLE status MODIFY COLUMN id BIGINT NOT NULL")
                    .execute(&self.pool)
                    .await
                    .ok();
            }
        }

        Ok(())
    }

    async fn ensure_status_row(&self) -> Result<(), Error> {
        let status: Option<i64> = sqlx::query_scalar(
            r#"
            SELECT id FROM status WHERE id = 0
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;

        if status.is_none() {
            sqlx::query(
                r#"
                INSERT INTO status (id, indexed_block) VALUES (0, 0)
                "#,
            )
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }

    pub async fn get_indexed_block(&self) -> u64 {
        sqlx::query_as(
            r#"
            SELECT indexed_block FROM status WHERE id = 0
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or((0,))
        .0
        .try_into()
        .expect("Failed to get indexed block")
    }

    pub async fn update_status(&self, indexed_block: u64) -> Result<(), Error> {
        let indexed_block: i64 = indexed_block.try_into()?;
        sqlx::query(
            r#"
            UPDATE status SET indexed_block = ? WHERE id = 0
            "#,
        )
        .bind(indexed_block)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn insert_operator(
        &self,
        registration_root: &str,
        owner: String,
        registered_at: u64,
    ) -> Result<(), Error> {
        let registered_at: i64 = registered_at.try_into()?;
        sqlx::query(
            r#"
            INSERT INTO operators (
                registration_root, owner, registered_at
            ) VALUES (?, ?, ?)
            "#,
        )
        .bind(registration_root)
        .bind(owner)
        .bind(registered_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn insert_protocol(
        &self,
        registration_root: &str,
        slasher: String,
        committer: String,
        opt_in_at: u64,
    ) -> Result<(), Error> {
        let opt_in_at: i64 = opt_in_at.try_into()?;
        sqlx::query(
            r#"
            INSERT INTO protocols (
                slasher, registration_root, opted_in_at, opted_out_at, committer
            ) VALUES (?, ?, ?, 0, ?)
            "#,
        )
        .bind(slasher)
        .bind(registration_root)
        .bind(opt_in_at)
        .bind(committer)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn insert_signed_registrations(
        &self,
        registration_root: &str,
        idx: usize,
        pubkey_x_a: String,
        pubkey_x_b: String,
        pubkey_y_a: String,
        pubkey_y_b: String,
    ) -> Result<(), Error> {
        let idx: i64 = idx.try_into()?;
        sqlx::query(
            r#"
            INSERT INTO signed_registrations (
                registration_root, idx, pubkeyXA, pubkeyXB, pubkeyYA, pubkeyYB
            ) VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(registration_root)
        .bind(idx)
        .bind(pubkey_x_a)
        .bind(pubkey_x_b)
        .bind(pubkey_y_a)
        .bind(pubkey_y_b)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_operators_by_pubkey(
        &self,
        slasher: &str,
        validator_pubkey: (String, String, String, String),
    ) -> Result<Vec<(String, u8, String)>, Error> {
        let results = sqlx::query_as::<_, (String, i64, String)>(
            r#"
            SELECT DISTINCT
                sr.registration_root,
                sr.idx,
                p.committer
            FROM signed_registrations sr
            INNER JOIN protocols p ON sr.registration_root = p.registration_root
            WHERE p.slasher = ?
              AND sr.pubkeyXA = ?
              AND sr.pubkeyXB = ?
              AND sr.pubkeyYA = ?
              AND sr.pubkeyYB = ?
            "#,
        )
        .bind(slasher)
        .bind(validator_pubkey.0)
        .bind(validator_pubkey.1)
        .bind(validator_pubkey.2)
        .bind(validator_pubkey.3)
        .fetch_all(&self.pool)
        .await?;

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let operators = results
            .into_iter()
            .map(|(root, leaf_index, committer)| (root, leaf_index as u8, committer))
            .collect();

        Ok(operators)
    }

    pub async fn get_registration_root_by_committer(
        &self,
        slasher: &str,
        committer: &str,
    ) -> Result<Vec<String>, Error> {
        let results = sqlx::query_scalar::<_, String>(
            r#"
            SELECT DISTINCT
                registration_root
            FROM protocols
            WHERE slasher = ?
              AND committer = ?
            "#,
        )
        .bind(slasher)
        .bind(committer)
        .fetch_all(&self.pool)
        .await?;

        Ok(results)
    }
}

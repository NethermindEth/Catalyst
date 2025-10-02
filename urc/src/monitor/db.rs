#![allow(dead_code)]

use anyhow::Error;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

pub struct DataBase {
    pool: SqlitePool,
}

impl DataBase {
    pub async fn new(db_filename: &str) -> Result<Self, Error> {
        let options = SqliteConnectOptions::new()
            .journal_mode(SqliteJournalMode::Off)
            .synchronous(SqliteSynchronous::Normal)
            .filename(db_filename)
            .create_if_missing(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;

        // Create operators table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS operators (
                registration_root TEXT PRIMARY KEY,
                owner TEXT NOT NULL,
                registered_at INTEGER NOT NULL,
                unregistered_at INTEGER,
                slashed_at INTEGER
            );
            "#,
        )
        .execute(&pool)
        .await?;

        // Create signed_registrations table
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS signed_registrations (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    registration_root TEXT NOT NULL,
                    idx INTEGER NOT NULL,
                    pubkeyXA TEXT NOT NULL,
                    pubkeyXB TEXT NOT NULL,
                    pubkeyYA TEXT NOT NULL,
                    pubkeyYB TEXT NOT NULL,
                    FOREIGN KEY (registration_root) REFERENCES operators(registration_root)
                );
                CREATE INDEX IF NOT EXISTS idx_signed_registrations_root
                    ON signed_registrations (registration_root);
            "#,
        )
        .execute(&pool)
        .await?;

        // Create slashers table
        sqlx::query(
            r#"
                CREATE TABLE IF NOT EXISTS protocols (
                    slasher TEXT NOT NULL,
                    registration_root TEXT NOT NULL,
                    opted_in_at INTEGER NOT NULL,
                    opted_out_at INTEGER NOT NULL,
                    committer TEXT NOT NULL,
                    PRIMARY KEY (slasher, registration_root),
                    FOREIGN KEY (registration_root) REFERENCES operators(registration_root)
                );
            "#,
        )
        .execute(&pool)
        .await?;

        // Create status table (only one row allowed)
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS status (
                id               INTEGER PRIMARY KEY CHECK (id = 0),
                indexed_block    INTEGER NOT NULL
            )
            "#,
        )
        .execute(&pool)
        .await?;

        // Insert status if not exist
        let status = sqlx::query(
            r#"
            SELECT id FROM status WHERE id = 0
            "#,
        )
        .fetch_optional(&pool)
        .await?;
        if status.is_none() {
            sqlx::query(
                r#"
                INSERT INTO status (id, indexed_block)
                VALUES (0, 0)
                "#,
            )
            .execute(&pool)
            .await?;
        }

        Ok(Self { pool })
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
            UPDATE status SET
                indexed_block = ?
            WHERE id = 0
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
        let results = sqlx::query_as::<_, (String, u64, String)>(
            r#"
            SELECT DISTINCT 
                sr.registration_root
                sr.idx
                p.committer
            FROM signed_registrations sr
            INNER JOIN protocols p ON sr.registration_root = p.registration_root
            WHERE p.slasher = ?
            AND (
                sr.pubkeyXA = ? AND 
                sr.pubkeyXB = ? AND 
                sr.pubkeyYA = ? AND 
                sr.pubkeyYB = ?
            )
            "#,
        )
        .bind(slasher)
        .bind(validator_pubkey.0)
        .bind(validator_pubkey.1)
        .bind(validator_pubkey.2)
        .bind(validator_pubkey.3)
        .fetch_all(&self.pool)
        .await?;

        #[allow(clippy::cast_possible_truncation)]
        let operators = results
            .into_iter()
            .map(|(root, leaf_index, committer)| (root, leaf_index as u8, committer))
            .collect();

        Ok(operators)
    }
}

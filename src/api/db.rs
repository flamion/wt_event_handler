use sqlx::{ConnectOptions, Encode, Executor, Pool, query, query_file, query_file_as_unchecked, query_file_unchecked, Row, Sqlite, SqliteConnection, SqlitePool};
use std::str::FromStr;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteRow};
use crate::api::db_error::DatabaseError;

use sqlx::migrate::Migrator;

#[derive(Clone)]
pub struct Database {
	pub connection: SqlitePool,
}

impl Database {
	pub async fn new() -> Result<Self, DatabaseError> {
		let options = SqliteConnectOptions::from_str("sqlite::memory:")?
			.create_if_missing(true)
			.shared_cache(true)
			.journal_mode(SqliteJournalMode::Wal);
		let mut db = SqlitePool::connect_with(options).await?;

		db.execute(include_str!("../../assets/setup_db.sql")).await?;

		Ok(Self {
			connection: db
		})
	}
	pub async fn store_recent_single(&self, value: &str, source: &str) -> Result<(), DatabaseError>
	{
		let now = chrono::Utc::now().timestamp();
			let q = query!("INSERT INTO sources (url, fetch_date, source)
						VALUES (?, ?, ?);",
						value, now, source);
			self.connection.execute(q).await?;
		Ok(())
	}

	pub async fn store_recent<I>(&self, values: I, source: &str) -> Result<(), DatabaseError>
		where I: IntoIterator,
			I::Item: ToString
	{
		for value in values {
			self.store_recent_single(&value.to_string(), source).await?;
		}
		Ok(())
	}

	pub async fn get_latest_news_from_source(&self, source_name: &str) -> Result<String, DatabaseError> {
		let q = query!("SELECT url
						FROM sources
						WHERE source = ?
						ORDER BY fetch_date DESC", source_name);
		Ok(self.connection.fetch_one(q).await?.get(0))
	}
}
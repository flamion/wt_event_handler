use sqlx::SqlitePool;

#[derive(Clone)]
pub struct Database {
	pub auth: SqlitePool,
	pub recent: SqlitePool,
}
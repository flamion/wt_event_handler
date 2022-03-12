use sqlx::SqlitePool;

#[derive(Clone)]
pub struct Database {
	pub db: SqlitePool,
}
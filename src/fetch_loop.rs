use std::fs;
use std::process::exit;
use std::time::Duration;

use actix_cors::Cors;
use actix_web::{App, HttpServer};
use actix_web::web::Data;
use lazy_static::lazy_static;
use tokio::signal;
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use crate::api::database::Database;
use crate::api::endpoints::{get_latest_news, get_latest_timestamp, get_uptime, greet, shutdown};

use crate::error::{error_webhook, NewsError};
use crate::json::sources::Sources;
use crate::scrapers::html_processing::html_processor;
use crate::scrapers::scraper_resources::resources::ScrapeType;
use crate::statistics::{Incr, increment, Statistics};
use crate::timeout::Timeout;

const FETCH_DELAY: u64 = 48;

pub const STAT_COOLDOWN_HOURS: u64 = 24;
// in seconds
const STAT_COOL_DOWN: u64 = 60 * 60 * STAT_COOLDOWN_HOURS;


lazy_static! {
	pub static ref STATS: Mutex<Statistics> = Mutex::new(Statistics::new());
}

pub async fn fetch_loop(hooks: bool) {
	let database = Database::new().await.expect("Cannot initiate DB");
	let mut sources = Sources::build(&database).await.expect("I fucked up my soup");

	#[cfg(debug_assertions)]
	sources.debug_remove_tracked_urls(&["a"]);

	let mut timeouts = Timeout::new();

	// Spawn statistics thread
	tokio::task::spawn(async {
		warn!("Spawned logging thread");
		loop {
			tokio::time::sleep(Duration::from_secs(STAT_COOL_DOWN)).await;
			let mut stats = STATS.lock().await;
			stats.post().await;
			stats.reset();
		}
	});

	// Spawn API thread
	tokio::task::spawn({
		let cloned_database = Data::new( database.clone());
		info!("Spawned API thread");
		HttpServer::new(move || {
			let cors = Cors::default()
				.allow_any_origin()
				.allowed_methods(vec!["GET", "POST"]);

			App::new()
				.wrap(cors)
				.app_data(Data::clone(&cloned_database))
				.service(greet)
				.service(get_latest_news)
				.service(shutdown)
				.service(get_latest_timestamp)
				.service(get_uptime)
		})
			.bind(("127.0.0.1", 8082))
			.expect("Cant bind local host on port 8080")
			.run()
	});

	// Responsible for shutting down tokio-parent / sibling processes
	tokio::spawn(async move {
		tokio::signal::ctrl_c().await.unwrap();
		exit(-1);
	});


	loop {
		for source in &mut sources.sources {
			if !timeouts.is_timed_out(&source.name) {
				increment(Incr::FetchCounter).await;
				match html_processor(source).await {
					Ok(news) => {
						for news_embed in &news {
							if hooks {
								source.handle_webhooks(news_embed, true, source.scrape_type).await;
							}
							increment(Incr::NewNews).await;
						}

						source.store_recent(news.iter().map(|new| &new.url));
						database.store_recent(news.iter().map(|new| &new.url), &source.name).await;
					}
					Err(e) => {
						increment(Incr::Errors).await;
						handle_err(e, source.scrape_type, source.name.clone(), &mut timeouts, hooks).await;
					}
				}
			}
			info!("Waiting for {FETCH_DELAY} seconds");
			tokio::time::sleep(Duration::from_secs(FETCH_DELAY)).await;
		}


		//Aborts program after running without hooks
		if !hooks {
			exit(0);
		}
	}
}

/// Throws error as webhook, times out pages accordingly and terminates program if unrecoverable
async fn handle_err(e: NewsError, scrape_type: ScrapeType, source: String, timeouts: &mut Timeout, hooks: bool) {
	error!("{e}");
	let crash_and_burn = |e: NewsError| async move {
		if hooks {
			error_webhook(&e, "The bot is now offline and needs investigation", false).await;
		}
		panic!("{:?}", e);
	};

	let time_out = |send_webhook_error_message, msg: String| async move {
		let now = chrono::offset::Utc::now().timestamp();
		let then = now + (60 * 30);
		if send_webhook_error_message {
			error_webhook(&NewsError::SourceTimeout(scrape_type, msg, then), "", true).await;
		}
		let _ = &timeouts.time_out(source, then);
	};

	#[allow(clippy::match_wildcard_for_single_variants)]
	match e {
		NewsError::NoUrlOnPost(name, html) => {
			let now = chrono::Local::now().timestamp();
			let sanitized_url = name.replace('/', "_").replace(':', "_");
			drop(fs::write(&format!("/log/err_html/{sanitized_url}_{now}.html"), html));
			time_out(true, "no_url_on_post".to_owned()).await;
		}
		NewsError::MetaCannotBeScraped(scrape_type) => {
			error_webhook(&e, &scrape_type.to_string(), true).await;
		}
		NewsError::SourceTimeout(_, _, _) => {
			// Dont do anything as it should've been handled earlier
		}
		NewsError::BadSelector(ref selector) => {
			error_webhook(&e, &format!("Selector: {selector}"), true).await;
		}
		NewsError::MonthParse(_) => {
			time_out(true, e.to_string()).await;
		}
		NewsError::SelectedNothing(source, _) => {
			time_out(true, source).await;
		}
		NewsError::SerenityError(_) => {
			error_webhook(&e, "", true).await;
		}
		NewsError::Reqwest(e) => {
			let status = e.status();
			let status_text = if let Some(status) = status {
				format!("status: {status} was returned and initiated:")
			} else {
				"no status code related error was returned and initiated:".to_owned()
			};
			match () {
				_ if e.is_builder() => {
					time_out(true, format!("{status_text} reqwest_bad_builder: {e}")).await;
				}
				_ if e.is_redirect() => {
					time_out(true, format!("{status_text} reqwest_bad_redirect: {e}")).await;
				}
				_ if e.is_status() => {
					time_out(true, format!("{status_text} reqwest_bad_status_{e}: {e}")).await;
				}
				_ if e.is_timeout() => {
					// Timeouts happen too often, they are no longer printed out status channels
					time_out(false, format!("{status_text} reqwest_timeout: {e}")).await;
				}
				_ if e.is_request() => {
					time_out(true, format!("{status_text} reqwest_bad_request: {e}")).await;
				}
				_ if e.is_connect() => {
					time_out(true, format!("{status_text} reqwest_bad_connect: {e}")).await;
				}
				_ if e.is_body() => {
					time_out(true, format!("{status_text} reqwest_bad_body: {e}")).await;
				}
				_ if e.is_decode() => {
					time_out(true, format!("{status_text} reqwest_bad_body: {e}")).await;
				}
				_ => {
					time_out(true, format!("{status_text} reqwest_everything_bad: {e}")).await;
				}
			}
		}
		NewsError::SerdeJson(_) => {
			todo!("Impelent this!");
		}
		_ => {
			crash_and_burn(e).await;
		}
	}

}

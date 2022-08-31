use std::error::Error;
use std::fs;
use std::process::exit;
use std::sync::Arc;
use std::time::Duration;

use actix_web::{App, HttpServer, web};
use lazy_static::lazy_static;
use tokio::sync::{Mutex};
use tracing::{error, info, warn};

use actix_cors::Cors;

use crate::api::core::get_latest_news;
use crate::api::core::greet;
use crate::error::{error_webhook, InputError, NewsError};
use crate::json::recent::Sources;
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
	let recent_data_raw = Sources::build_from_drive().await.expect("I fucked up my soup");

	#[cfg(debug_assertions)]
	{
		let to_remove_urls: &[&str] = &[];
		for to_remove in to_remove_urls {
			for source in &mut recent_data_raw.sources {
				source.tracked_urls.write().await.remove(to_remove.to_owned());
			}
		}
	}

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

	let recent_data = Arc::new(recent_data_raw);
	let temp = recent_data.clone();
	let test_struct_data = web::Data::from(temp);

	// Spawn API thread
	tokio::task::spawn({
		info!("Spawned API thread");
		HttpServer::new(move || {
			let cors = Cors::default()
				.allow_any_origin()
				.allowed_methods(vec!["GET", "POST"]);

			App::new()
				.wrap(cors)
				.app_data(test_struct_data.clone())
				.service(greet)
				.service(get_latest_news)
		})
			.bind(("127.0.0.1", 8080))
			.expect("Cant bind local host on port 8080")
			.run()
	});


	loop {
		for source in &recent_data.sources {
			if !timeouts.is_timed_out(&source.name) {
				increment(Incr::FetchCounter).await;
				match html_processor(source).await {
					Ok(news) => {
						for news_embed in &news {
							if hooks {
								source.handle_webhooks(&news_embed, true, source.scrape_type).await;
							}
							increment(Incr::NewNews).await;
						}

						if let Err(why) = source.store_recent(news.into_iter().map(|new| new.url)).await {
							error_webhook(&why, true).await; // it's recoverable, but the API is fucked up then (Outdated Data)
						}
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
async fn handle_err(e: Box<dyn Error>, scrape_type: ScrapeType, source: String, timeouts: &mut Timeout, hooks: bool) {
	error!(e);
	let crash_and_burn = |e: InputError| async move {
		if hooks {
			error_webhook(&e, false).await;
		}
		panic!("{:?}", e);
	};

	let time_out = |send, msg: String| async move {
		let now = chrono::offset::Utc::now().timestamp();
		let then = now + (60 * 30);
		if send {
			error_webhook(&Box::new(NewsError::SourceTimeout(scrape_type, msg, then)).into(), true).await;
		}
		let _ = &timeouts.time_out(source, then);
	};

	match () {
		_ if let Some(e) = e.downcast_ref::<reqwest::Error>() => {
			let e: &reqwest::Error = e;

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
		_ if let Some(variant) = e.downcast_ref::<NewsError>() => {
			match variant {
				NewsError::NoUrlOnPost(name, html) => {
					let now = chrono::Local::now().timestamp();
					let sanitized_url = name.replace('/', "_").replace(':', "_");
					drop(fs::write(&format!("/log/err_html/{sanitized_url}_{now}.html"), html));
					time_out(true, "no_url_on_post".to_owned()).await;
				}
				_ => {
					crash_and_burn(e).await;
				}
			}
		}
		_ => {
			crash_and_burn(e).await;
		}
	}
}

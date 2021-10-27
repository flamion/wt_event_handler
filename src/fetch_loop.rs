use core::time;
use std::process::exit;
use std::thread::sleep;

use log::info;
use rand::Rng;

use crate::json::recent::Recent;
use crate::scrapers::main_news::html_processor;
use crate::scrapers::scraper_resources::resources::ScrapeType;

pub async fn fetch_loop(hooks: bool) {
	let mut recent_data = Recent::read_latest();

	loop {
		if let Some(wt_news_content) = html_processor(&recent_data.warthunder_news, ScrapeType::Main).await {
			if recent_data.warthunder_news.is_outdated(&wt_news_content) {
				if hooks {
					recent_data.warthunder_news.handle_wt_news_webhook(&wt_news_content).await;
				}
				recent_data.append_latest_warthunder_news(&wt_news_content);
				println!("All wt news hooks are served");
				info!("All wt news hooks are served");
				if hooks {
					continue;
				}
			}
		};

		if let Some(wt_changelog) = html_processor(&recent_data.warthunder_changelog, ScrapeType::Main).await {
			if recent_data.warthunder_changelog.is_outdated(&wt_changelog) {
				if hooks {
					recent_data.warthunder_changelog.handle_wt_news_webhook(&wt_changelog).await;
				}
				recent_data.append_latest_warthunder_changelog(&wt_changelog);
				println!("All wt changelog hooks are served");
				info!("All wt changelog hooks are served");
				if hooks {
					continue;
				}
			}
		};

		if let Some(forum_news_updates_information) = html_processor(&recent_data.forums_updates_information, ScrapeType::Forum).await {
			if recent_data.forums_updates_information.is_outdated(&forum_news_updates_information) {
				if hooks {
					recent_data.forums_updates_information.handle_simple_webhook(&forum_news_updates_information).await;
				}
				recent_data.append_latest_warthunder_forums_updates_information(&forum_news_updates_information);
				println!("All forum_updates_information hooks are served");
				info!("All forum_updates_information hooks are served");
				if hooks {
					continue;
				}
			}
		};

		if let Some(forum_news_project_news) = html_processor(&recent_data.forums_project_news, ScrapeType::Forum).await {
			if recent_data.forums_project_news.is_outdated(&forum_news_project_news) {
				if hooks {
					recent_data.forums_project_news.handle_simple_webhook(&forum_news_project_news).await;
				}
				recent_data.append_latest_warthunder_forums_project_news(&forum_news_project_news);
				println!("All forum_project_news hooks are served");
				info!("All forum_project_news hooks are served");
				if hooks {
					continue;
				}
			}

			//Aborts program after running without hooks
			if !hooks {
				exit(0);
			}

			// Cool down to prevent rate limiting and excessive performance impact
			let wait = rand::thread_rng().gen_range(50..70);
			println!("Waiting for {} seconds", wait);
			info!("Waiting for {} seconds", wait);
			sleep(time::Duration::from_secs(wait));
		}
	}
}
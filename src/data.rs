use std::{
    collections::{HashMap, HashSet},
    env,
    sync::Arc,
    time::Duration,
};

use chrono::{NaiveDate, Timelike};
use chrono_tz::Europe::Berlin;
use serde::Deserialize;
use tokio::{
    fs::{File, OpenOptions},
    io::{AsyncReadExt, AsyncWriteExt},
    sync::RwLock,
};

const DATA_DIRECTORY: &str = "data";
const DATASET_HIST_URL: &str = "https://www.ecb.europa.eu/stats/eurofxref/eurofxref-hist.xml";

pub type SharedDataset = Arc<RwLock<Dataset>>;

pub type Currency = &'static str;
pub const EUR: Currency = "EUR";

#[derive(Clone)]
pub struct Dataset {
    pub days: Vec<Day>,
    pub currencies: &'static [Currency],
}

impl Dataset {
    pub fn timeframe(&self) -> Option<[NaiveDate; 2]> {
        let first = self.days.first()?;
        let last = self.days.last()?;

        Some([first.date, last.date])
    }

    /// Convert a currency code to a static one from the dataset
    pub fn from(&self, from: &String) -> Option<&'static str> {
        let index = self.currencies.binary_search(&from.as_str()).ok()?;

        Some(self.currencies[index])
    }
}

#[derive(Clone)]
pub struct Day {
    pub date: NaiveDate,

    /// List of rates for every currency in the dataset.
    /// Currencies that did not exist at the time will be `None`
    pub rates: Vec<Option<f64>>,
}

impl Day {
    pub fn convert(self, from: &'static str, currencies: &'static [Currency]) -> Option<Self> {
        // We do not need to convert if the base currency is EUR
        // ECB publishes the rates with Euro as the base currency
        if from == EUR {
            return Some(self);
        }

        // Find the index of the base currency
        let from = currencies.binary_search(&from).ok()?;
        // Get the base currency rate
        let from_rate = self.rates.get(from)?.as_ref()?.clone();
        // Convert all the rates
        let rates = self
            .rates
            .into_iter()
            .map(|rate| rate.map(|r| r / from_rate))
            .collect::<Vec<_>>();

        Some(Self { rates, ..self })
    }

    /// Turns the day rates into a `HashMap` with currency codes as keys
    pub fn to_hashmap(&self, currencies: &'static [Currency]) -> HashMap<String, Option<f64>> {
        currencies
            .into_iter()
            .map(ToString::to_string)
            .zip(self.rates.clone())
            .collect::<HashMap<_, _>>()
    }
}

async fn cache_file() -> Option<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(DATA_DIRECTORY.to_string() + "/dataset.xml")
        .await
        .ok()
}

pub async fn dataset() -> anyhow::Result<SharedDataset> {
    let dataset = match cache_file().await {
        // If we have no cached version of the dataset, download it
        None => download_dataset().await?,
        // Otherwise, read, parse, and return the cached version
        Some(mut file) => {
            let mut data = String::new();
            file.read_to_string(&mut data).await?;

            match parse_dataset(data).await {
                Err(_) => download_dataset().await?,
                Ok(dataset) => {
                    let today = chrono::Utc::now().with_timezone(&Berlin).date_naive();

                    // However, when the cached version is outdated, download a new one
                    if let Some(true) = dataset.days.last().map(|day| day.date < today) {
                        log::warn!("Dataset might be outdated, downloading a new one");
                        download_dataset().await?
                    } else {
                        log::info!("Using cached dataset");
                        dataset
                    }
                }
            }
        }
    };

    Ok(Arc::new(RwLock::new(dataset)))
}

pub async fn schedule_dataset_update(dataset: SharedDataset) {
    let update_at = env::var("UPDATE_AT")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        // ECB rates are usually updated at 16:00 CET, but we use 18:00 CET,
        // just to be sure we actually get the newest rates
        .unwrap_or(18 * 60);

    loop {
        let berlin_now = chrono::Utc::now().with_timezone(&Berlin);
        let berlin_minute = berlin_now.hour() * 60 + berlin_now.minute();

        // Check if we need to wait until tomorrow to prevent integer overflow
        let next_update_in = if berlin_minute >= update_at {
            // If it has been 18:00 CET, wait until the next day
            update_at - berlin_minute + 24 * 60
        } else {
            // Otherwise, wait until 18:00 CET today
            update_at - berlin_minute
        } as u64
            * 60;

        log::info!(
            "Updates scheduled every day at {:02}:{:02} CET",
            update_at / 60,
            update_at % 60
        );

        log::debug!("Next update in {} seconds", next_update_in);
        tokio::time::sleep(Duration::from_secs(next_update_in)).await;

        if update_at == berlin_minute {
            match download_dataset().await {
                Ok(new_dataset) => {
                    let mut lock = dataset.write().await;
                    *lock = new_dataset
                }
                Err(e) => log::error!(
                    "Failed to update dataset, using yesterday's\n{:ident$}",
                    e,
                    ident = 2
                ),
            }
        }
    }
}

async fn download_dataset() -> anyhow::Result<Dataset> {
    log::info!("Downloading dataset");

    let response = reqwest::get(DATASET_HIST_URL).await?.text().await?;

    // Cache the response
    if let Some(mut file) = cache_file().await {
        let _ = file.write_all(response.as_bytes()).await;
        let _ = file.flush().await;
    }

    let dataset = parse_dataset(response).await;

    log::info!("Downloaded dataset");

    dataset
}

async fn parse_dataset(data: String) -> anyhow::Result<Dataset> {
    tokio::task::spawn_blocking(move || {
        let xml_document: XmlDocument = quick_xml::de::from_str(&data)?;

        let mut currencies = HashSet::new();

        // Fill the currencies `HashSet`
        currencies.insert("EUR".to_string());
        for day in xml_document.data.days.iter() {
            for rate in day.rates.iter() {
                currencies.insert(rate.currency.clone());
            }
        }

        // Turn the currencies into a `Vec` and sort them
        let mut currencies = currencies.into_iter().collect::<Vec<String>>();
        currencies.sort();

        // Unwrapping is safe because we add it to the `HashSet` above
        let eur_index = currencies.binary_search(&"EUR".to_string()).unwrap();

        let mut days = Vec::new();

        // For every day,
        for mut xml_day in xml_document.data.days {
            let mut day = Day {
                date: xml_day.date,
                rates: vec![None; currencies.len()],
            };

            // sort the rates,
            xml_day.rates.sort_by_key(|rate| rate.currency.clone());

            // and set the Euro rate to 1.0,
            day.rates[eur_index] = Some(1.0);
            for rate in xml_day.rates {
                // and then set all supported currencies
                if let Ok(index) = currencies.binary_search(&&rate.currency) {
                    day.rates[index] = Some(rate.rate);
                }
            }

            days.push(day);
        }

        // Reverse the days so that the oldest day is first
        days.reverse();

        // Build a static slice of static currency codes
        let currencies: &'static [&str] = currencies
            .into_iter()
            .map(|c| -> &'static str { c.leak() })
            .collect::<Vec<_>>()
            .leak();

        Ok(Dataset { days, currencies })
    })
    .await?
}

#[derive(Debug, Deserialize)]
struct XmlDocument {
    #[serde(rename = "Cube")]
    data: XmlCube,
}

#[derive(Debug, Deserialize)]
struct XmlCube {
    #[serde(rename = "Cube")]
    days: Vec<XmlDay>,
}

#[derive(Debug, Deserialize)]
struct XmlDay {
    #[serde(rename = "@time")]
    date: NaiveDate,
    #[serde(rename = "$value")]
    rates: Vec<XmlRate>,
}

#[derive(Debug, Deserialize)]
struct XmlRate {
    #[serde(rename = "@currency")]
    currency: String,
    #[serde(rename = "@rate")]
    rate: f64,
}

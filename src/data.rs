use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use chrono::NaiveDate;
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

// impl FromStr for Currency {
//     type Err = anyhow::Error;

//     fn from_str(s: &str) -> Result<Self, Self::Err> {
//         let mut chars = s.chars();

//         return if let [Some(a), Some(b), Some(c)] = [chars.next(), chars.next(), chars.next()] {
//             // The string is three characters long
//             Ok(Self { code: [a, b, c] })
//         } else {
//             Err(anyhow::anyhow!(
//                 "Currency codes are 3 characters long and {} is not",
//                 s
//             ))
//         };
//     }
// }

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

    pub fn from(&self, from: &String) -> Option<&'static str> {
        let index = self.currencies.binary_search(&from.as_str()).ok()?;

        Some(self.currencies[index])
    }
}

#[derive(Clone)]
pub struct Day {
    pub date: NaiveDate,
    pub rates: Vec<Option<f64>>,
}

impl Day {
    pub fn convert(self, from: &'static str, currencies: &'static [Currency]) -> Option<Self> {
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
    pub fn to_hashmap(&self, currencies: &'static [Currency]) -> HashMap<String, Option<f64>> {
        currencies
            .into_iter()
            .map(ToString::to_string)
            .zip(self.rates.clone())
            .collect::<HashMap<_, _>>()
    }
}

async fn dataset_file() -> Option<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(DATA_DIRECTORY.to_string() + "/dataset.xml")
        .await
        .ok()
}

pub async fn dataset() -> anyhow::Result<SharedDataset> {
    let dataset = match dataset_file().await {
        // If we have no cached version of the dataset, download it
        None => download_dataset().await?,
        // Otherwise, read, parse and return the cached version
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
    // tokio::spawn(async move {
    //     loop {
    //         let at = 1080;
    //         let berlin_now = chrono::Utc::now().with_timezone(&Berlin);
    //         let berlin_minute = berlin_now.hour() * 60 + berlin_now.minute();

    //         if at == berlin_minute {
    //             let mut lock = dataset.write().await;
    //             *lock = download_dataset().await.unwrap_or(lock.clone());
    //         }
    //     }
    // });
}

async fn download_dataset() -> anyhow::Result<Dataset> {
    log::info!("Downloading dataset");

    let response = reqwest::get(DATASET_HIST_URL).await?.text().await?;

    if let Some(mut file) = dataset_file().await {
        let _ = file.write_all(response.as_bytes()).await;
        let _ = file.flush().await;
    }

    parse_dataset(response).await
}

async fn parse_dataset(data: String) -> anyhow::Result<Dataset> {
    tokio::task::spawn_blocking(move || {
        let xml_document: XmlDocument = quick_xml::de::from_str(&data)?;

        let mut currencies = HashSet::new();

        // Fill the symbols `HashSet`
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

        for mut xml_day in xml_document.data.days {
            let mut day = Day {
                date: xml_day.date,
                rates: vec![None; currencies.len()],
            };

            xml_day.rates.sort_by_key(|rate| rate.currency.clone());

            day.rates[eur_index] = Some(1.0);
            for rate in xml_day.rates {
                if let Ok(index) = currencies.binary_search(&&rate.currency) {
                    day.rates[index] = Some(rate.rate);
                }
            }

            days.push(day);
        }

        // Reverse the days so that the oldest day is first
        days.reverse();

        let currencies: &'static [&'static str] = currencies
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

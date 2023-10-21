use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use serde::Deserialize;

const DATA_DIRECTORY: &str = "data";
const DATASET_HIST_URL: &str = "https://www.ecb.europa.eu/stats/eurofxref/eurofxref-hist.xml";

pub struct Dataset {
    pub rates: Vec<(NaiveDate, HashMap<Currency, f64>)>,
    pub symbols: Vec<String>,
}

pub type Currency = String;

pub async fn dataset() -> anyhow::Result<Dataset> {
    download_dataset().await
}

async fn download_dataset() -> anyhow::Result<Dataset> {
    let response = reqwest::get(DATASET_HIST_URL).await?.text().await?;

    parse_dataset(response).await
}

async fn parse_dataset(data: String) -> anyhow::Result<Dataset> {
    tokio::task::spawn_blocking(move || {
        let xml_document: XmlDocument = quick_xml::de::from_str(&data)?;

        let mut rates = Vec::new();
        let mut symbols = HashSet::new();

        for mut day in xml_document.data.days {
            day.rates.push(Rate {
                currency: "EUR".to_string(),
                rate: 1.0,
            });

            day.rates.sort_by_cached_key(|r| r.currency.clone());

            rates.push((
                day.date,
                day.rates
                    .iter()
                    .map(|rate| {
                        symbols.insert(rate.currency.clone());
                        (rate.currency.clone(), rate.rate)
                    })
                    .collect(),
            ));
        }

        rates.reverse();

        let mut symbols: Vec<String> = symbols.into_iter().collect();
        symbols.sort();

        Ok(Dataset { rates, symbols })
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
    days: Vec<Day>,
}

#[derive(Debug, Deserialize)]
struct Day {
    #[serde(rename = "@time")]
    date: NaiveDate,
    #[serde(rename = "$value")]
    rates: Vec<Rate>,
}

#[derive(Debug, Deserialize)]
struct Rate {
    #[serde(rename = "@currency")]
    currency: Currency,
    #[serde(rename = "@rate")]
    rate: f64,
}

use std::{collections::HashMap, sync::Arc};

use chrono::NaiveDate;
use poem::web::Data;
use poem_openapi::{
    payload::Json,
    types::{ToJSON, Type},
    ApiResponse, Object, OpenApi,
};
use reqwest::StatusCode;
use tokio::sync::RwLock;

use crate::data::{Currency, Dataset};

type SharedDataset = Arc<RwLock<Dataset>>;

pub struct Api;

#[derive(Object)]
struct IndexResponse {
    symbols: Vec<String>,
    timeframe: [NaiveDate; 2],
}

#[derive(Object)]
struct RatesRequest {
    date: Option<NaiveDate>,
    from: Option<Currency>,
    to: Option<Vec<Currency>>,
}

#[derive(ApiResponse)]
enum RatesResponse<T: Send + Type + ToJSON> {
    #[oai(status = 200)]
    Ok(Json<T>),
    #[oai(status = 404)]
    CurrenciesNotFound(Json<CurrenciesNotFound>),
}

#[derive(Object)]
struct CurrenciesNotFound {
    currencies_not_found: Vec<Currency>,
}

impl<T: Send + Type + ToJSON> RatesResponse<T> {
    fn not_found(currencies: Vec<Currency>) -> Self {
        RatesResponse::CurrenciesNotFound(Json(CurrenciesNotFound {
            currencies_not_found: currencies,
        }))
    }
}

#[derive(Object)]
struct Rates {
    date: NaiveDate,
    rates: HashMap<Currency, f64>,
}

impl Api {
    fn no_rates() -> poem::Error {
        poem::Error::from_string("No rates available", StatusCode::NOT_FOUND)
    }
}

#[OpenApi]
impl Api {
    #[oai(path = "/", method = "get")]
    async fn index(&self, dataset: Data<&SharedDataset>) -> poem::Result<Json<IndexResponse>> {
        let dataset = dataset.read().await;

        match (dataset.rates.first(), dataset.rates.last()) {
            (Some((last_date, _)), Some((first_date, _))) => Ok(Json(IndexResponse {
                symbols: dataset.symbols.clone(),
                timeframe: [*first_date, *last_date],
            })),
            _ => Err(Api::no_rates()),
        }
    }

    #[oai(path = "/rates", method = "post")]
    async fn rates(
        &self,
        dataset: Data<&SharedDataset>,
        req: Json<Option<RatesRequest>>,
    ) -> poem::Result<RatesResponse<Rates>> {
        let dataset = dataset.read().await;

        let index = req
            .as_ref()
            .and_then(|r| r.date)
            .map(|d| {
                dataset
                    .rates
                    .binary_search_by_key(&d, |&(date, _)| date)
                    .unwrap_or_else(|e| e - 1)
            })
            .unwrap_or(dataset.rates.len() - 1);

        let (date, rates) = dataset.rates.get(index).ok_or_else(Api::no_rates)?;

        let from = req
            .as_ref()
            .and_then(|r| r.from.clone())
            .unwrap_or("EUR".to_string());

        let mut rates = match convert(from.clone(), rates.clone()) {
            Some(converted) => converted,
            None => return Ok(RatesResponse::not_found(vec![from])),
        };

        let to = req.as_ref().and_then(|r| r.to.clone());

        let rates = match to.as_deref() {
            Some([]) | None => rates,
            Some(to) => {
                let to = to.iter().map(|c| c.to_uppercase()).collect::<Vec<_>>();

                let not_found = to
                    .clone()
                    .drain(..)
                    .filter(|c| !dataset.symbols.contains(c))
                    .collect::<Vec<_>>();

                if !not_found.is_empty() {
                    return Ok(RatesResponse::not_found(not_found));
                }

                rates
                    .drain()
                    .filter(|(currency, _)| to.contains(&currency))
                    .collect::<HashMap<_, _>>()
            }
        };

        Ok(RatesResponse::Ok(Json(Rates { date: *date, rates })))
    }
}

fn convert(from: Currency, rates: HashMap<Currency, f64>) -> Option<HashMap<Currency, f64>> {
    if from == "EUR".to_string() {
        return Some(rates);
    }

    let rates_ = rates.clone();
    let from_rate = rates_.get(&from)?;
    let rates = rates
        .into_iter()
        .map(|(currency, rate)| (currency, rate / from_rate))
        .collect::<HashMap<_, _>>();

    Some(rates)
}

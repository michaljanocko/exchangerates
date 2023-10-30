use std::{collections::HashMap, str::FromStr};

use chrono::NaiveDate;
use poem::web::Data;
use poem_openapi::{
    payload::Json,
    types::{ToJSON, Type},
    ApiResponse, Object, OpenApi, Union,
};
use reqwest::StatusCode;

use crate::data::{self, Currency, Day, SharedDataset};

pub struct Api;

#[derive(Object)]
struct IndexResponse {
    currencies: Vec<String>,
    timeframe: [NaiveDate; 2],
}

#[derive(Object, Clone)]
struct ConversionParams {
    #[oai(validator(pattern = "^([A-Z]{3})$"))]
    from: Option<String>,
    #[oai(validator(pattern = "^([A-Z]{3})$"))]
    to: Option<Vec<String>>,
}

#[derive(Object)]
struct RatesRequest {
    date: Option<NaiveDate>,
    #[oai(flatten)]
    conversion: Option<ConversionParams>,
}

#[derive(ApiResponse)]
enum RatesResponse<T: Send + Type + ToJSON> {
    #[oai(status = 200)]
    Ok(Json<T>),
    #[oai(status = 404)]
    CurrenciesNotFound(Json<CurrenciesNotFound>),
}

impl<T> From<CurrenciesNotFound> for RatesResponse<T>
where
    T: Send + Type + ToJSON,
{
    fn from(value: CurrenciesNotFound) -> Self {
        RatesResponse::CurrenciesNotFound(Json(value))
    }
}

#[derive(Object)]
struct CurrenciesNotFound {
    #[oai(skip_serializing_if_is_empty)]
    currencies_not_found: Vec<String>,
}

#[derive(Object)]
struct DayRates {
    date: NaiveDate,
    rates: HashMap<String, Option<f64>>,
}

impl Api {
    fn no_rates() -> poem::Error {
        poem::Error::from_string("No rates available", StatusCode::INTERNAL_SERVER_ERROR)
    }
}

#[OpenApi]
impl Api {
    #[oai(path = "/", method = "get")]
    async fn index(&self, dataset: Data<&SharedDataset>) -> poem::Result<Json<IndexResponse>> {
        let dataset = dataset.read().await;

        match dataset.timeframe() {
            Some([first, last]) => Ok(Json(IndexResponse {
                currencies: dataset
                    .currencies
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
                timeframe: [first, last],
            })),
            None => Err(Api::no_rates()),
        }
    }

    #[oai(path = "/rates", method = "post")]
    async fn rates(
        &self,
        dataset: Data<&SharedDataset>,
        req: Json<Option<RatesRequest>>,
    ) -> poem::Result<RatesResponse<DayRates>> {
        let dataset = dataset.read().await;

        // Try to extract the date from the request
        let index = req
            .as_ref()
            .and_then(|r| r.date)
            .map(|d| {
                dataset
                    .days
                    .binary_search_by_key(&d, |day| day.date)
                    .unwrap_or_else(|e| e.checked_sub(1).unwrap_or(1))
            })
            .unwrap_or_else(|| dataset.days.len().checked_sub(1).unwrap_or(1));

        let mut day = dataset.days.get(index).ok_or_else(Api::no_rates)?.clone();

        let from = req
            .as_ref()
            .and_then(|r| r.conversion.clone())
            .and_then(|r| r.from);

        // Try to find the base currency
        let from = match from {
            Some(from) => match dataset.from(&from) {
                Some(from) => from,
                None => {
                    return Ok(CurrenciesNotFound {
                        currencies_not_found: vec![from],
                    }
                    .into())
                }
            },
            None => data::EUR,
        };

        day.rates = match (Conversion {
            from,
            currencies: dataset.currencies.clone(),
        })
        // It actually makes sense to clone the rates here because returning
        // the values from the API is going to consume them anyway
        .convert_day(day.rates.clone())
        {
            Some(converted) => converted,
            None => {
                return Ok(CurrenciesNotFound {
                    currencies_not_found: vec![from.to_string()],
                }
                .into())
            }
        };

        let to = req
            .as_ref()
            .and_then(|r| r.conversion.clone())
            .and_then(|r| r.to)
            .unwrap_or_default();

        let currencies_not_found = to
            .clone()
            .into_iter()
            .filter(|c| dataset.currencies.binary_search(&c.as_str()).is_err())
            .collect::<Vec<_>>();

        if currencies_not_found.is_empty() {
            let mut rates = day.to_hashmap(dataset.currencies.clone());

            if !to.is_empty() {
                rates = rates
                    .into_iter()
                    .filter(|(c, _)| to.contains(c))
                    .collect::<HashMap<_, _>>();
            }

            Ok(RatesResponse::Ok(Json(DayRates {
                date: day.date,
                rates,
            })))
        } else {
            return Ok(CurrenciesNotFound {
                currencies_not_found,
            }
            .into());
        }
    }
}

struct Conversion {
    pub from: &'static str,
    pub currencies: Vec<&'static str>,
}

impl Conversion {
    fn convert_day(&self, rates: Vec<Option<f64>>) -> Option<Vec<Option<f64>>> {
        if self.from == data::EUR {
            return Some(rates);
        }

        // Find the index of the base currency
        let from = self.currencies.binary_search(&self.from).ok()?;
        // Get the base currency rate
        let from_rate = rates.get(from)?.as_ref()?;
        // Convert all the rates
        let rates = rates
            .iter()
            .map(|rate| rate.map(|r| r / from_rate))
            .collect::<Vec<_>>();

        Some(rates)
    }

    fn conver_days(&self) {
        unimplemented!()
    }

    // fn convert(&self) -> Option<HashMap<String, f64>> {
    //     let rates = self.convert_day()?;

    //     Some(
    //         self.currencies
    //             .iter()
    //             .zip(rates)
    //             .filter_map(|(currency, rate)| rate.map(|r| (currency.to_string(), r)))
    //             .collect::<HashMap<_, _>>(),
    //     )
    // }
}

use std::collections::HashMap;

use chrono::NaiveDate;
use poem::web::Data;
use poem_openapi::{
    payload::Json,
    types::{ToJSON, Type},
    ApiResponse, Object, OpenApi,
};
use reqwest::StatusCode;

use crate::data::{self, Dataset, SharedDataset};

#[derive(Clone, Copy)]
pub struct Api;

#[derive(Object)]
struct IndexResponse {
    currencies: Vec<String>,
    timeframe: [NaiveDate; 2],
}

#[derive(Debug)]
struct Conversion {
    from: &'static str,
    to: Vec<String>,
}

impl Default for Conversion {
    fn default() -> Self {
        Self {
            from: data::EUR,
            to: Vec::new(),
        }
    }
}

#[derive(Object, Clone, Debug)]
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

impl Conversion {
    fn from_params(
        params: &ConversionParams,
        dataset: &Dataset,
    ) -> Result<Self, CurrenciesNotFound> {
        Ok(Self {
            from: match params.from.as_ref() {
                Some(from) => match dataset.from(&from) {
                    // If we have a matching currency, return it
                    Some(from) => from,
                    // If not, return an error
                    None => {
                        return Err(CurrenciesNotFound {
                            currencies_not_found: vec![from.clone()],
                        })
                    }
                },
                // If no currency has been provided, use EUR
                None => data::EUR,
            },
            // If no `conversion.to` was specified, return an empty `Vec` → all currencies
            to: {
                let (to, not_found): (Vec<_>, Vec<_>) = params
                    .to
                    .as_ref()
                    .unwrap_or(&Vec::new())
                    .clone()
                    .into_iter()
                    .partition(|c| dataset.currencies.binary_search(&c.as_str()).is_ok());

                if !not_found.is_empty() {
                    return Err(CurrenciesNotFound {
                        currencies_not_found: not_found,
                    });
                }

                to
            },
        })
    }
}

#[derive(Object)]
struct Rates {
    date: NaiveDate,
    rates: HashMap<String, Option<f64>>,
}

#[derive(Object)]
struct TimeframeRequest {
    timeframe: [Option<NaiveDate>; 2],
    #[oai(flatten)]
    conversion: Option<ConversionParams>,
}

#[derive(Object)]
struct Timeframe {
    timeframe: [NaiveDate; 2],
    rates: Vec<Rates>,
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

impl Api {
    fn no_rates() -> poem::Error {
        poem::Error::from_string("No rates available", StatusCode::INTERNAL_SERVER_ERROR)
    }
}

#[OpenApi]
impl Api {
    /// Returns the list of available currencies and the timeframe of the dataset
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

    /// Returns the exchange rates for the given date
    #[oai(path = "/rates", method = "post")]
    async fn rates(
        &self,
        dataset: Data<&SharedDataset>,
        req: Json<Option<RatesRequest>>,
    ) -> poem::Result<RatesResponse<Rates>> {
        let dataset = dataset.read().await;

        // Try to extract the date from the request
        let index = match req.as_ref().and_then(|r| r.date) {
            // Find the index of the day if provided
            Some(date) => dataset
                .days
                .binary_search_by_key(&date, |day| day.date)
                // We are using `.checked_sub` because if the dataset
                // is empty, we would run into an underflow
                .unwrap_or_else(|e| e.checked_sub(1).unwrap_or_default()),

            // Otherwise, use the latest day
            None => dataset.days.len().checked_sub(1).unwrap_or_default(),
        };

        let conversion = match req
            .as_ref()
            .and_then(|r| r.conversion.as_ref())
            .map(|c| Conversion::from_params(&c, &dataset))
        {
            // Supplied → use it
            Some(Ok(conversion)) => conversion,
            // Error → return it
            Some(Err(e)) => return Ok(e.into()),
            // None → use default
            None => Conversion::default(),
        };

        let day = dataset.days.get(index).ok_or_else(Api::no_rates)?.clone();

        // It actually makes sense to clone the rates here because returning
        // the values from the API is going to consume them anyway
        let day = match day.convert(conversion.from, dataset.currencies) {
            Some(converted) => converted,
            None => {
                // We have validated this before but the base currency might
                // not be available for the requested date
                return Ok(CurrenciesNotFound {
                    currencies_not_found: vec![conversion.from.to_string()],
                }
                .into());
            }
        };

        let mut rates = day.to_hashmap(dataset.currencies);

        if !conversion.to.is_empty() {
            rates = rates
                .into_iter()
                .filter(|(c, _)| conversion.to.contains(c))
                .collect::<HashMap<_, _>>();
        }

        Ok(RatesResponse::Ok(Json(Rates {
            date: day.date,
            rates,
        })))
    }

    #[oai(path = "/rates", method = "get")]
    async fn rates_(&self, dataset: Data<&SharedDataset>) -> poem::Result<RatesResponse<Rates>> {
        self.rates(dataset, Json(None)).await
    }

    #[oai(path = "/rates/timeframe", method = "post")]
    async fn timeframe(
        &self,
        dataset: Data<&SharedDataset>,
        req: Json<TimeframeRequest>,
    ) -> poem::Result<RatesResponse<Timeframe>> {
        let dataset = dataset.read().await;

        let (start, end) = (
            req.timeframe[0]
                .map(|start| {
                    dataset
                        .days
                        .binary_search_by_key(&start, |day| day.date)
                        // If not found, take the previous day (or the first day)
                        .unwrap_or_else(|e| e.checked_sub(1).unwrap_or_default())
                })
                // Otherwise, take the very first day
                .unwrap_or(0),
            req.timeframe[1]
                .map(|end| {
                    dataset
                        .days
                        .binary_search_by_key(&end, |day| day.date)
                        // If not found, take the next day
                        .unwrap_or_else(|e| e + 1)
                })
                // Otherwise, take the latest
                .unwrap_or(dataset.days.len() - 1),
        );

        let days = dataset.days.get(start..end).ok_or_else(Api::no_rates)?;

        let conversion = match req
            .conversion
            .as_ref()
            .map(|c| Conversion::from_params(&c, &dataset))
        {
            // Supplied → use it
            Some(Ok(conversion)) => conversion,
            // Error → return it
            Some(Err(e)) => return Ok(e.into()),
            // None → use default
            None => Conversion::default(),
        };

        let rates = days
            .into_iter()
            .filter_map(|day| {
                day.clone()
                    .convert(conversion.from, dataset.currencies)
                    .map(|rates| {
                        let mut rates = rates.to_hashmap(dataset.currencies);

                        if !conversion.to.is_empty() {
                            rates = rates
                                .into_iter()
                                .filter(|(c, _)| conversion.to.contains(c))
                                .collect::<HashMap<_, _>>();
                        };

                        Rates {
                            date: day.date,
                            rates,
                        }
                    })
            })
            .collect::<Vec<_>>();

        Ok(RatesResponse::Ok(Json(Timeframe {
            timeframe: [
                rates.first().map(|d| d.date).unwrap(),
                rates.last().map(|d| d.date).unwrap(),
            ],
            rates,
        })))
    }
}

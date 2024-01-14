use std::net::{Ipv4Addr, SocketAddrV4};

use poem::{listener::TcpListener, EndpointExt, Route};
use poem_openapi::OpenApiService;
use tokio::signal::ctrl_c;

mod api;
mod data;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    pretty_env_logger::init();

    // Download dataset or use a cached one
    let dataset = data::dataset().await?;

    // Schedule dataset updates
    tokio::spawn(data::schedule_dataset_update(dataset.clone()));

    let service =
        OpenApiService::new(api::Api, "Exchange rates API", "1.0").server("https://exchange.rates");

    let app = Route::new()
        .at("/openapi.json", service.clone().spec_endpoint())
        .nest("/", service.data(dataset));

    let socket_addr = SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 8000);
    poem::Server::new(TcpListener::bind(socket_addr))
        .run_with_graceful_shutdown(app, async move { ctrl_c().await.unwrap_or(()) }, None)
        .await?;

    Ok(())
}

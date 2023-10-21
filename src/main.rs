use std::{
    net::{Ipv4Addr, SocketAddrV4},
    sync::Arc,
};

use poem::{listener::TcpListener, EndpointExt, Route};
use poem_openapi::OpenApiService;
use tokio::sync::RwLock;

mod api;
mod data;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dataset = Arc::new(RwLock::new(data::dataset().await?));

    let service =
        OpenApiService::new(api::Api, "Exchange rate API", "1.0").server("https://exchange.rates");

    let app = Route::new().nest("/api", service.data(dataset));

    let socket_addr = SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 8000);

    poem::Server::new(TcpListener::bind(socket_addr))
        .run(app)
        .await?;

    Ok(())
}

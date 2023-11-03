FROM rust:1 AS builder

RUN apt-get update
RUN apt-get install -y musl-tools
RUN rustup target add x86_64-unknown-linux-musl

WORKDIR /usr/src/exchangerates

# We need this stupid thing to cache dependency builds
RUN mkdir src
RUN echo "fn main() {}" > src/main.rs

COPY Cargo.toml Cargo.lock ./
RUN cargo build --release --target x86_64-unknown-linux-musl

COPY . .
RUN cargo install --path . --target x86_64-unknown-linux-musl

FROM alpine:latest
COPY --from=builder /usr/local/cargo/bin/exchangerates /usr/local/bin/exchangerates

ENV RUST_LOG=info

VOLUME [ "/data" ]

EXPOSE 8000

CMD [ "exchangerates" ]

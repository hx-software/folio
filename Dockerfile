FROM rust:bookworm AS build

RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY rust-toolchain.toml ./
RUN rustup show active-toolchain \
    && cargo install dioxus-cli --version 0.7.10 --locked

COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

RUN dx bundle --web --release --locked --out-dir /site \
    && cp crates/pdf/assets/FONT-LICENSE.txt /site/public/

FROM nginx:stable-alpine

COPY deploy/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /site/public/ /usr/share/nginx/html/

EXPOSE 80

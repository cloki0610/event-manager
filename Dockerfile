# syntax=docker/dockerfile:1.7
FROM rust:1.90-slim-bookworm AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev g++ \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src src
COPY migrations migrations
# Empty for SQLite only; `--build-arg FEATURES=duckdb` adds DuckDB (slow C++ build).
ARG FEATURES=""
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked --features "$FEATURES" \
    && cp target/release/event-manager /usr/local/bin/event-manager

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --no-create-home app \
    && mkdir /data && chown app /data
COPY --from=build /usr/local/bin/event-manager /usr/local/bin/event-manager
USER app
ENV BIND_ADDR=0.0.0.0:8080 \
    DATABASE_URL=sqlite:///data/events.db \
    RUST_LOG=info
EXPOSE 8080
VOLUME /data
CMD ["event-manager"]

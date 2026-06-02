# syntax=docker/dockerfile:1
# go-on Simple Server — Docker
#   Build:   docker build -f deploy/simple-server/Dockerfile -t go-on:simple .
#   Run:     docker compose -f deploy/simple-server/docker-compose.yml up

# ---- Builder ----
FROM rust:1.83-slim-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libsqlite3-dev && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY config/ config/
COPY languages/ languages/
COPY prompts/ prompts/
COPY src/ src/

RUN cargo build --release --no-default-features -F profile-simple-server && \
    cp target/release/go-on /go-on

# ---- Runtime ----
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libsqlite3-0 curl && \
    rm -rf /var/lib/apt/lists/*

RUN groupadd -r go-on && useradd -r -g go-on -d /opt/go-on -s /sbin/nologin go-on

COPY --from=builder --chown=go-on:go-on /go-on /usr/local/bin/go-on

USER go-on
WORKDIR /opt/go-on

EXPOSE 8090

HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8090/health || exit 1

ENTRYPOINT ["go-on"]
CMD ["-c", "/opt/go-on/config.toml", "--mode", "acp_http", "--bind", "0.0.0.0:8090"]

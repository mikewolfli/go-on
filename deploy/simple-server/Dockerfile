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
COPY src/core/providers_data.toml src/core/
COPY src/ src/

RUN cargo build --release --no-default-features -F profile-simple-server && \
    cp target/release/go-on /go-on

# ---- Runtime ----
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libsqlite3-0 && \
    rm -rf /var/lib/apt/lists/*

RUN groupadd -r goon && useradd -r -g goon -d /opt/go-on -s /sbin/nologin goon

COPY --from=builder --chown=goon:goon /go-on /usr/local/bin/go-on

USER goon
WORKDIR /opt/go-on

EXPOSE 8090

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD go-on -c /opt/go-on/config.toml --status || exit 1

ENTRYPOINT ["go-on"]
CMD ["-c", "/opt/go-on/config.toml", "--mode", "acp_http", "--bind", "0.0.0.0:8090"]

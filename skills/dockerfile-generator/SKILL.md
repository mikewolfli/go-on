---
name: dockerfile-generator
description: Generates optimized Dockerfiles and Docker Compose configurations for any language or framework
version: 1.0.0
author: go-on-team
tags: [docker, container, dockerfile, docker-compose, devops, deployment]
min_go_on_version: 1.0.0
---

# Dockerfile Generator Skill

Generates production-ready Dockerfiles and Docker Compose configurations following best practices for layer caching, multi-stage builds, security (non-root user, minimal base images), and architecture-specific optimization.

## How It Works

1. **Analyze** — Determines the build strategy based on language, framework, and dependency manager
2. **Multi-Stage** — Separates build and runtime stages to minimize final image size
3. **Cache** — Orders Dockerfile instructions for maximum layer cache reuse (dependencies before source code)
4. **Secure** — Applies security best practices: non-root user, no shell in production, minimal base, HEALTHCHECK, proper signal handling
5. **Compose** — Generates optional `docker-compose.yml` with linked services (database, cache, queue, reverse proxy) and environment variable documentation

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `language` | string | Language: `rust`, `python`, `typescript`, `go`, `java`, `node` |
| `framework` | string | Optional: web framework (e.g., `actix-web`, `fastapi`, `express`) |
| `port` | integer | Application port (default: 8080) |
| `include_compose` | boolean | Optional: also generate docker-compose.yml (default: false) |
| `compose_services` | string[] | Optional: additional services for compose (`postgres`, `redis`, `nginx`, `minio`) |
| `target_arch` | string | Optional: target architecture (`amd64`, `arm64`, default: platform-native) |

## Example

```json
{
  "language": "rust",
  "framework": "actix-web",
  "port": 8080,
  "include_compose": true,
  "compose_services": ["postgres", "redis"],
  "target_arch": "amd64"
}
```

**Example output (abbreviated):**

```dockerfile
# Stage 1: Build
FROM rust:1.85-slim-bookworm AS builder
WORKDIR /app
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release 2>/dev/null || true
RUN rm -rf src

# Build application
COPY . .
RUN touch src/main.rs
RUN cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*
RUN groupadd -r app && useradd -r -g app app
WORKDIR /app
COPY --from=builder /app/target/release/my-app .
USER app
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
  CMD curl -f http://localhost:8080/health || exit 1
CMD ["./my-app"]
```

```yaml
# docker-compose.yml
version: "3.9"
services:
  app:
    build: .
    ports: ["8080:8080"]
    environment:
      DATABASE_URL: postgres://user:pass@postgres:5432/db
      REDIS_URL: redis://redis:6379
    depends_on: [postgres, redis]
  postgres:
    image: postgres:16-alpine
    environment:
      POSTGRES_DB: myapp
      POSTGRES_USER: user
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD}
    volumes: ["pgdata:/var/lib/postgresql/data"]
  redis:
    image: redis:7-alpine
volumes: { pgdata: }
```

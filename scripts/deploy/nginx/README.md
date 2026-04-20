# Nginx TLS Ingress For go-on

This directory contains a production ingress template for Stage C release readiness.

## What it enforces

1. HTTPS-only ingress with automatic HTTP->HTTPS redirect
2. TLS 1.2/1.3 termination
3. Entry rate-limit at ingress level (240 rpm, burst 60)
4. Basic concurrency cap per source IP
5. Forwarding trace/request headers to backend

## Quick start

1. Copy `deploy/nginx/go-on.conf` to your Nginx sites path.
2. Set `server_name` and backend target if not `127.0.0.1:8090`.
3. Install cert and key:
   - `/etc/nginx/certs/go-on.crt`
   - `/etc/nginx/certs/go-on.key`
4. Verify and reload:
   - `nginx -t`
   - `systemctl reload nginx`

## Backend pairing

Use `config.production.toml` so backend keeps `entry_auth_enabled=true` and `production_strict=true`.

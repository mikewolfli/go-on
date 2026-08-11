# go-on Kubernetes Deployment

This directory contains Kubernetes manifests for deploying go-on as a
production-grade multi-agent orchestration service.

## Quick Start

```bash
# 1. Create the local secrets file (gitignored — never commit real secrets).
#    Copy the committed template and fill in real values:
cp deploy/k8s/.secrets.env deploy/k8s/.secrets.local.env
#    then edit deploy/k8s/.secrets.local.env and set:
#      GO_ON_ENTRY_API_KEY     (entry auth, default env name)
#      GO_ON_DEEPSEEK_API_KEY  (deepseek agent `api_key_env`)
#    The KEY NAMES become the environment variable names injected via `envFrom`
#    in deployment.yaml; `kustomization.yaml` turns this file into the
#    `go-on-secrets` Secret via its secretGenerator. For a PostgreSQL backend
#    (multi-users-server parity) also set GO_ON_PG_CONNECTION_STRING.

# 2. Deploy (the directory is a Kustomization, so use -k, not -f)
kubectl apply -k deploy/k8s/

# 3. Check status
kubectl get pods -l app=go-on
kubectl logs -l app=go-on
```

## Architecture

```
          ┌──────────┐
          │ Ingress  │  (TLS termination)
          └────┬─────┘
               │
          ┌────▼─────┐
          │ Service  │  (ClusterIP, port 8090)
          └────┬─────┘
               │
          ┌────▼──────────┐
          │ Deployment    │  (3 replicas recommended)
          │ go-on server  │
          └───────────────┘
```

## Components

- `deployment.yaml` – Main Deployment (configurable replicas, resource limits, probes)
- `service.yaml` – ClusterIP Service for internal routing
- `ingress.yaml` – Ingress with TLS termination via cert-manager (nginx ingress class)
- `configmap.yaml` – go-on configuration (non-sensitive settings)
- `network-policy.yaml` – NetworkPolicy restricting inbound traffic to the go-on
  pods and outbound to the internet (no private RFC1918 egress)
- `pod-disruption-budget.yaml` – PodDisruptionBudget (maxUnavailable: 1) for HA
  during voluntary disruptions
- Secret – generated from `deploy/k8s/.secrets.local.env` (gitignored; copy
  from the `.secrets.env` template) by the `secretGenerator` in
  `kustomization.yaml`; no `secret.yaml` manifest is shipped
- `kustomization.yaml` – Kustomize overlay that includes all resources above

## Ingress Configuration

The `ingress.yaml` manifest configures external TLS access through an nginx ingress
controller. It is included in `kustomization.yaml` and will be applied automatically
when running `kubectl apply -k deploy/k8s/`.

### Customizing the Ingress

1. Update `spec.rules[0].host` from `go-on.example.com` to your actual domain.
2. Ensure cert-manager is installed in your cluster for automatic TLS certificate
   provisioning. If you use a different TLS solution, remove or modify the
   `cert-manager.io/cluster-issuer` annotation.
3. If your cluster does not use the `nginx` ingress class, change
   `spec.ingressClassName` accordingly.

## Production Checklist

- [ ] Replace `your-domain.com` in ingress annotations
- [ ] Set resource limits based on expected workload
- [ ] Configure HPA for auto-scaling (`kubectl autoscale deployment go-on --cpu-percent=80 --min=3 --max=10`)
- [ ] Use external secrets operator (e.g. External Secrets Operator, Sealed Secrets)
- [ ] Enable PodDisruptionBudget for HA
- [ ] Set up monitoring: Prometheus + Grafana dashboards

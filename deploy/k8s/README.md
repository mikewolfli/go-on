# go-on Kubernetes Deployment

This directory contains Kubernetes manifests for deploying go-on as a
production-grade multi-agent orchestration service.

## Quick Start

```bash
# 1. Create a secret with your API keys
kubectl create secret generic go-on-secrets \
  --from-literal=deepseek-api-key=sk-xxxxx \
  --from-literal=server-api-key=change-me

# 2. Deploy
kubectl apply -f deploy/k8s/

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
- `configmap.yaml` – go-on configuration (non-sensitive settings)
- `secret.yaml` – Template for sensitive credentials (API keys)

## Production Checklist

- [ ] Replace `your-domain.com` in ingress annotations
- [ ] Set resource limits based on expected workload
- [ ] Configure HPA for auto-scaling (`kubectl autoscale deployment go-on --cpu-percent=80 --min=3 --max=10`)
- [ ] Use external secrets operator (e.g. External Secrets Operator, Sealed Secrets)
- [ ] Enable PodDisruptionBudget for HA
- [ ] Set up monitoring: Prometheus + Grafana dashboards

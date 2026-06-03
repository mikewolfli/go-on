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
- `ingress.yaml` – Ingress with TLS termination via cert-manager (nginx ingress class)
- `configmap.yaml` – go-on configuration (non-sensitive settings)
- `secret.yaml` – Template for sensitive credentials (API keys)
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

# Kubernetes Deployment (S14)

This directory contains baseline manifests for deploying go-on.

Files:
- deployment.yaml
- service.yaml
- configmap.yaml

Usage:
```bash
kubectl apply -f deploy/k8s/configmap.yaml
kubectl apply -f deploy/k8s/deployment.yaml
kubectl apply -f deploy/k8s/service.yaml
```

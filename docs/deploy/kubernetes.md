# Kubernetes deploy

The same Docker image that runs on a Pi runs on a k8s fleet. Manifests
below are minimal — pair with cert-manager, an Ingress controller, and
external Postgres for production.

> A polished Helm chart is on the Phase 5 roadmap. The plain manifests
> below are stable and represent the contract the chart will follow.

## Image

`server/Dockerfile` builds a single-binary image with the bundle
migrations baked in.

```bash
docker build -t skill-pool-server:0.1.0 -f server/Dockerfile .
docker push myregistry.example.com/skill-pool-server:0.1.0
```

## Secrets

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: skill-pool-env
  namespace: skill-pool
type: Opaque
stringData:
  SKILL_POOL_DATABASE_URL: postgres://skillpool:CHANGEME@postgres-rw.skill-pool.svc.cluster.local/skillpool
  # Optional: OIDC / SAML / SMTP secrets go here.
```

Use external-secrets / sealed-secrets / sops in production — never
commit credentials verbatim.

## Deployment

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: skill-pool-server
  namespace: skill-pool
spec:
  replicas: 2
  selector: { matchLabels: { app: skill-pool-server } }
  template:
    metadata:
      labels: { app: skill-pool-server }
    spec:
      automountServiceAccountToken: false
      containers:
        - name: server
          image: myregistry.example.com/skill-pool-server:0.1.0
          args: ["serve"]
          ports:
            - containerPort: 8080
              name: http
          env:
            - name: SKILL_POOL_BIND
              value: "0.0.0.0:8080"
            - name: SKILL_POOL_STORAGE_URI
              value: "s3://skill-pool-bundles?region=us-east-1"
            - name: RUST_LOG
              value: "info,skill_pool=info"
            - name: RUST_LOG_FORMAT
              value: "json"
          envFrom:
            - secretRef: { name: skill-pool-env }
          readinessProbe:
            httpGet: { path: /v1/healthz, port: http }
            periodSeconds: 5
            failureThreshold: 3
          livenessProbe:
            httpGet: { path: /v1/healthz, port: http }
            periodSeconds: 30
            failureThreshold: 5
          resources:
            requests: { cpu: "100m", memory: "128Mi" }
            limits:   { cpu: "1",    memory: "1Gi"   }
          securityContext:
            runAsNonRoot: true
            runAsUser: 65532
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            capabilities: { drop: ["ALL"] }
---
apiVersion: v1
kind: Service
metadata:
  name: skill-pool-server
  namespace: skill-pool
spec:
  selector: { app: skill-pool-server }
  ports:
    - { port: 80, targetPort: http, name: http }
```

## Ingress

Use whatever ingress class fits — examples assume `nginx-ingress` with
cert-manager `ClusterIssuer` named `letsencrypt-prod`.

```yaml
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: skill-pool
  namespace: skill-pool
  annotations:
    cert-manager.io/cluster-issuer: letsencrypt-prod
    nginx.ingress.kubernetes.io/proxy-body-size: "50m"
spec:
  ingressClassName: nginx
  tls:
    - hosts: ["skill-pool.example.com", "*.skill-pool.example.com"]
      secretName: skill-pool-tls
  rules:
    - host: skill-pool.example.com
      http:
        paths:
          - path: /v1
            pathType: Prefix
            backend: { service: { name: skill-pool-server, port: { number: 80 } } }
          - path: /metrics
            pathType: Exact
            backend: { service: { name: skill-pool-server, port: { number: 80 } } }
          - path: /
            pathType: Prefix
            backend: { service: { name: skill-pool-web, port: { number: 80 } } }
```

The wildcard host can be split into per-tenant Ingress objects if you
need per-tenant `tls` certs or different annotations.

## Autoscaling

```yaml
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: skill-pool-server
  namespace: skill-pool
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: skill-pool-server
  minReplicas: 2
  maxReplicas: 20
  metrics:
    - type: Resource
      resource:
        name: cpu
        target: { type: Utilization, averageUtilization: 70 }
```

Once `/metrics` is scraped by Prometheus + prometheus-adapter is in place,
add a `Pods` metric on `rate(http_requests_total[1m])` for traffic-based
scaling.

## Postgres + storage

Use whatever you already trust:

- **Postgres**: managed (RDS / Cloud SQL / Crunchy) or
  zalando-postgres-operator / CloudNativePG in-cluster.
- **Bundles**: S3 / GCS / Azure Blob via `SKILL_POOL_STORAGE_URI`. The
  server uses opendal — no cloud-specific SDK on the path.

## Observability

- Scrape `/metrics` (see `ops/grafana/skill-pool.json` and
  `ops/alerts/skill-pool.rules.yaml`).
- Build with `--features otlp` and set `OTEL_EXPORTER_OTLP_ENDPOINT` for
  distributed tracing.
- Logs are line-delimited JSON to stdout — Loki / CloudWatch / Splunk
  ingest unchanged.

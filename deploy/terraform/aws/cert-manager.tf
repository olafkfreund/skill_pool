# cert-manager — issues Let's Encrypt certs for the ALB.
#
# Pair this with `values-aws.yaml`'s `cert-manager.io/cluster-issuer:
# letsencrypt-prod` annotation on the Ingress. cert-manager watches the
# Ingress, kicks off an HTTP-01 challenge via the ALB (port 80, which we
# leave open), proves domain ownership to Let's Encrypt, and writes the
# resulting cert into the Secret named in the Ingress's `tls[].secretName`.
#
# Why cert-manager + LE instead of ACM?
#   * Dev / staging deployments use a `*.nip.io` hostname tied to the
#     ALB's public IP. ACM can't validate nip.io (you don't own the zone)
#     but LE can — it just resolves the DNS and hits the challenge URL.
#   * Production deployments with a real domain can either keep this
#     setup OR switch to ACM by setting `var.use_acm_cert = true` and
#     wiring the ARN into the chart. Both paths work; pick one per env.
#
# Trade-offs vs ACM:
#   * Cert renewal happens in-cluster (cert-manager does it automatically
#     before 30d-from-expiry). ACM is fully managed.
#   * Let's Encrypt has rate limits (50 certs / registered-domain / week,
#     5 duplicate certs / week). For dev that means one cert per tenant
#     subdomain, plus the apex — a 50-tenant dev cluster could hit this.
#     Switch to wildcard via DNS-01 if that happens (the doc shows how).

resource "helm_release" "cert_manager" {
  count = var.cert_manager_enabled ? 1 : 0

  name             = "cert-manager"
  repository       = "https://charts.jetstack.io"
  chart            = "cert-manager"
  version          = var.cert_manager_chart_version
  namespace        = "cert-manager"
  create_namespace = true

  # The CRDs ride along with the chart; alternative is to install them
  # separately and set this to false. Keeping it true is simpler.
  set {
    name  = "crds.enabled"
    value = "true"
  }

  # Resource limits — cert-manager is tiny but defaults to no limits.
  set {
    name  = "resources.requests.cpu"
    value = "20m"
  }
  set {
    name  = "resources.requests.memory"
    value = "64Mi"
  }
  set {
    name  = "resources.limits.memory"
    value = "256Mi"
  }

  # The webhook needs the ALB controller to be live before it can serve
  # admission requests; ordering matters.
  depends_on = [
    helm_release.alb_controller,
  ]
}

# ClusterIssuer for Let's Encrypt production. HTTP-01 solver via the
# AWS Load Balancer Controller's Ingress class — cert-manager creates
# a transient Ingress for each challenge, the ALB picks it up, LE hits
# the well-known URL, gets the response, issues the cert.
resource "kubernetes_manifest" "letsencrypt_prod" {
  count = var.cert_manager_enabled ? 1 : 0

  manifest = {
    apiVersion = "cert-manager.io/v1"
    kind       = "ClusterIssuer"
    metadata = {
      name = "letsencrypt-prod"
    }
    spec = {
      acme = {
        email  = var.letsencrypt_email
        server = "https://acme-v02.api.letsencrypt.org/directory"
        privateKeySecretRef = {
          name = "letsencrypt-prod-account"
        }
        solvers = [
          {
            http01 = {
              ingress = {
                ingressClassName = "alb"
              }
            }
          }
        ]
      }
    }
  }

  depends_on = [helm_release.cert_manager]
}

# Staging ClusterIssuer — same shape, points at LE's staging endpoint
# which has much higher rate limits but issues untrusted certs.
# Use this while iterating on the deploy; switch to letsencrypt-prod
# once it's working.
resource "kubernetes_manifest" "letsencrypt_staging" {
  count = var.cert_manager_enabled ? 1 : 0

  manifest = {
    apiVersion = "cert-manager.io/v1"
    kind       = "ClusterIssuer"
    metadata = {
      name = "letsencrypt-staging"
    }
    spec = {
      acme = {
        email  = var.letsencrypt_email
        server = "https://acme-staging-v02.api.letsencrypt.org/directory"
        privateKeySecretRef = {
          name = "letsencrypt-staging-account"
        }
        solvers = [
          {
            http01 = {
              ingress = {
                ingressClassName = "alb"
              }
            }
          }
        ]
      }
    }
  }

  depends_on = [helm_release.cert_manager]
}

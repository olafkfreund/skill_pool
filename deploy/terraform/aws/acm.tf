# ACM cert covering the apex + wildcard for tenant subdomains.
# DNS-validated against the existing hosted zone (see route53.tf).
#
# The cert is paired with the ALB via the Ingress annotation
# `alb.ingress.kubernetes.io/certificate-arn` (see deploy/helm/skill-pool/values-aws.yaml).

resource "aws_acm_certificate" "service" {
  domain_name = var.service_hostnames[0]
  # Everything beyond the first hostname becomes a SAN. Wildcard works here too.
  subject_alternative_names = slice(var.service_hostnames, 1, length(var.service_hostnames))
  validation_method         = "DNS"

  lifecycle {
    create_before_destroy = true
  }
}

resource "aws_acm_certificate_validation" "service" {
  certificate_arn         = aws_acm_certificate.service.arn
  validation_record_fqdns = [for r in aws_route53_record.acm_validation : r.fqdn]
}

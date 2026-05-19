# DNS. We REFERENCE an existing public hosted zone; we do not create it.
# Creating zones from Terraform is dangerous — the operator usually wants
# the zone managed by-hand or by a separate "platform" Terraform stack
# that won't get destroyed on a `terraform destroy` of this module.
#
# After `terraform apply`, point the Route53 record at the ALB hostname.
# The Helm chart's Ingress object creates the ALB; you read the
# `ADDRESS` field off the Ingress and create an A-ALIAS record.
# Documented in docs/deploy/aws.md §8.

data "aws_route53_zone" "service" {
  name         = var.route53_zone_name
  private_zone = false
}

# DNS validation records for ACM. The ACM cert in acm.tf uses these to
# prove ownership of every domain on the cert.
resource "aws_route53_record" "acm_validation" {
  for_each = {
    for dvo in aws_acm_certificate.service.domain_validation_options : dvo.domain_name => {
      name   = dvo.resource_record_name
      record = dvo.resource_record_value
      type   = dvo.resource_record_type
    }
  }

  allow_overwrite = true
  name            = each.value.name
  records         = [each.value.record]
  ttl             = 60
  type            = each.value.type
  zone_id         = data.aws_route53_zone.service.zone_id
}

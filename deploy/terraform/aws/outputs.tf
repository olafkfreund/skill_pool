# Outputs the operator pastes into Helm values, GitHub secrets, and the
# deploy doc. Sensitive bits (DB password ARN) only — the password itself
# stays in Secrets Manager.

output "region" {
  description = "AWS region — paste into SKILL_POOL_STORAGE_URI's `?region=` query."
  value       = var.region
}

# --- EKS -----------------------------------------------------------------

output "cluster_name" {
  description = "EKS cluster name — `aws eks update-kubeconfig --name <this>`."
  value       = module.eks.cluster_name
}

output "cluster_endpoint" {
  description = "EKS API endpoint."
  value       = module.eks.cluster_endpoint
}

output "cluster_oidc_issuer_url" {
  description = "OIDC issuer URL for IRSA trust policies. Already wired into the IRSA role; surfaced here for ad-hoc service accounts."
  value       = module.eks.cluster_oidc_issuer_url
}

# --- IAM -----------------------------------------------------------------

output "irsa_role_arn" {
  description = "Paste into helm values: serviceAccount.annotations.eks.amazonaws.com/role-arn"
  value       = aws_iam_role.skill_pool_app.arn
}

output "github_actions_role_arn" {
  description = "Paste into the GitHub repo secret AWS_ROLE_ARN. Used by deploy/build workflows."
  value       = aws_iam_role.github_actions.arn
}

# --- Storage / DB --------------------------------------------------------

output "bundle_bucket_name" {
  description = "S3 bucket holding tenant bundles. Use in SKILL_POOL_STORAGE_URI=s3://<this>?region=<region>."
  value       = aws_s3_bucket.bundles.id
}

output "bundle_storage_uri" {
  description = "Drop-in for SKILL_POOL_STORAGE_URI."
  value       = "s3://${aws_s3_bucket.bundles.id}?region=${var.region}"
}

output "rds_endpoint" {
  description = "RDS writer endpoint. Combine with the Secrets Manager secret for the full DSN."
  value       = module.rds.db_instance_endpoint
}

output "rds_password_secret_arn" {
  description = "Secrets Manager ARN holding the RDS master password. Pull from here; do not hard-code."
  value       = aws_secretsmanager_secret.rds_password.arn
  sensitive   = true
}

output "database_url_template" {
  description = "DSN template with `__PASSWORD__` placeholder — substitute from Secrets Manager at deploy time."
  value       = "postgres://${module.rds.db_instance_username}:__PASSWORD__@${module.rds.db_instance_endpoint}/${module.rds.db_instance_name}?sslmode=require"
  sensitive   = true
}

# --- ElastiCache (Redis) -------------------------------------------------

output "redis_endpoint" {
  description = "ElastiCache primary endpoint (DNS name). Empty when `elasticache_enabled = false`."
  value       = var.elasticache_enabled ? aws_elasticache_replication_group.redis[0].primary_endpoint_address : ""
}

output "redis_port" {
  description = "ElastiCache port (always 6379 in this config)."
  value       = var.elasticache_enabled ? 6379 : 0
}

output "redis_auth_secret_arn" {
  description = "Secrets Manager ARN holding the Redis AUTH token + drop-in `url` field (rediss://). Pull from here; do not hard-code."
  value       = var.elasticache_enabled ? aws_secretsmanager_secret.redis_auth[0].arn : ""
  sensitive   = true
}

# --- ECR -----------------------------------------------------------------

output "ecr_server_repo_url" {
  description = "ECR repo for skill-pool-server. Paste into helm values: image.server.repository."
  value       = aws_ecr_repository.server.repository_url
}

output "ecr_web_repo_url" {
  description = "ECR repo for skill-pool-web. Paste into helm values: image.web.repository."
  value       = aws_ecr_repository.web.repository_url
}

# --- ACM / DNS -----------------------------------------------------------

output "acm_certificate_arn" {
  description = "ACM cert for the service hostnames. Paste into helm values: ingress.annotations.alb.ingress.kubernetes.io/certificate-arn."
  value       = aws_acm_certificate.service.arn
}

output "route53_zone_id" {
  description = "Hosted zone ID — useful when the operator scripts the ALB → DNS hookup."
  value       = data.aws_route53_zone.service.zone_id
}

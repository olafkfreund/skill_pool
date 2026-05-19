# ElastiCache Redis (single-node, t4g.micro for dev/test).
#
# Cluster mode disabled — single primary, no shards. This is the lean
# baseline (~$11/mo for cache.t4g.micro) used by the read-through caches
# in `server/src/cache.rs` (theme resolution + per-request auth lookup)
# and by the rate-limiter shipped in parallel.
#
# Encryption at rest + in transit (TLS) is ON; AUTH token lives in
# Secrets Manager (same pattern as RDS — see `rds.tf`).
#
# Upgrade path for production HA: enable cluster mode with N shards +
# automatic-failover replicas. The repl-group resource below already
# uses `aws_elasticache_replication_group` (not the legacy `cluster`
# resource), so the migration is a flip of `num_cache_clusters` +
# `automatic_failover_enabled` rather than a re-create.
#
# Controlled by `var.elasticache_enabled` (default true). Set to false
# to skip the Redis stack entirely (useful for the cheapest dev
# environments — the server falls back gracefully when
# `SKILL_POOL_REDIS_URL` is unset).

resource "random_password" "redis_auth" {
  count = var.elasticache_enabled ? 1 : 0

  length  = 32
  special = false # AUTH token: alphanumeric only — Redis CLI quoting is dodgy otherwise.
}

resource "aws_secretsmanager_secret" "redis_auth" {
  count = var.elasticache_enabled ? 1 : 0

  name                    = "${var.name_prefix}-${var.env}/redis-auth"
  description             = "ElastiCache Redis AUTH token + drop-in URL for skill-pool. Pulled by k8s out-of-band (see docs/deploy/aws.md §5)."
  recovery_window_in_days = 7
}

resource "aws_secretsmanager_secret_version" "redis_auth" {
  count = var.elasticache_enabled ? 1 : 0

  secret_id = aws_secretsmanager_secret.redis_auth[0].id
  secret_string = jsonencode({
    auth_token = random_password.redis_auth[0].result
    host       = aws_elasticache_replication_group.redis[0].primary_endpoint_address
    port       = 6379
    # Drop-in URL. `rediss://` (double-s) selects TLS; the server's
    # `redis` crate dep enables `tls-rustls` so this works out of the
    # box. AUTH is sent as the password component of the URL.
    url = "rediss://:${random_password.redis_auth[0].result}@${aws_elasticache_replication_group.redis[0].primary_endpoint_address}:6379"
  })
}

# Subnet group over the EKS private subnets — Redis is never reachable
# from the public internet. The VPC layout is the same as RDS.
resource "aws_elasticache_subnet_group" "redis" {
  count = var.elasticache_enabled ? 1 : 0

  name        = "${var.name_prefix}-${var.env}-redis"
  description = "skill-pool Redis subnet group (private subnets only)."
  subnet_ids  = module.vpc.private_subnets
}

# Security group: only the EKS node SG may reach Redis on 6379.
resource "aws_security_group" "redis" {
  count = var.elasticache_enabled ? 1 : 0

  name        = "${var.name_prefix}-${var.env}-redis"
  description = "ElastiCache Redis — accessible only from the EKS cluster SG."
  vpc_id      = module.vpc.vpc_id
}

resource "aws_security_group_rule" "redis_from_eks" {
  count = var.elasticache_enabled ? 1 : 0

  type                     = "ingress"
  from_port                = 6379
  to_port                  = 6379
  protocol                 = "tcp"
  security_group_id        = aws_security_group.redis[0].id
  source_security_group_id = module.eks.node_security_group_id
  description              = "Redis from EKS workers"
}

resource "aws_elasticache_replication_group" "redis" {
  count = var.elasticache_enabled ? 1 : 0

  replication_group_id = "${var.name_prefix}-${var.env}"
  description          = "skill-pool Redis read-through cache + rate-limit store"

  engine         = "redis"
  engine_version = "7.1"
  node_type      = var.elasticache_node_type
  port           = 6379

  num_cache_clusters         = 1
  automatic_failover_enabled = false
  multi_az_enabled           = false

  # Encryption: at-rest + in-transit (TLS). Combined with AUTH this gives
  # us defence in depth — even if the SG were misconfigured, the
  # connection still requires the token.
  at_rest_encryption_enabled = true
  transit_encryption_enabled = true
  auth_token                 = random_password.redis_auth[0].result

  subnet_group_name  = aws_elasticache_subnet_group.redis[0].name
  security_group_ids = [aws_security_group.redis[0].id]

  # The single-node node group keeps the bill down; bump to N replicas +
  # `automatic_failover_enabled = true` for production HA.
  apply_immediately = false

  # CloudWatch metrics + slow-log help future capacity planning; the
  # default destination is the AWS-managed log group, no extra cost.
  log_delivery_configuration {
    destination      = "${var.name_prefix}-${var.env}-redis-slow"
    destination_type = "cloudwatch-logs"
    log_format       = "json"
    log_type         = "slow-log"
  }

  tags = {
    Name = "${var.name_prefix}-${var.env}-redis"
  }
}

# CloudWatch log group that the slow-log writes to. Created up-front so
# the replication group has somewhere to deliver — ElastiCache will
# silently disable logging otherwise.
resource "aws_cloudwatch_log_group" "redis_slow" {
  count = var.elasticache_enabled ? 1 : 0

  name              = "${var.name_prefix}-${var.env}-redis-slow"
  retention_in_days = 14
}

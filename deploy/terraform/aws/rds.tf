# Postgres 16 with pgvector. Single-AZ by default to keep the lean baseline
# at ~$50/mo; flip `var.rds_multi_az` for prod HA (~+$50/mo).
#
# pgvector lives in the default `shared_preload_libraries` of recent RDS
# Postgres versions, but the extension itself still needs `CREATE EXTENSION`
# inside the database. That happens during the server's sqlx migration step
# (server/migrations/000X_pgvector.sql), not here.

resource "random_password" "rds_master" {
  length  = 32
  special = false # avoid quoting hazards in DSNs / shell pipelines
}

resource "aws_secretsmanager_secret" "rds_password" {
  name                    = "${var.name_prefix}-${var.env}/rds-password"
  description             = "Postgres master password for skill-pool RDS. Pulled by k8s External Secrets / manual kubectl-create-secret."
  recovery_window_in_days = 7
}

resource "aws_secretsmanager_secret_version" "rds_password" {
  secret_id = aws_secretsmanager_secret.rds_password.id
  secret_string = jsonencode({
    username = "skillpool"
    password = random_password.rds_master.result
    host     = module.rds.db_instance_endpoint
    database = "skillpool"
    # Drop-in DSN. Beware: rotating the password without touching this field
    # leaves the DSN stale. Use jq to read username+password if you rotate.
    dsn = "postgres://skillpool:${random_password.rds_master.result}@${module.rds.db_instance_endpoint}/skillpool?sslmode=require"
  })
}

# Security group: only the EKS cluster security group may reach Postgres on 5432.
resource "aws_security_group" "rds" {
  name        = "${var.name_prefix}-${var.env}-rds"
  description = "RDS Postgres — accessible only from the EKS cluster SG."
  vpc_id      = module.vpc.vpc_id
}

resource "aws_security_group_rule" "rds_from_eks" {
  type                     = "ingress"
  from_port                = 5432
  to_port                  = 5432
  protocol                 = "tcp"
  security_group_id        = aws_security_group.rds.id
  source_security_group_id = module.eks.node_security_group_id
  description              = "Postgres from EKS workers"
}

module "rds" {
  source  = "terraform-aws-modules/rds/aws"
  version = "~> 6.7"

  identifier = "${var.name_prefix}-${var.env}"

  engine               = "postgres"
  engine_version       = var.rds_postgres_version
  family               = "postgres16"
  major_engine_version = "16"
  instance_class       = var.rds_instance_class

  allocated_storage     = var.rds_allocated_storage_gb
  max_allocated_storage = var.rds_allocated_storage_gb * 4 # autoscale ceiling

  db_name  = "skillpool"
  username = "skillpool"
  password = random_password.rds_master.result
  port     = 5432
  # `manage_master_user_password` is the newer auto-rotation feature; we use
  # the static password + Secrets Manager pattern here because the app's
  # connection pool doesn't refresh credentials mid-run. Revisit when sqlx
  # gains credential reload.
  manage_master_user_password = false

  multi_az               = var.rds_multi_az
  db_subnet_group_name   = module.vpc.database_subnet_group_name == null ? null : module.vpc.database_subnet_group_name
  subnet_ids             = module.vpc.private_subnets
  vpc_security_group_ids = [aws_security_group.rds.id]

  # pgvector is on by default in Postgres 16 on RDS; no parameter group tweak
  # needed for the extension. We still set log_statement for slow-query
  # visibility — see docs/ops/runbook.md for the corresponding alert rules.
  parameters = [
    {
      name  = "log_min_duration_statement"
      value = "1000" # ms
    },
    {
      name         = "shared_preload_libraries"
      value        = "pg_stat_statements,vector"
      apply_method = "pending-reboot"
    },
  ]

  backup_retention_period = 7
  skip_final_snapshot     = false
  deletion_protection     = true # CUSTOMISE — flip false in dev/staging.

  performance_insights_enabled = true
}

# Top-level knobs. Most operators only edit the CUSTOMISE-marked defaults.

variable "region" {
  description = "AWS region for every resource. Use the region closest to your tenants — bundles live here too."
  type        = string
  default     = "eu-west-1" # CUSTOMISE
}

variable "env" {
  description = "Environment tag (`prod`, `staging`, `dev`). Drives resource naming and the cluster name."
  type        = string
  default     = "prod" # CUSTOMISE
}

variable "name_prefix" {
  description = "Prefix prepended to every named resource. Useful when sharing an account across many products."
  type        = string
  default     = "skill-pool"
}

# --- Networking ----------------------------------------------------------

variable "vpc_cidr" {
  description = "Top-level VPC CIDR. /16 leaves room for many AZ splits."
  type        = string
  default     = "10.42.0.0/16"
}

variable "azs" {
  description = "Availability zones to spread the cluster + RDS across. Two is the cheap baseline; three is HA."
  type        = list(string)
  default     = ["eu-west-1a", "eu-west-1b"] # CUSTOMISE — must match `region`.
}

# --- EKS -----------------------------------------------------------------

variable "kubernetes_version" {
  description = "EKS control-plane version. Pin and bump intentionally."
  type        = string
  default     = "1.30"
}

variable "node_instance_types" {
  description = "Node group instance types. `t3.medium` is the lean baseline; bump to `m6i.large` for real load."
  type        = list(string)
  default     = ["t3.medium"]
}

variable "node_group_min" {
  description = "Minimum node count per managed node group."
  type        = number
  default     = 2
}

variable "node_group_max" {
  description = "Maximum node count per managed node group. HPA decides within this range."
  type        = number
  default     = 6
}

# --- RDS -----------------------------------------------------------------

variable "rds_instance_class" {
  description = "RDS instance class. `db.t4g.medium` ≈ $50/mo, fine for early prod."
  type        = string
  default     = "db.t4g.medium" # CUSTOMISE
}

variable "rds_allocated_storage_gb" {
  description = "RDS storage in GB. Autoscale on; this is the starting floor."
  type        = number
  default     = 50
}

variable "rds_multi_az" {
  description = "Multi-AZ Postgres. Off by default for cost. Flip to `true` for prod-grade HA."
  type        = bool
  default     = false # CUSTOMISE — flip to true once you've sized the bill.
}

variable "rds_postgres_version" {
  description = "Postgres major.minor. pgvector ships in 16.x by default in AWS RDS."
  type        = string
  default     = "16.3"
}

# --- DNS / TLS -----------------------------------------------------------

variable "route53_zone_name" {
  description = "Existing public hosted zone, e.g. `skill-pool.example.com`. The module references it; it does not create it."
  type        = string
  default     = "skill-pool.example.com" # CUSTOMISE
}

variable "service_hostnames" {
  description = "Hostnames served by the ALB. The ACM cert covers all of these + the wildcard."
  type        = list(string)
  default = [
    "skill-pool.example.com",   # CUSTOMISE — apex
    "*.skill-pool.example.com", # CUSTOMISE — tenant subdomains
  ]
}

# --- GitHub OIDC --------------------------------------------------------

variable "github_repository" {
  description = "`owner/repo` of the GitHub repo allowed to assume the deploy role. Used in the OIDC trust condition."
  type        = string
  default     = "olafkfreund/skill_pool" # CUSTOMISE
}

variable "github_allowed_refs" {
  description = "Refs (branches + tag patterns) allowed to assume the deploy role. Other branches get InvalidIdentityToken."
  type        = list(string)
  default = [
    "refs/heads/main",
    "refs/tags/v*",
  ]
}

# --- Storage / KMS ------------------------------------------------------

variable "bundle_bucket_use_kms" {
  description = "Encrypt the bundle bucket with a managed KMS key (true) or SSE-S3 (false). KMS adds ~$1/mo + per-call cost."
  type        = bool
  default     = false
}

variable "bundle_draft_ttl_days" {
  description = "Days after which un-promoted drafts under `<tenant_id>/drafts/` are expired. Matches packaging/bucket-policy/README.md."
  type        = number
  default     = 14
}

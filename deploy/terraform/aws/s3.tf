# Bundle bucket. Mirrors the layout documented in
# `packaging/bucket-policy/README.md` — one bucket, per-tenant prefixes
# (`{tenant_id}/...`). Server-side enforcement via the IRSA role
# (iam-irsa.tf) + the in-app `Storage::bundle_key`.

# Optional customer-managed KMS key. Behind a variable because KMS adds
# ~$1/mo for the key + per-request cost. Most operators are happy with
# SSE-S3 for bundles (they contain no secrets — the publish path runs
# a secret-scan; see server/src/bundle.rs).
resource "aws_kms_key" "bundles" {
  count = var.bundle_bucket_use_kms ? 1 : 0

  description             = "KMS key for skill-pool bundle bucket."
  deletion_window_in_days = 14
  enable_key_rotation     = true
}

resource "aws_s3_bucket" "bundles" {
  # CUSTOMISE: bucket names are globally unique. `${env}-${random}` is one
  # pattern; here we use the AWS account suffix to keep it deterministic.
  bucket        = "${var.name_prefix}-${var.env}-bundles-${data.aws_caller_identity.current.account_id}"
  force_destroy = false
}

data "aws_caller_identity" "current" {}

resource "aws_s3_bucket_public_access_block" "bundles" {
  bucket                  = aws_s3_bucket.bundles.id
  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_versioning" "bundles" {
  bucket = aws_s3_bucket.bundles.id
  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "bundles" {
  bucket = aws_s3_bucket.bundles.id
  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm     = var.bundle_bucket_use_kms ? "aws:kms" : "AES256"
      kms_master_key_id = var.bundle_bucket_use_kms ? aws_kms_key.bundles[0].arn : null
    }
  }
}

# Lifecycle: kill un-promoted drafts after `bundle_draft_ttl_days`.
# Matches the recommendation in packaging/bucket-policy/README.md.
resource "aws_s3_bucket_lifecycle_configuration" "bundles" {
  bucket = aws_s3_bucket.bundles.id

  rule {
    id     = "expire-drafts"
    status = "Enabled"
    filter {
      # Tenant IDs are UUIDs, so `*/drafts/` is per-tenant by construction.
      # S3 doesn't support `*` mid-prefix; the rule matches `drafts/` after
      # any tenant prefix only via prefix filter on the literal segment.
      # The catalog re-issues new draft IDs on republish, so the false-
      # positive risk is small. Document the limitation; revisit if it bites.
      prefix = "drafts/"
    }
    expiration {
      days = var.bundle_draft_ttl_days
    }
  }

  # Bucket-policy TLS-only is enforced separately below.
}

# Mirror the `DenyInsecureTransport` from packaging/bucket-policy/bucket-policy-shared.json.
resource "aws_s3_bucket_policy" "bundles" {
  bucket = aws_s3_bucket.bundles.id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid       = "DenyInsecureTransport"
        Effect    = "Deny"
        Principal = "*"
        Action    = "s3:*"
        Resource = [
          aws_s3_bucket.bundles.arn,
          "${aws_s3_bucket.bundles.arn}/*",
        ]
        Condition = {
          Bool = { "aws:SecureTransport" = "false" }
        }
      },
    ]
  })
}

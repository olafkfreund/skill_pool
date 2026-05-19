# IRSA (IAM Roles for Service Accounts) for the in-cluster app pod.
#
# Trust: the role can only be assumed by the `skill-pool` service account
# in the `skill-pool` namespace, via the cluster's OIDC provider.
#
# Permissions: S3 RW on the bundle bucket prefix + Secrets Manager
# GetSecretValue on the RDS password (so a startup hook or External
# Secrets Operator can pull it). KMS-decrypt is conditional on the
# `bundle_bucket_use_kms` flag.
#
# Note on RDS IAM auth: deliberately NOT enabled for v1. The app uses
# password auth via the k8s Secret created from Secrets Manager. The
# upgrade path is documented in docs/deploy/aws.md §11.

data "aws_iam_policy_document" "irsa_trust" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRoleWithWebIdentity"]

    principals {
      type        = "Federated"
      identifiers = [module.eks.oidc_provider_arn]
    }

    condition {
      test     = "StringEquals"
      variable = "${replace(module.eks.cluster_oidc_issuer_url, "https://", "")}:sub"
      values   = ["system:serviceaccount:skill-pool:skill-pool"]
    }

    condition {
      test     = "StringEquals"
      variable = "${replace(module.eks.cluster_oidc_issuer_url, "https://", "")}:aud"
      values   = ["sts.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "skill_pool_app" {
  name               = "${var.name_prefix}-${var.env}-app"
  description        = "Assumed by the skill-pool server pod via IRSA. Scoped to S3 bundle bucket + RDS password secret."
  assume_role_policy = data.aws_iam_policy_document.irsa_trust.json
}

# Mirrors the `AllowAppShared` Statement in packaging/bucket-policy/iam-policy-app.json.
data "aws_iam_policy_document" "app_s3" {
  statement {
    sid    = "AllowAppShared"
    effect = "Allow"
    actions = [
      "s3:GetObject",
      "s3:PutObject",
      "s3:DeleteObject",
    ]
    resources = ["${aws_s3_bucket.bundles.arn}/*"]
  }

  statement {
    sid       = "AllowAppSharedListBucket"
    effect    = "Allow"
    actions   = ["s3:ListBucket"]
    resources = [aws_s3_bucket.bundles.arn]
  }

  # Only emit a KMS Statement when the bucket actually uses KMS — keeping
  # the policy tight avoids spurious access to unrelated KMS keys.
  dynamic "statement" {
    for_each = var.bundle_bucket_use_kms ? [1] : []
    content {
      sid    = "AllowAppBundleKms"
      effect = "Allow"
      actions = [
        "kms:Decrypt",
        "kms:Encrypt",
        "kms:GenerateDataKey",
        "kms:DescribeKey",
      ]
      resources = [aws_kms_key.bundles[0].arn]
    }
  }

  statement {
    sid       = "AllowReadRdsPasswordSecret"
    effect    = "Allow"
    actions   = ["secretsmanager:GetSecretValue", "secretsmanager:DescribeSecret"]
    resources = [aws_secretsmanager_secret.rds_password.arn]
  }
}

resource "aws_iam_policy" "skill_pool_app" {
  name        = "${var.name_prefix}-${var.env}-app"
  description = "S3 RW on the bundle bucket prefix + read RDS password secret."
  policy      = data.aws_iam_policy_document.app_s3.json
}

resource "aws_iam_role_policy_attachment" "skill_pool_app" {
  role       = aws_iam_role.skill_pool_app.name
  policy_arn = aws_iam_policy.skill_pool_app.arn
}

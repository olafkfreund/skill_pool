# GitHub Actions → AWS via OIDC. No long-lived access keys; each workflow
# run mints a short-lived STS session via the OIDC provider below.
#
# References:
#   - https://docs.github.com/en/actions/deployment/security-hardening-your-deployments/about-security-hardening-with-openid-connect
#   - https://docs.aws.amazon.com/IAM/latest/UserGuide/id_roles_create_for-idp_oidc.html

# Thumbprint pinning is no longer strictly required — AWS validates the GitHub
# OIDC cert chain against the standard public roots since 2023 — but we leave
# the legacy thumbprint here for compatibility with older AWS partitions.
# `tls_certificate` fetches it dynamically so we don't hand-pin a hash.
data "tls_certificate" "github" {
  url = "https://token.actions.githubusercontent.com"
}

resource "aws_iam_openid_connect_provider" "github" {
  url            = "https://token.actions.githubusercontent.com"
  client_id_list = ["sts.amazonaws.com"]
  thumbprint_list = [
    data.tls_certificate.github.certificates[0].sha1_fingerprint,
  ]
}

# Build a `sub` claim allow-list of "repo:<owner>/<repo>:ref:<ref>" entries
# from the var.github_allowed_refs list, so the trust policy explicitly
# enumerates which refs may assume the role.
locals {
  github_sub_allowed = [
    for ref in var.github_allowed_refs :
    "repo:${var.github_repository}:ref:${ref}"
  ]
}

data "aws_iam_policy_document" "github_actions_trust" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRoleWithWebIdentity"]

    principals {
      type        = "Federated"
      identifiers = [aws_iam_openid_connect_provider.github.arn]
    }

    condition {
      test     = "StringEquals"
      variable = "token.actions.githubusercontent.com:aud"
      values   = ["sts.amazonaws.com"]
    }

    # StringLike so we can use the `refs/tags/v*` glob for release tags.
    condition {
      test     = "StringLike"
      variable = "token.actions.githubusercontent.com:sub"
      values   = local.github_sub_allowed
    }
  }
}

resource "aws_iam_role" "github_actions" {
  name               = "${var.name_prefix}-${var.env}-github-actions"
  description        = "Assumed by GitHub Actions workflows on main + release tags. ECR push + EKS describe."
  assume_role_policy = data.aws_iam_policy_document.github_actions_trust.json
  # Default session is 1 hour. Workflows that need longer should bump per-job.
  max_session_duration = 3600
}

data "aws_iam_policy_document" "github_actions" {
  # ECR auth + push, scoped to our two repos only.
  statement {
    sid     = "EcrAuth"
    effect  = "Allow"
    actions = ["ecr:GetAuthorizationToken"]
    # ecr:GetAuthorizationToken is account-wide — `*` is the documented form.
    resources = ["*"]
  }
  statement {
    sid    = "EcrPush"
    effect = "Allow"
    actions = [
      "ecr:BatchCheckLayerAvailability",
      "ecr:CompleteLayerUpload",
      "ecr:InitiateLayerUpload",
      "ecr:PutImage",
      "ecr:UploadLayerPart",
      "ecr:DescribeImages",
      "ecr:BatchGetImage",
      "ecr:DescribeRepositories",
    ]
    resources = [
      aws_ecr_repository.server.arn,
      aws_ecr_repository.web.arn,
    ]
  }

  # EKS describe so the workflow can `aws eks update-kubeconfig`. Note: this
  # only mints a kubeconfig — actual cluster RBAC is granted in eks.tf via
  # access_entries, NOT here. K8s authorisation is a separate hop on purpose.
  statement {
    sid     = "EksDescribe"
    effect  = "Allow"
    actions = ["eks:DescribeCluster", "eks:ListClusters"]
    resources = [
      module.eks.cluster_arn,
    ]
  }

  # Read the RDS password secret — useful for the deploy job to construct
  # the k8s Secret manifest from CI. Comment out if you'd rather scope this
  # to a dedicated rotation role.
  statement {
    sid       = "ReadRdsPasswordForK8sSecret"
    effect    = "Allow"
    actions   = ["secretsmanager:GetSecretValue"]
    resources = [aws_secretsmanager_secret.rds_password.arn]
  }
}

resource "aws_iam_policy" "github_actions" {
  name        = "${var.name_prefix}-${var.env}-github-actions"
  description = "ECR push + EKS describe + read RDS secret. Scoped to skill-pool resources only."
  policy      = data.aws_iam_policy_document.github_actions.json
}

resource "aws_iam_role_policy_attachment" "github_actions" {
  role       = aws_iam_role.github_actions.name
  policy_arn = aws_iam_policy.github_actions.arn
}

# K8s RBAC for the deploy role: grant `system:masters`-equivalent on the
# cluster via an EKS access entry, so workflows can helm-apply. Tighten to
# a narrower role in production (e.g. only `skill-pool` namespace edit).
resource "aws_eks_access_entry" "github_actions" {
  cluster_name      = module.eks.cluster_name
  principal_arn     = aws_iam_role.github_actions.arn
  kubernetes_groups = []
  type              = "STANDARD"
}

resource "aws_eks_access_policy_association" "github_actions" {
  cluster_name = module.eks.cluster_name
  # CUSTOMISE: AmazonEKSAdminPolicy is broad. Swap for AmazonEKSEditPolicy
  # + a namespace scope once you've validated the workflow.
  policy_arn    = "arn:aws:eks::aws:cluster-access-policy/AmazonEKSAdminPolicy"
  principal_arn = aws_iam_role.github_actions.arn

  access_scope {
    type = "cluster"
  }
}

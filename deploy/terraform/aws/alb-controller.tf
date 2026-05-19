# AWS Load Balancer Controller — the in-cluster operator that turns
# Kubernetes Ingress objects into AWS ALBs. Required for the
# `ingressClassName: alb` in values-aws.yaml.
#
# Two pieces: (1) IAM role + policy bound to the controller's service
# account via IRSA, (2) the helm_release that actually installs the
# controller into kube-system.

# The canonical IAM policy JSON is published by the controller project.
# We embed it inline because (a) it changes rarely and (b) inlining beats
# pulling a remote file at plan time. Source:
# https://raw.githubusercontent.com/kubernetes-sigs/aws-load-balancer-controller/v2.8.2/docs/install/iam_policy.json
# Re-fetch and update when bumping the controller chart version.
data "http" "alb_controller_iam_policy" {
  url = "https://raw.githubusercontent.com/kubernetes-sigs/aws-load-balancer-controller/v2.8.2/docs/install/iam_policy.json"
  request_headers = {
    Accept = "application/json"
  }
}

resource "aws_iam_policy" "alb_controller" {
  name        = "${var.name_prefix}-${var.env}-alb-controller"
  description = "AWS Load Balancer Controller — manages ALBs from Ingress objects."
  policy      = data.http.alb_controller_iam_policy.response_body
}

# IRSA trust for the controller's service account.
data "aws_iam_policy_document" "alb_controller_trust" {
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
      values   = ["system:serviceaccount:kube-system:aws-load-balancer-controller"]
    }

    condition {
      test     = "StringEquals"
      variable = "${replace(module.eks.cluster_oidc_issuer_url, "https://", "")}:aud"
      values   = ["sts.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "alb_controller" {
  name               = "${var.name_prefix}-${var.env}-alb-controller"
  assume_role_policy = data.aws_iam_policy_document.alb_controller_trust.json
}

resource "aws_iam_role_policy_attachment" "alb_controller" {
  role       = aws_iam_role.alb_controller.name
  policy_arn = aws_iam_policy.alb_controller.arn
}

# Helm install of the controller chart.
resource "helm_release" "alb_controller" {
  name       = "aws-load-balancer-controller"
  repository = "https://aws.github.io/eks-charts"
  chart      = "aws-load-balancer-controller"
  # Pin to a known-good chart version. Bump deliberately when you bump the
  # IAM policy URL above.
  version   = "1.8.1"
  namespace = "kube-system"

  set {
    name  = "clusterName"
    value = module.eks.cluster_name
  }

  set {
    name  = "serviceAccount.create"
    value = "true"
  }
  set {
    name  = "serviceAccount.name"
    value = "aws-load-balancer-controller"
  }
  set {
    name  = "serviceAccount.annotations.eks\\.amazonaws\\.com/role-arn"
    value = aws_iam_role.alb_controller.arn
  }

  set {
    name  = "region"
    value = var.region
  }
  set {
    name  = "vpcId"
    value = module.vpc.vpc_id
  }

  # Order matters: the controller needs the cluster up first, the IAM role
  # attached, and the EBS CSI addon ready (the chart pulls some shared CRDs).
  depends_on = [
    module.eks,
    aws_iam_role_policy_attachment.alb_controller,
  ]
}

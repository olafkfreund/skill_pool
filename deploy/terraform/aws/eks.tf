# EKS cluster on the VPC's private subnets, two managed node groups.
# IRSA enabled so the app pod can pick up an IAM role via service account
# annotation (see iam-irsa.tf).
#
# Why two node groups: keeps surge upgrades safe — one group can roll while
# the other carries traffic. They are identical; differentiate later if you
# need spot vs on-demand or arm64 vs x86_64.

module "eks" {
  source  = "terraform-aws-modules/eks/aws"
  version = "~> 20.24"

  cluster_name    = "${var.name_prefix}-${var.env}"
  cluster_version = var.kubernetes_version

  vpc_id                         = module.vpc.vpc_id
  subnet_ids                     = module.vpc.private_subnets
  cluster_endpoint_public_access = true # CUSTOMISE — turn off + use a VPN/jump host in regulated envs.

  # IRSA: required for the app's S3 access via service account annotation.
  enable_irsa = true

  # Built-in addons. coredns + kube-proxy + vpc-cni are mandatory; pod-identity
  # is the modern IRSA replacement but optional here.
  cluster_addons = {
    coredns = {
      most_recent = true
    }
    kube-proxy = {
      most_recent = true
    }
    vpc-cni = {
      most_recent = true
    }
    aws-ebs-csi-driver = {
      most_recent = true
    }
  }

  # Two managed node groups, identical, so we can surge-roll.
  eks_managed_node_groups = {
    workers_a = {
      instance_types = var.node_instance_types
      min_size       = var.node_group_min
      max_size       = var.node_group_max
      desired_size   = var.node_group_min
      subnet_ids     = [module.vpc.private_subnets[0]]
      labels         = { workload = "general" }
    }
    workers_b = {
      instance_types = var.node_instance_types
      min_size       = var.node_group_min
      max_size       = var.node_group_max
      desired_size   = var.node_group_min
      subnet_ids     = [module.vpc.private_subnets[1]]
      labels         = { workload = "general" }
    }
  }

  # CUSTOMISE: add cluster-admin entries for your team's IAM principals here.
  # The bootstrap operator (the IAM principal running `terraform apply`) is
  # added automatically. Everyone else needs an explicit entry.
  access_entries = {}
}

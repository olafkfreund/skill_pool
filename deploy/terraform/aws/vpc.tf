# Two-AZ VPC, public + private subnets. Single NAT gateway to keep the
# bill down — for HA flip `single_nat_gateway = false`, which costs about
# $32/mo extra per additional AZ.
#
# Why the community module: VPC layout is high-volume, opinionated, and
# riddled with EKS-specific tagging (`kubernetes.io/role/elb`,
# `kubernetes.io/role/internal-elb`) that the module bakes in for you.

module "vpc" {
  source  = "terraform-aws-modules/vpc/aws"
  version = "~> 5.13"

  name = "${var.name_prefix}-${var.env}"
  cidr = var.vpc_cidr

  azs             = var.azs
  public_subnets  = [for i, _ in var.azs : cidrsubnet(var.vpc_cidr, 8, i)]      # /24 each
  private_subnets = [for i, _ in var.azs : cidrsubnet(var.vpc_cidr, 8, i + 10)] # /24 each, offset 10

  enable_nat_gateway   = true
  single_nat_gateway   = true # CUSTOMISE — flip false for per-AZ NAT (HA, +$32/mo each).
  enable_dns_hostnames = true
  enable_dns_support   = true

  # ALB-controller-required tags. The module reads these for the auto-discovery
  # mode the controller uses when it creates ALBs from Ingress objects.
  public_subnet_tags = {
    "kubernetes.io/role/elb" = "1"
  }
  private_subnet_tags = {
    "kubernetes.io/role/internal-elb" = "1"
  }
}

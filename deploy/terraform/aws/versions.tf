# Pinned provider + Terraform versions. Bump deliberately — the community
# modules (terraform-aws-modules/*) track these and a sloppy bump can cascade
# into a hundred-resource diff.
terraform {
  required_version = ">= 1.6.0, < 2.0.0"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.60"
    }
    # Used by alb-controller.tf and (optionally) the operator's k8s secret bootstrap.
    helm = {
      source  = "hashicorp/helm"
      version = "~> 2.14"
    }
    kubernetes = {
      source  = "hashicorp/kubernetes"
      version = "~> 2.31"
    }
    tls = {
      source  = "hashicorp/tls"
      version = "~> 4.0"
    }
    random = {
      source  = "hashicorp/random"
      version = "~> 3.6"
    }
    http = {
      source  = "hashicorp/http"
      version = "~> 3.4"
    }
  }

  # CUSTOMISE: point this at your own S3 backend + DynamoDB lock table.
  # The block below is commented out so `terraform init` works locally
  # without any backend, but you should set this before applying to a
  # shared environment.
  #
  # backend "s3" {
  #   bucket         = "skill-pool-tfstate"
  #   key            = "aws/prod/terraform.tfstate"
  #   region         = "eu-west-1"
  #   dynamodb_table = "skill-pool-tflock"
  #   encrypt        = true
  # }
}

provider "aws" {
  region = var.region

  default_tags {
    tags = {
      Project   = "skill-pool"
      ManagedBy = "terraform"
      Env       = var.env
    }
  }
}

# Helm + kubernetes providers are wired to the EKS cluster that this module
# creates. The data sources are evaluated lazily, so `terraform plan` works
# even before the cluster exists.
provider "helm" {
  kubernetes {
    host                   = module.eks.cluster_endpoint
    cluster_ca_certificate = base64decode(module.eks.cluster_certificate_authority_data)
    exec {
      api_version = "client.authentication.k8s.io/v1beta1"
      command     = "aws"
      args        = ["eks", "get-token", "--cluster-name", module.eks.cluster_name, "--region", var.region]
    }
  }
}

provider "kubernetes" {
  host                   = module.eks.cluster_endpoint
  cluster_ca_certificate = base64decode(module.eks.cluster_certificate_authority_data)
  exec {
    api_version = "client.authentication.k8s.io/v1beta1"
    command     = "aws"
    args        = ["eks", "get-token", "--cluster-name", module.eks.cluster_name, "--region", var.region]
  }
}

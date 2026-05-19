# Two ECR repos: one for the Rust server image, one for the Next.js web image.
# Image scanning on push catches CVE-laden base images before they hit prod.
# The lifecycle policy keeps the last 10 untagged images + every `v*` tag
# forever — preserves your ability to roll back to any released version.

locals {
  ecr_lifecycle_policy = jsonencode({
    rules = [
      {
        rulePriority = 1
        description  = "Keep every release tag (v*) forever."
        selection = {
          tagStatus     = "tagged"
          tagPrefixList = ["v"]
          countType     = "imageCountMoreThan"
          countNumber   = 10000
        }
        action = { type = "expire" } # high threshold ⇒ never expires
      },
      {
        rulePriority = 2
        description  = "Keep last 10 untagged (CI sha-only) images."
        selection = {
          tagStatus   = "untagged"
          countType   = "imageCountMoreThan"
          countNumber = 10
        }
        action = { type = "expire" }
      },
    ]
  })
}

resource "aws_ecr_repository" "server" {
  name                 = "${var.name_prefix}-server"
  image_tag_mutability = "IMMUTABLE" # tags can't be overwritten; release safety.

  image_scanning_configuration {
    scan_on_push = true
  }

  encryption_configuration {
    encryption_type = "AES256"
  }
}

resource "aws_ecr_lifecycle_policy" "server" {
  repository = aws_ecr_repository.server.name
  policy     = local.ecr_lifecycle_policy
}

resource "aws_ecr_repository" "web" {
  name                 = "${var.name_prefix}-web"
  image_tag_mutability = "IMMUTABLE"

  image_scanning_configuration {
    scan_on_push = true
  }

  encryption_configuration {
    encryption_type = "AES256"
  }
}

resource "aws_ecr_lifecycle_policy" "web" {
  repository = aws_ecr_repository.web.name
  policy     = local.ecr_lifecycle_policy
}

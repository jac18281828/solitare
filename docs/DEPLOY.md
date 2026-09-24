# Production Deploy (S3 + CloudFront)

The CDK app under `cdk/` defines every AWS resource `solitare.2ad.com` needs.
Every tag deploys that stack, then syncs built content into it.

## Infrastructure (`cdk/`)

`StackSolitare2adCom` owns, in `us-east-1`:
- the origin S3 bucket `solitare-us-east-1-504242000181`, private and encrypted
- the ACM certificate for `solitare.2ad.com`, DNS-validated
- the CloudFront distribution, its Origin Access Control and 404 page
- the bucket policy granting read and list access to that distribution alone
- the `solitare` A and AAAA alias records

It does not own the `2ad.com` hosted zone: that is imported read-only and managed by
no stack, so it survives the teardown of any one site. It does not own site content
either — `deploy-static-site.yml` puts that in the bucket.

The tag workflow deploys this stack on every run. Use these commands directly only
for a first deploy or a teardown:

```sh
bun install
bun run cdk:synth StackSolitare2adCom
bun run cdk:deploy StackSolitare2adCom
```

The tag workflow reads the stack's `DistributionId` output straight from
`cdk-outputs.json`, written by its own `cdk deploy`; a manual deploy has no such
file, so invalidate the distribution by hand afterward.

The bucket is destroyed with the stack and emptied on the way out, so
`bun run cdk:destroy StackSolitare2adCom` deletes the live site's content. That is
safe only because `trunk build` regenerates it from source; a retained bucket would
instead block the next deploy with `BucketAlreadyExists`.

## Content (`deploy-static-site.yml`)

Deployment target settings:
- `SITE_URL`: `https://solitare.2ad.com`
- `AWS_REGION`: `us-east-1`
- `S3_BUCKET_NAME`: `solitare-us-east-1-504242000181`

Workflow behavior:
- Runs on tag push or manual dispatch.
- Builds and tests Rust/WASM.
- Builds static assets with `trunk`.
- Typechecks and tests the CDK stack.
- Deploys `StackSolitare2adCom`, writing `cdk-outputs.json`.
- Syncs `dist/` to the private S3 bucket.
- Uploads `index.html` with no-cache headers.
- Invalidates CloudFront using the `DistributionId` from `cdk-outputs.json`.

Required GitHub setup:
1. AWS OIDC role trust for GitHub Actions:
   - `arn:aws:iam::504242000181:role/GithubDeployCI`
2. `us-east-1` in account 504242000181 is CDK-bootstrapped, and
   `GithubDeployCI` can assume its deploy and file-publishing roles.

Notes:
- The deploy workflow deploys the stack before syncing content.
- Bucket, region and domain are defined in `cdk/solitare-stack.ts`; the workflow env
  values must match it.

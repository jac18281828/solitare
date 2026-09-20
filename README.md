# Solitare of Olympus

Ancient Greek and Roman themed Klondike solitaire built with Rust + Yew + WebAssembly.

## Features

- Full 52-card Klondike deal (7 tableau columns, stock, waste, 4 foundations)
- Click-to-select and click-to-move interactions, or drag a card or run
  straight to its destination on touch and mouse
- Double-click waste or top tableau card to auto-send to foundation
- Foundation and tableau legality checks
- Auto-flip hidden tableau cards after moves
- Win detection when all four foundations reach King
- Green felt board with gold, ivy, Cupid/Bacchus visual theme

## Run

1. Install Rust and Trunk.
2. Add wasm target:
   - `rustup target add wasm32-unknown-unknown`
3. Serve locally:
   - `trunk serve --release`
4. Open:
   - `http://127.0.0.1:8080`

## Controls

- Click `Stock` to draw (or redeal waste back into stock)
- Click a card/run to select it, or drag it straight to a destination
- Click a destination tableau/foundation to move a selected card
- `Auto To Temple` sends one available card to foundation
- `New Shuffle` starts a fresh game

## VS Code Dev Container

This repo includes a complete dev container setup:

- Dev container config: `.devcontainer/devcontainer.json`
- Dockerfile used by the container build: `Dockerfile`

Start steps:

1. Open the repo root in VS Code.
2. Run `Dev Containers: Reopen in Container`.
3. In the container terminal run `trunk serve --release`.
4. Open `http://127.0.0.1:8080`.

## Production Deploy (S3 + CloudFront)

The CDK app under `cdk/` defines every AWS resource `solitare.2ad.com` needs.
Every tag deploys that stack, then syncs built content into it.

### Infrastructure (`cdk/`)

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

### Content (`deploy-static-site.yml`)

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

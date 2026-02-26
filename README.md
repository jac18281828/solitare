# Solitare of Olympus

Ancient Greek and Roman themed Klondike solitaire built with Rust + Yew + WebAssembly.

## Features

- Full 52-card Klondike deal (7 tableau columns, stock, waste, 4 foundations)
- Click-to-select and click-to-move interactions
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
- Click a card/run to select it
- Click destination tableau/foundation to move selected card(s)
- `Auto To Temple` sends one available card to foundation
- `New Shuffle` starts a fresh game

## VS Code Dev Container

This repo includes a complete dev container setup:

- Dev container config: `/Users/john/sandbox/solitare/.devcontainer/devcontainer.json`
- Dockerfile used by the container build: `/Users/john/sandbox/solitare/Dockerfile`

Start steps:

1. Open `/Users/john/sandbox/solitare` in VS Code.
2. Run `Dev Containers: Reopen in Container`.
3. In the container terminal run `trunk serve --release`.
4. Open `http://127.0.0.1:8080`.

## Production Deploy (S3 + CloudFront)

This repo includes a deploy workflow at:
- `/Users/john/sandbox/solitare/.github/workflows/deploy-static-site.yml`

Deployment target settings:
- `SITE_URL`: `https://solitare.2ad.com`
- `AWS_REGION`: `us-east-2`
- `S3_BUCKET_NAME`: `solitare-us-east-2-504242000181`

Workflow behavior:
- Runs on tag push or manual dispatch.
- Builds and tests Rust/WASM.
- Builds static assets with `trunk`.
- Syncs `dist/` to the private S3 bucket.
- Uploads `index.html` with no-cache headers.
- Invalidates CloudFront if `CLOUDFRONT_DISTRIBUTION_ID` secret is set.

Required GitHub setup:
1. Repository secret: `CLOUDFRONT_DISTRIBUTION_ID` (CloudFront distribution ID for `solitare.2ad.com`).
2. AWS OIDC role trust for GitHub Actions:
   - `arn:aws:iam::504242000181:role/GithubDeployCI`
3. Existing S3 bucket and CloudFront distribution configured for this site.

Notes:
- The deploy workflow does not create infrastructure.
- If bucket/distribution names differ, update the workflow env values accordingly.

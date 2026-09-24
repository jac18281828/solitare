# Solitare of Olympus

Ancient Greek and Roman themed Klondike solitaire built with Rust + Yew + WebAssembly.

## Features

- Full 52-card Klondike deal (7 tableau columns, stock, waste, 4 foundations)
- Click-to-select and click-to-move interactions, or drag a card or run
  straight to its destination on touch and mouse
- Double-click waste or top tableau card to auto-send to foundation
- Cards fly to where they land, with a soft settle-back on a rejected
  drop; honours `prefers-reduced-motion` throughout
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

## Deploy

Production deploy notes (CDK, S3, CloudFront, GitHub Actions) live in
[`docs/DEPLOY.md`](docs/DEPLOY.md).

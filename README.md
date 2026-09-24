# Solitare of Olympus

Play cards with Cupid, ivy, Bacchus, and temple gold.

<p>
<a href="https://solitare.2ad.com"><img src="docs/media/board.webp" width="72%" alt="Solitare of Olympus, desktop board mid-game with a temple started"></a><a href="https://solitare.2ad.com"><img src="docs/media/phone.webp" width="26%" alt="Solitare of Olympus, phone board mid-game with a temple started"></a>
</p>

**[Play now](https://solitare.2ad.com)**  
Plays in your phone's browser. No app, no install.

![Touch play: tap the stock, drag a card into place, send one to a temple](docs/media/play.gif)

Tap the stock, drag a card into place, send one to a temple.

## How to play

Draw from stock and build the four temples from Ace to King.

**Moving cards.** Tap or click a card to select it, then tap or click where it goes. Drag a card straight to its destination instead, and it flies there. Double-click, or double-tap on a phone, a waste or top tableau card to send it to a temple.

**Temple Gold.** A card that reaches a temple earns a gold. So does a waste card placed on the tableau. Rearranging the tableau earns nothing. Recycling the waste back into stock costs a gold, and running out that way ends the game.

**Buttons.**
- `Auto To Temple` sends one eligible card to a temple.
- `All To Temple` repeats that until no temple move remains.
- `Zeus' Vision` reveals every hidden card — and ends the game.
- `Easy`/`Hard` switches the draw between one card and three, only before your first move.
- `Give Up!` deals a fresh game at 0 gold; after a win it reads `Play Again` and keeps your gold.

**Keyboard.** `D` or `Enter` draws, `Space` sends one card to a temple, `A` sends every eligible card.

**Game over** when all four temples reach King, keeping your gold; or you lose it — out of gold on a recycle, no legal move left, or Zeus' Vision called.

## Features

- Drag any card with a finger; a swipe on the felt still scrolls the page.
- A phone-sized board and buttons, not a shrunk desktop one.
- Full mouse and keyboard play alongside touch.
- Cards fly to where they land, honoring `prefers-reduced-motion`.
- Temple gold carries from one win into the next game.

## About

Solitare of Olympus is Rust and Yew compiled to WebAssembly. Game logic lives in `src/game.rs`, pure and tested with host-run unit tests. Every tag deploys to [solitare.2ad.com](https://solitare.2ad.com) through GitHub Actions and a CDK stack — see [`docs/DEPLOY.md`](docs/DEPLOY.md).

## Run it locally

1. Install Rust and Trunk.
2. Add the wasm target:
   - `rustup target add wasm32-unknown-unknown`
3. Serve locally:
   - `trunk serve --release`
4. Open:
   - `http://127.0.0.1:8080`

### VS Code Dev Container

This repo includes a complete dev container setup:

- Dev container config: `.devcontainer/devcontainer.json`
- Dockerfile used by the container build: `Dockerfile`

Start steps:

1. Open the repo root in VS Code.
2. Run `Dev Containers: Reopen in Container`.
3. In the container terminal run `trunk serve --release`.
4. Open `http://127.0.0.1:8080`.

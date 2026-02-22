FROM mcr.microsoft.com/devcontainers/rust:1-1-bookworm

ARG USERNAME=vscode
ARG PROJECT=solitare

WORKDIR /workspaces/${PROJECT}

RUN su ${USERNAME} -c "rustup target add wasm32-unknown-unknown" \
    && su ${USERNAME} -c "cargo install trunk --locked"

ENV PATH=/home/${USERNAME}/.cargo/bin:${PATH}

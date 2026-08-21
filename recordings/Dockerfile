# syntax=docker/dockerfile:1

FROM rust:1.88-bookworm AS rust-builder

WORKDIR /source
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN --mount=type=cache,id=claude-rs-recording-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=claude-rs-recording-target,target=/source/target \
    cargo build --locked --release && \
    cp target/release/claude-rs /tmp/claude-rs

FROM node:24-bookworm AS bridge-builder

WORKDIR /source/agent-sdk
COPY agent-sdk/package.json agent-sdk/package-lock.json ./
RUN --mount=type=cache,id=claude-rs-recording-npm,target=/root/.npm npm ci
COPY agent-sdk/tsconfig.json ./
COPY agent-sdk/src ./src
RUN npm run build && npm prune --omit=dev

FROM oven/bun:1.3.14 AS bun-runtime

FROM ghcr.io/charmbracelet/vhs:latest

COPY --from=rust-builder /tmp/claude-rs /usr/local/bin/claude-rs
COPY --from=bun-runtime /usr/local/bin/bun /usr/local/bin/claude-rs-bridge-bun
COPY --from=bridge-builder /source/agent-sdk/package.json /opt/claude-rs/agent-sdk/package.json
COPY --from=bridge-builder /source/agent-sdk/dist /opt/claude-rs/agent-sdk/dist
COPY --from=bridge-builder /source/agent-sdk/node_modules /opt/claude-rs/agent-sdk/node_modules
RUN chmod +x /usr/local/bin/claude-rs /usr/local/bin/claude-rs-bridge-bun && mkdir -p /root/.claude

WORKDIR /workspace/claude-code-rust
ENTRYPOINT ["/usr/bin/vhs"]
CMD ["/recording/demo.tape"]

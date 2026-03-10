# ════════════════════════════════════════════════════════════════════════════════
# Stage 1 — Build
#
# Strategy: copy all Cargo manifests first and compile with stub sources so that
# the dependency layer is cached independently of application source changes.
# Only aura-proxy and its deps are actually compiled (cargo -p flag).
# ════════════════════════════════════════════════════════════════════════════════
FROM rust:1.85-slim AS builder

# tokio-tungstenite uses native-tls which requires OpenSSL headers at compile time.
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# ── Copy all workspace Cargo manifests (invalidates cache only on dep changes) ─
COPY Cargo.toml Cargo.lock ./
COPY crates/aura-bridge/Cargo.toml  crates/aura-bridge/Cargo.toml
COPY crates/aura-daemon/Cargo.toml  crates/aura-daemon/Cargo.toml
COPY crates/aura-gemini/Cargo.toml  crates/aura-gemini/Cargo.toml
COPY crates/aura-input/Cargo.toml   crates/aura-input/Cargo.toml
COPY crates/aura-memory/Cargo.toml  crates/aura-memory/Cargo.toml
COPY crates/aura-menubar/Cargo.toml crates/aura-menubar/Cargo.toml
COPY crates/aura-proxy/Cargo.toml   crates/aura-proxy/Cargo.toml
COPY crates/aura-screen/Cargo.toml  crates/aura-screen/Cargo.toml
COPY crates/aura-voice/Cargo.toml   crates/aura-voice/Cargo.toml

# ── Stub sources for all workspace members ────────────────────────────────────
# Cargo requires src files to exist for the workspace to resolve.
# We only need aura-proxy's deps compiled; the other crates are not transitive
# deps of aura-proxy so they won't be compiled regardless.
RUN set -e; \
    for lib_crate in aura-bridge aura-gemini aura-input aura-memory \
                     aura-menubar aura-screen aura-voice; do \
        mkdir -p "crates/${lib_crate}/src"; \
        printf '' > "crates/${lib_crate}/src/lib.rs"; \
    done; \
    mkdir -p crates/aura-daemon/src; \
    printf '' > crates/aura-daemon/src/lib.rs; \
    printf 'fn main() {}' > crates/aura-daemon/src/main.rs; \
    mkdir -p crates/aura-proxy/src; \
    printf 'pub mod relay;\n' > crates/aura-proxy/src/lib.rs; \
    printf '' > crates/aura-proxy/src/relay.rs; \
    printf 'fn main() {}\n' > crates/aura-proxy/src/main.rs

# Build dependencies only — cached until Cargo.toml/Cargo.lock changes.
RUN cargo build --release -p aura-proxy

# ── Copy real proxy source and do the final build ────────────────────────────
# Remove stub artifacts so Cargo sees the sources as changed.
RUN rm -rf \
    target/release/aura-proxy \
    target/release/deps/aura_proxy-* \
    target/release/deps/aura-proxy-* \
    target/release/.fingerprint/aura-proxy-* \
    target/release/.fingerprint/aura_proxy-* \
    target/release/incremental/aura_proxy-*

COPY crates/aura-proxy/src crates/aura-proxy/src

RUN cargo build --release --locked -p aura-proxy

# ════════════════════════════════════════════════════════════════════════════════
# Stage 2 — Runtime
# ════════════════════════════════════════════════════════════════════════════════
FROM debian:bookworm-slim AS runtime

# ca-certificates is required for TLS connections to Gemini (wss://).
# curl is needed for the HEALTHCHECK (must be installed before switching user).
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
    && rm -rf /var/lib/apt/lists/*

# Run as non-root.
RUN useradd -m -u 1001 aura
USER aura

COPY --from=builder /app/target/release/aura-proxy /usr/local/bin/aura-proxy

# Cloud Run sets PORT at runtime. Default to 8080 for local docker run.
ENV PORT=8080
EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD ["/bin/sh", "-c", "curl -sf http://localhost:${PORT}/health || exit 1"]

CMD ["aura-proxy"]

FROM rust:1.85-bookworm AS builder
WORKDIR /app
COPY consolidation/ .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/aura-consolidation /usr/local/bin/
ENV PORT=8080
EXPOSE 8080
CMD ["aura-consolidation"]

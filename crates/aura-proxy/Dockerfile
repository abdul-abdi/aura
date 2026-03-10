FROM rust:1.85 AS builder
WORKDIR /app
COPY . .
RUN cargo build --release -p aura-proxy

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/aura-proxy /usr/local/bin/
ENV PORT=8080
EXPOSE 8080
CMD ["aura-proxy"]

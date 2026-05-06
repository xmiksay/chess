# syntax=docker/dockerfile:1.7

# --- Stage 1: build the binary ---
FROM rust:1-slim-bookworm AS builder
WORKDIR /app
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config \
    && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked
RUN strip target/release/chess-puzzles

# --- Stage 2: fetch the Lichess puzzle dataset ---
FROM debian:bookworm-slim AS puzzles
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /data
ARG LICHESS_PUZZLE_URL=https://database.lichess.org/lichess_db_puzzle.csv.zst
RUN curl -fL --retry 3 -o lichess_db_puzzle.csv.zst "$LICHESS_PUZZLE_URL"

# --- Stage 3: runtime image ---
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 10001 -m -d /app -s /usr/sbin/nologin chess
WORKDIR /app
COPY --from=builder /app/target/release/chess-puzzles /usr/local/bin/chess-puzzles
COPY --from=puzzles /data/lichess_db_puzzle.csv.zst /app/lichess_db_puzzle.csv.zst
COPY static /app/static
RUN chown -R chess:chess /app
USER chess
EXPOSE 3000
ENV RUST_LOG=info,tower_http=info
ENTRYPOINT ["/usr/local/bin/chess-puzzles"]
CMD ["--csv", "/app/lichess_db_puzzle.csv.zst", "--static-dir", "/app/static", "--bind", "0.0.0.0:3000"]

FROM rust:1-bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY tarn ./tarn
COPY tarn-mcp ./tarn-mcp
COPY tarn-lsp ./tarn-lsp
COPY demo-server ./demo-server
# tarn-lsp/src/schema.rs uses `include_str!("../../schemas/v1/testfile.json")`
# at compile time, so the build context needs the schemas directory.
COPY schemas ./schemas

RUN cargo build --release -p tarn -p tarn-mcp -p tarn-lsp

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/tarn /usr/local/bin/tarn
COPY --from=builder /app/target/release/tarn-mcp /usr/local/bin/tarn-mcp
COPY --from=builder /app/target/release/tarn-lsp /usr/local/bin/tarn-lsp

ENTRYPOINT ["tarn"]
CMD ["--help"]

FROM lukemathwalker/cargo-chef:latest-rust-1.98.0-alpine AS chef
ARG APP_NAME=lightpolars
WORKDIR /build
RUN apk add --no-cache python3
ENV RUSTFLAGS="-C target-cpu=x86-64-v3"

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build

COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --release --bin cdp-server

FROM gcr.io/distroless/cc-debian13 AS runtime

COPY --from=build /build/target/release/cdp-server ./
CMD [ "./cdp-server" ]
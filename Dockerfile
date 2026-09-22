FROM lukemathwalker/cargo-chef:latest-rust-1.98.0-alpine AS chef
ARG APP_NAME=lightpolars
WORKDIR /build
RUN apk add --no-cache python3

# A linha de base da ISA depende da arquitetura, então não pode ser um ENV fixo.
# Em x86-64, v3 (AVX2/BMI2, Haswell em diante) é o piso portável; em aarch64 não
# existe um equivalente com a mesma aceitação, e `generic` é o armv8-a que toda
# máquina ARM de servidor e todo Apple Silicon executam.
#
# `native` está fora de questão aqui: assaria a ISA de quem construiu, e o
# runner do CI não é o host de deploy — seria um SIGILL esperando acontecer.
#
# TARGETARCH vem do buildx. Num `docker build` sem buildx ele chega vazio, e o
# arquivo sai sem flag nenhuma, o que constrói na linha de base da arquitetura.
ARG TARGETARCH
RUN case "$TARGETARCH" in \
      amd64) echo '-C target-cpu=x86-64-v3' > /rustflags ;; \
      arm64) echo '-C target-cpu=generic'   > /rustflags ;; \
      *)     echo ''                        > /rustflags ;; \
    esac

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build

COPY --from=planner /build/recipe.json recipe.json
RUN RUSTFLAGS="$(cat /rustflags)" cargo chef cook --release --recipe-path recipe.json

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN RUSTFLAGS="$(cat /rustflags)" cargo build --release --bin cdp-server

FROM gcr.io/distroless/cc-debian13 AS runtime

COPY --from=build /build/target/release/cdp-server ./
CMD [ "./cdp-server" ]

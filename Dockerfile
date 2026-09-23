# Build em glibc, não musl: o fontique abre a libfontconfig com dlopen, e um
# binário musl estático não consegue fazer dlopen. Construído na alpine, o
# servidor sobe sem fonte de sistema nenhuma, mesmo com fontes instaladas.
FROM lukemathwalker/cargo-chef:latest-rust-1.98.0 AS chef
ARG APP_NAME=lightpolars
WORKDIR /build
RUN apt-get update \
 && apt-get install -y --no-install-recommends python3 \
 && rm -rf /var/lib/apt/lists/*

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

# O runtime precisa de fontes do sistema. O fontique (via parley/blitz) acha as
# fontes no Linux abrindo a libfontconfig com dlopen; sem ela, ou sem fonte
# nenhuma instalada, todo texto que não vem de um @font-face embutido some do
# PDF. A distroless não tem nem uma nem outra, por isso a base é a debian slim.
#
# fonts-liberation: métricas iguais às de Arial, Times New Roman e Courier New,
# e o fontconfig já mapeia esses nomes para ela.
# fonts-dejavu-core: cobertura ampla de Unicode, inclusive símbolos como ⚠.
FROM debian:trixie-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends fontconfig fonts-liberation fonts-dejavu-core \
 && rm -rf /var/lib/apt/lists/* \
 && fc-cache -f

COPY --from=build /build/target/release/cdp-server ./
CMD [ "./cdp-server" ]

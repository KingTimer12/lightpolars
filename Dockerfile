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
# PDF. A distroless não tem nem uma nem outra, então elas são montadas aqui e
# copiadas para lá: a base debian inteira custaria ~110 MB só por isso.
#
# fonts-dejavu-core: cobertura ampla de Unicode, inclusive símbolos como ⚠. É o
# que atende `sans-serif`, `serif` e também `Arial`: o fontique não segue os
# aliases do fontconfig, então uma fonte métrica-compatível (Liberation) não
# seria escolhida no lugar de Arial de qualquer jeito.
FROM debian:trixie-slim AS fonts
RUN apt-get update \
 && apt-get install -y --no-install-recommends fontconfig fonts-dejavu-core \
 && rm -rf /var/lib/apt/lists/* \
 && fc-cache -f
# A libfontconfig e as dependências dela, exceto o que a distroless cc já traz
# (glibc e libgcc). O `cp -L` grava cada uma pelo soname, que é o nome que o
# dlopen e o loader procuram.
RUN mkdir -p /out/lib \
 && lib="$(ls /usr/lib/*-linux-gnu/libfontconfig.so.1)" \
 && { echo "$lib"; ldd "$lib" | awk '/=> \//{print $3}'; } \
    | grep -vE '/(libc|libm|ld-linux[^/]*|libgcc_s)\.so' \
    | xargs -I{} cp -L {} /out/lib/

FROM gcr.io/distroless/cc-debian13 AS runtime
# /usr/lib está no caminho padrão do loader em qualquer arquitetura, então não
# é preciso saber o triplet aqui.
COPY --from=fonts /out/lib/ /usr/lib/
COPY --from=fonts /etc/fonts /etc/fonts
COPY --from=fonts /usr/share/fontconfig /usr/share/fontconfig
COPY --from=fonts /usr/share/xml/fontconfig /usr/share/xml/fontconfig
COPY --from=fonts /usr/share/fonts /usr/share/fonts
COPY --from=fonts /var/cache/fontconfig /var/cache/fontconfig

# Sem isto a memória fica presa no maior pico de concorrência já atendido. O
# mimalloc só devolve páginas livres ao SO depois de um atraso, e a checagem
# roda na thread dona quando ela volta a alocar; worker do tokio ocioso nunca
# volta, então o que ele liberou fica com o processo. Com 6 sessões paralelas,
# o heap parava em ~150 MB depois do pico; com PURGE_DELAY=0, ~55 MB, sem
# diferença mensurável de tempo. As duas MALLOC_* valem para o que ainda passa
# pelo malloc do glibc (fontconfig, freetype): menos arenas e trim mais cedo.
ENV MIMALLOC_PURGE_DELAY=0 \
    MALLOC_ARENA_MAX=2 \
    MALLOC_TRIM_THRESHOLD_=131072

# Eventos do ciclo de vida (START, STOP com o sinal, PANIC e UNCLEAN quando a
# execução anterior morreu sem parar limpo). Monte /var/log/lightpolars num
# volume para o histórico sobreviver à recriação do container.
ENV CDP_LOG_FILE=/var/log/lightpolars/eventos.log

COPY --from=build /build/target/release/cdp-server ./
# Sem curl na distroless: o próprio binário chama o /health.
HEALTHCHECK --interval=10s --timeout=3s --retries=3 CMD [ "/cdp-server", "--health" ]
CMD [ "./cdp-server" ]

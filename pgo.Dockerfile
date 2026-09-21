# PGO, optionally followed by BOLT.
#
# Debian rather than Alpine because BOLT is not packaged for Alpine, and not
# from Debian main either: bookworm has no bolt package (only sid ships
# llvm-bolt), so it comes from apt.llvm.org. The runtime stage is glibc-based
# to match.
#
#   docker build -f pgo.Dockerfile .                        # PGO only
#   docker build -f pgo.Dockerfile --build-arg ENABLE_BOLT=1 .
#
# BOLT is off by default because its instrumentation is still buggy on AArch64
# upstream (llvm-project#143472, #55005). On x86-64 it is worth turning on.
FROM rust:1.98-bookworm AS chef

ARG LLVM_VERSION=19
ARG ENABLE_BOLT=0
WORKDIR /build

# llvm-profdata comes from the Rust toolchain, so no LLVM from apt is needed
# for plain PGO — only BOLT pulls the external repository in.
RUN rustup component add llvm-tools-preview
RUN set -eux; \
    if [ "$ENABLE_BOLT" = "1" ]; then \
      apt-get update; \
      apt-get install -y --no-install-recommends ca-certificates wget gnupg; \
      wget -qO /etc/apt/trusted.gpg.d/apt.llvm.org.asc https://apt.llvm.org/llvm-snapshot.gpg.key; \
      echo "deb http://apt.llvm.org/bookworm/ llvm-toolchain-bookworm-${LLVM_VERSION} main" \
        > /etc/apt/sources.list.d/llvm.list; \
      apt-get update; \
      apt-get install -y --no-install-recommends "bolt-${LLVM_VERSION}" "libbolt-${LLVM_VERSION}-dev"; \
      ln -sf "/usr/lib/llvm-${LLVM_VERSION}/bin/llvm-bolt" /usr/local/bin/llvm-bolt; \
      ln -sf "/usr/lib/llvm-${LLVM_VERSION}/bin/merge-fdata" /usr/local/bin/merge-fdata; \
      rm -rf /var/lib/apt/lists/*; \
    fi
RUN cargo install cargo-pgo cargo-chef

# NOTE: -C target-cpu=native bakes in the *builder's* ISA. Fine when the image
# is built on the same machine family that runs it; it is a SIGILL waiting to
# happen otherwise. Pin an explicit baseline (e.g. x86-64-v3) for portable
# images.
ENV RUSTFLAGS="-C target-cpu=native"

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
ARG ENABLE_BOLT=0
COPY --from=planner /build/recipe.json recipe.json

# Warm the cache for the two profiles actually used below. Cooking --release
# warms nothing for them: cargo keys the target directory by profile name.
RUN cargo chef cook --profile pgo-gen --recipe-path recipe.json
RUN cargo chef cook --profile pgo --recipe-path recipe.json

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# 1. Instrumented build. Profile pgo-gen has no LTO, which is what keeps the
#    link inside the builder's memory budget.
RUN cargo pgo instrument build -- --profile pgo-gen --example workload

# 2. Collect profiles. The workload drives the engine in process, so no server,
#    no socket and no Node are needed here.
RUN ./target/*/pgo-gen/examples/workload 20

# 3. Optimized build. With BOLT: instrument, re-run the workload, then apply.
RUN set -eux; \
    if [ "$ENABLE_BOLT" = "1" ]; then \
      cargo pgo bolt build --with-pgo -- --profile pgo --bin cdp-server --example workload; \
      ./target/*/pgo/examples/workload-bolt-instrumented 20; \
      cargo pgo bolt optimize --with-pgo -- --profile pgo --bin cdp-server; \
      cp ./target/*/pgo/cdp-server-bolt-optimized /build/cdp-server; \
    else \
      cargo pgo optimize build -- --profile pgo --bin cdp-server; \
      cp ./target/*/pgo/cdp-server /build/cdp-server; \
    fi

FROM gcr.io/distroless/cc-debian13 AS runtime
COPY --from=build /build/cdp-server /cdp-server
CMD ["/cdp-server"]

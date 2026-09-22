#!/usr/bin/env bash
# Orquestra o benchmark: constrói os dois binários, sobe cada um numa porta,
# roda bench.js e derruba tudo no fim.
#
#   tests/bench/rodar.sh                 # normal + pgo + chromium
#   MOTORES=normal,pgo tests/bench/rodar.sh
#   PULAR_BUILD=1 tests/bench/rodar.sh   # reaproveita binários já construídos
#
# O binário com PGO precisa de cargo-pgo (`cargo install cargo-pgo`) e do
# componente llvm-tools-preview. Sem eles, o motor "pgo" é pulado em vez de
# quebrar a rodada.
set -euo pipefail

raiz="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$raiz"

PORTA_NORMAL="${PORTA_NORMAL:-9222}"
PORTA_PGO="${PORTA_PGO:-9223}"
MOTORES="${MOTORES:-normal,pgo,chromium}"
export RUSTFLAGS="${RUSTFLAGS:--C target-cpu=native}"

pids=()
limpar() {
  for pid in "${pids[@]:-}"; do
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
}
trap limpar EXIT

quer() { [[ ",$MOTORES," == *",$1,"* ]]; }

# Sobe um binário e espera a porta aceitar conexão antes de seguir; sem isso o
# bench conecta cedo demais e reporta o motor como indisponível.
subir() {
  local binario="$1" porta="$2" nome="$3"
  CDP_ADDR="127.0.0.1:$porta" "$binario" >"/tmp/lightpolars-$nome.log" 2>&1 &
  local pid=$!
  pids+=("$pid")
  # O bench precisa do PID para medir CPU e RAM; sem ele só mede tempo.
  # `tr` em vez de `${nome^^}` porque o bash do macOS ainda é o 3.2.
  local variavel="PID_$(printf '%s' "$nome" | tr '[:lower:]' '[:upper:]')"
  export "$variavel=$pid"
  for _ in $(seq 1 50); do
    if nc -z 127.0.0.1 "$porta" 2>/dev/null; then
      echo "$nome no ar em ws://127.0.0.1:$porta"
      return 0
    fi
    sleep 0.2
  done
  echo "FALHA: $nome não subiu na porta $porta (veja /tmp/lightpolars-$nome.log)" >&2
  return 1
}

motores_ok=()

if quer normal; then
  [[ -n "${PULAR_BUILD:-}" ]] || cargo build --release --bin cdp-server
  subir target/release/cdp-server "$PORTA_NORMAL" normal
  motores_ok+=(normal)
fi

if quer pgo; then
  if ! command -v cargo-pgo >/dev/null; then
    echo "aviso: cargo-pgo não instalado, pulando o motor pgo" >&2
  else
    if [[ -z "${PULAR_BUILD:-}" ]]; then
      # Mesma sequência do pgo.Dockerfile: instrumenta, roda o workload para
      # colher os perfis, reconstrói otimizado.
      cargo pgo instrument build -- --profile pgo-gen --example workload
      "$(ls -d target/*/pgo-gen/examples/workload | head -1)" 20
      cargo pgo optimize build -- --profile pgo --bin cdp-server
    fi
    binario_pgo="$(ls -d target/*/pgo/cdp-server | head -1)"
    subir "$binario_pgo" "$PORTA_PGO" pgo
    motores_ok+=(pgo)
  fi
fi

quer chromium && motores_ok+=(chromium)

MOTORES="$(IFS=,; echo "${motores_ok[*]}")" \
WS_NORMAL="ws://127.0.0.1:$PORTA_NORMAL" \
WS_PGO="ws://127.0.0.1:$PORTA_PGO" \
  node tests/bench/bench.js

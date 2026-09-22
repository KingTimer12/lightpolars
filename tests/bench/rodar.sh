#!/usr/bin/env bash
# Orquestra o benchmark: constrói o binário, sobe numa porta, roda bench.js e
# derruba tudo no fim.
#
#   tests/bench/rodar.sh                 # motor + chromium
#   MOTORES=motor tests/bench/rodar.sh   # só o motor
#   PULAR_BUILD=1 tests/bench/rodar.sh   # reaproveita o binário já construído
set -euo pipefail

raiz="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$raiz"

# Deliberadamente fora da 9222: a porta padrão do CDP costuma já estar tomada
# (o Docker Desktop a publica), e um segundo listener em `*:9222` por IPv6
# aceita IPv4 também, então quem atende a conexão vira sorteio.
PORTA_MOTOR="${PORTA_MOTOR:-9345}"
MOTORES="${MOTORES:-motor,chromium}"
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

# Sobe o binário e espera até ele anunciar que está escutando.
#
# A espera é pelo anúncio do próprio processo, e não por um `nc -z` na porta:
# qualquer outro serviço que já esteja segurando a 9222 — o Docker faz isso —
# responderia ao `nc`, o motor teria morrido com "address already in use" e o
# bench mediria o intruso sem que nada acusasse. Já aconteceu.
subir() {
  local binario="$1" porta="$2" nome="$3"
  local log="/tmp/lightpolars-$nome.log"
  : >"$log"
  CDP_ADDR="127.0.0.1:$porta" "$binario" >"$log" 2>&1 &
  local pid=$!
  pids+=("$pid")
  # O bench precisa do PID para medir CPU e RAM; sem ele só mede tempo.
  # `tr` em vez de `${nome^^}` porque o bash do macOS ainda é o 3.2.
  local variavel="PID_$(printf '%s' "$nome" | tr '[:lower:]' '[:upper:]')"
  export "$variavel=$pid"

  for _ in $(seq 1 50); do
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "FALHA: $nome morreu ao subir:" >&2
      sed 's/^/  /' "$log" >&2
      return 1
    fi
    if grep -q "CDP listening" "$log" 2>/dev/null; then
      echo "$nome no ar em ws://127.0.0.1:$porta (pid $pid)"
      return 0
    fi
    sleep 0.2
  done
  echo "FALHA: $nome não anunciou a porta $porta em 10s (veja $log)" >&2
  return 1
}

motores_ok=()

if quer motor; then
  [[ -n "${PULAR_BUILD:-}" ]] || cargo build --release --bin cdp-server
  subir target/release/cdp-server "$PORTA_MOTOR" motor
  motores_ok+=(motor)
fi

quer chromium && motores_ok+=(chromium)

MOTORES="$(IFS=,; echo "${motores_ok[*]}")" \
WS_MOTOR="ws://127.0.0.1:$PORTA_MOTOR" \
  node tests/bench/bench.js

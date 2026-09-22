# Benchmark

Compara dois motores no mesmo cliente e com o mesmo HTML:

- `motor` — o `cdp-server` do perfil `release`
- `chromium` — Chrome/Chromium headless real

O cliente é `puppeteer-core` para os dois, de propósito: assim a diferença
medida vem do motor, não do driver. Os documentos (`documentos.js`) são três
perfis — texto pesado, gráficos e tabelas.

## Rodar

```sh
npm i -D puppeteer-core          # ou aponte NODE_PATH para outro projeto
tests/bench/rodar.sh
```

O script constrói o binário, sobe numa porta, mede e derruba tudo.

Só o motor, sem Chromium:

```sh
MOTORES=motor tests/bench/rodar.sh
```

Contra um Chromium já no ar, sem construir nada:

```sh
PULAR_BUILD=1 CHROME_WS=ws://127.0.0.1:9333 tests/bench/rodar.sh
```

Chamando o medidor direto (servidores já no ar):

```sh
ITERACOES=50 JSON=/tmp/bench.json node tests/bench/bench.js
```

## Variáveis

| Variável | Padrão | Para quê |
|---|---|---|
| `MOTORES` | `motor,chromium` | quais medir |
| `ITERACOES` | `20` | repetições medidas por documento |
| `AQUECIMENTO` | `3` | repetições descartadas antes de medir |
| `PORTA_MOTOR` | `9345` | porta do motor (fora da 9222, que costuma estar tomada) |
| `CHROME_PATH` | detectado | executável do Chromium |
| `CHROME_WS` | — | usa um Chromium já no ar em vez de subir um |
| `PULAR_BUILD` | — | reaproveita o binário já construído |
| `JSON` | — | grava o resultado bruto |
| `PID_MOTOR` / `PID_CHROMIUM` | — | PID do motor, para medir CPU e RAM |

## Ler o resultado

Por documento e motor: `p50`, `p95`, média, a quebra em `setContent`, `pdf` e
`png`, e o consumo de recursos. A coluna `vs` é razão de p50 contra o Chromium
(ou contra o primeiro motor disponível): acima de `1.00x` a linha é mais rápida
que a referência.

Colunas de recurso (`recursos.js` amostra a árvore de processos a cada 100ms,
só durante a janela medida — o aquecimento e a subida do processo ficam fora):

- `cpu ms/doc` — tempo de CPU gasto por documento. É a métrica honesta para
  comparar: independe de quantos núcleos o motor conseguiu ocupar.
- `cpu %` — média de ocupação na janela; acima de 100% é mais de um núcleo.
- `rss pico MB` — soma do RSS da árvore. **Superestima o Chromium**: páginas
  compartilhadas entre os processos são contadas uma vez por processo.
- `proc` — pico de processos na árvore (1 no motor Rust, ~7-8 no Chromium).

Sem o PID do motor, as colunas de recurso saem como `—` e o tempo continua
sendo medido. O `rodar.sh` preenche os PIDs sozinho; só faz falta quando você
sobe os motores por fora.

Cuidados na interpretação:

- `RUSTFLAGS` do `rodar.sh` usa `target-cpu=native`, enquanto a imagem Docker
  fixa `x86-64-v3`. O ganho local é o teto, não o número de produção.
- Chromium e motor não produzem PDFs idênticos; o benchmark mede tempo, não
  fidelidade. Para fidelidade, use `tests/aceitacao/`.
- O Chromium paga o custo de subir o processo uma vez; as iterações medidas já
  excluem isso, mas não excluem o JIT aquecendo — daí o `AQUECIMENTO`.

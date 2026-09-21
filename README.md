# lightpolars

Motor de renderização headless com protocolo CDP, em Rust. Substitui o Chromium
usado hoje pela API consumidora para gerar PDF via `page.pdf()` e imagem via
`page.screenshot()`, sem motor de JavaScript, sem navegação real e sem acesso à
rede — mantendo o Puppeteer
existente sem mudança de código de aplicação, apenas trocando o endpoint
WebSocket.

## Arquitetura

```
crates/render-ir      tipos puros trocados entre os crates: Rect, Glyph, TextRun,
                      BoxItem, ImageItem, DisplayList, conversões de unidade

crates/render-core    layout do HTML via blitz-dom e extração da DisplayList
  extract.rs          percorre a árvore e emite caixas, runs e imagens
  style.rs            lê fundo, borda e cor dos valores computados do stylo
  net.rs              provedor de recursos restrito a data:
  resource.rs         decodificação de data: URI
  svg/raster.rs       usvg + resvg -> RGBA
  svg/inline.rs       <svg> inline -> <img src="data:...">

crates/paginate       aritmética pura de retângulos, sem renderizar nada
  geometry.rs         folha e margens (A4, retrato/paisagem)
  page.rs             uma página e o empilhamento de itens
  slicer.rs           caminho de impressão: corte em folhas, header/footer
  capture.rs          caminho de screenshot: documento inteiro ou recorte
  fonts.rs            tabela de fontes única do documento

crates/pdf-out        emite Vec<Page> como PDF
  draw.rs             itens -> operadores do content stream

crates/raster-out     emite uma Page como PNG/JPEG
  canvas.rs           caixas e bitmaps (tiny-skia)
  text.rs             glifos (swash) e composição da máscara
  encode.rs           pixmap -> bytes

crates/cdp-server     binário: servidor WebSocket do subconjunto do CDP
  session/wire.rs     formato das mensagens: Command, Output, sessionId
  session/page.rs     estado de uma página aberta (html, viewport, escala)
  session/streams.rs  streams do ReturnAsStream, drenados por IO.read
  session/mod.rs      estado + tabela de roteamento
  handlers/           um módulo por grupo de métodos: target, page, print,
                      screenshot, runtime
  print_params.rs     parâmetros do printToPDF -> PageGeometry
```

`render-ir` existe para `paginate` não depender do blitz (via `render-core`) só
para nomear tipos — mantém a fronteira entre layout e paginação.

## Rodando

```bash
cargo run -p cdp-server                      # escuta em ws://127.0.0.1:9222
CDP_ADDR=127.0.0.1:9333 cargo run -p cdp-server   # outra porta
CDP_DEBUG=1 cargo run -p cdp-server           # espelha o tráfego CDP no stderr
```

Aponte o Puppeteer existente para o endpoint:

```js
const browser = await puppeteer.connect({ browserWSEndpoint: 'ws://127.0.0.1:9222' })
const page = await browser.newPage()
await page.setContent(html)
const pdf = await page.pdf({ format: 'A4', printBackground: true, margin: {...} })
const png = await page.screenshot({ fullPage: true })
```

## Invariante de segurança

`render-core` resolve **exclusivamente** `data:` URI. Qualquer outro esquema
(`file:`, `http:`, `https:`, protocolo-relativo) resolve como recurso ausente e
nunca abre socket ou arquivo — o HTML processado é gerado por IA e não é
confiável. Vale tanto para `<img>` quanto para folhas de estilo e fontes.

## O que funciona

- Layout de HTML/CSS via `blitz-dom`, com texto (parley/fontique), imagens
  `data:` (PNG/JPEG/WebP) e caixas com fundo/borda.
- SVG, tanto em `<img src="data:image/svg+xml;…">` quanto escrito inline no HTML
  (o `<svg>` inline é reescrito como `<img>` antes do parse, porque o blitz-dom
  só entende SVG que chega como recurso). É rasterizado a 3x o tamanho da caixa
  (teto de 4096px por lado) e a transparência vira `/SMask` no PDF.
- Paginação A4 retrato/paisagem, margens assimétricas, header/footer repetido
  por página, sem partir caixa ou imagem ao meio.
- PDF com texto vetorial selecionável (glifos posicionados, fonte embutida,
  ToUnicode) e imagens embutidas como XObject.
- Screenshot em PNG e JPEG via `Page.captureScreenshot`, com `page.screenshot()`,
  `{ fullPage: true }`, `clip` (incluindo `scale`) e `deviceScaleFactor` do
  `page.setViewport()`. `Page.getLayoutMetrics` reporta a altura real do
  documento, que é o que o Puppeteer usa para montar o clip de página inteira.
- Handshake CDP completo para o fluxo real do Puppeteer 25: `Target.setAutoAttach`
  (sem `attachToTarget` explícito), contextos de execução, `Page.printToPDF` com
  `transferMode: ReturnAsStream` via `IO.read`/`IO.close`, e páginas múltiplas em
  sequência com sessão/frame próprios.

## O que não funciona ainda

- SVG sai rasterizado, não vetorial: ampliar muito o PDF mostra o bitmap.
- Motor de JavaScript: qualquer `Runtime.evaluate`/`callFunctionOn` devolve
  `undefined`. Suficiente para `document.fonts.ready`, não para scripts reais.
- Screenshot em WebP: o CDP aceita o formato, mas aqui ele responde erro
  `-32000` em vez de devolver um PNG com o rótulo errado.
- No screenshot o texto é rasterizado, não selecionável — é uma imagem. Para
  texto selecionável, use `page.pdf()`.

## Testes

```bash
cargo test --workspace          # 112 testes de unidade/integração
```

Aceitação ponta a ponta com Puppeteer real, em `tests/aceitacao/`:

```bash
# Sem fixtures nem golden — fecha o handshake completo (connect, newPage,
# setContent, page.pdf):
NODE_PATH=/caminho/para/node_modules/com/puppeteer-core \
  node tests/aceitacao/e2e_puppeteer.js

# Fala CDP cru pelo WebSocket nativo do Node, sem puppeteer:
node tests/aceitacao/smoke.js

# Comparação estrutural com goldens do Chromium — exige HTMLs reais dos fluxos
# de page.pdf (não versionados, veja tests/aceitacao/README.md):
node tests/aceitacao/gerar_goldens.js
node tests/aceitacao/rodar.js
```

## Documentação

- Spec: `docs/superpowers/specs/2026-09-17-motor-render-cdp-design.md`
- Plano de implementação: `docs/superpowers/plans/2026-09-17-motor-render-pdf.md`

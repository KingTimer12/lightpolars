# Motor de renderização headless com protocolo CDP

Data: 2026-09-17
Status: aprovado para planejamento

## Objetivo

Substituir o Chromium headless usado hoje pela API consumidora para geração de PDF
e PNG, mantendo o Puppeteer existente sem mudança de código de aplicação — apenas
a troca do endpoint WebSocket do CDP.

Fora de escopo: motor de JavaScript, navegação real, stack de rede, e o fluxo de
scraping de `controllers/comissao/_materia.js` (`page.goto` + `page.evaluate`),
que permanece no Chromium ou no Lightpanda.

## Consumidores reais

Levantado no código da API consumidora (puppeteer-core 25.11.0). Cinco chamadas de
`setContent`, quatro de `page.pdf`, uma de `screenshot`.

| Call site | Saída | Particularidades |
|---|---|---|
| `helpers/_pdf.js` | PDF A4 | `displayHeaderFooter`, `headerTemplate` com logo, margens `51mm/15mm/20mm/15mm`, `waitUntil: 'load'`, HTML não confiável |
| `controllers/rh/_funcionarios.js` | PDF A4 | **`landscape: true`**, margens `20mm/15mm/20mm/15mm`, **`waitUntil: 'networkidle0'`** |
| `controllers/ged/services/_documento.js` | PDF A4 | margens `0cm/2cm/0cm/2cm`, sem header |
| `controllers/ged/_recibos.js` | PDF A4 | margens `0.1cm/1cm/1cm/1cm` |
| `controllers/cerimonial/_agendamentos.js` | PNG | viewport 1080x1350 dSF 1, `setJavaScriptEnabled(false)` |

Todos usam `printBackground: true`. O conteúdo dos PDFs vem de um editor rico
(TinyMCE), então o subconjunto HTML é o que esse editor emite: `p`, `h1`-`h6`,
`strong`/`em`, `ul`/`ol`/`li`, `table`, `img`, `a`, `blockquote`, `span` com
estilo inline, `text-align`, `hr`.

A troca de endpoint já está preparada: `helpers/_puppeteerService.js` tem o
`puppeteer.connect({ browserWSEndpoint })` comentado acima do `launch()`.

## Requisito de segurança

Em `helpers/_pdf.js` e `controllers/cerimonial/_agendamentos.js` o HTML é tratado
como não confiável — em `_pdf.js` ele é gerado por IA. Hoje a proteção contra SSRF
e leitura de arquivo local vem de um handler `page.on('request')` no lado do Node,
que aborta tudo que não seja `data:` ou `about:blank`.

**Esse handler deixa de ter efeito quando o alvo do CDP muda.** O motor precisa
impor a mesma restrição internamente: `render-core` resolve exclusivamente
`data:` URI; qualquer outro esquema (`file:`, `http:`, `https:`) resolve como
recurso vazio, sem emitir requisição. Isso é invariante do motor, não configuração,
e é o item que não pode regredir em nenhuma etapa.

## Decisão de arquitetura

O motor é construído **sobre o `blitz`** (`blitz-dom` 0.2.4 + `blitz-html` 0.2.0),
que já integra `stylo` (CSS do Firefox/Servo), `taffy` 0.9 (block/flex/grid),
`parley` 0.11 (shaping) e `usvg`. Cascade, herança, seletores, block/inline,
justify e quebra de linha vêm prontos e corretos.

Rejeitado: escrever DOM, CSS, layout e shaping do zero. O ponto onde projetos
assim encalham é o block/inline formatting context, e ele não é diferencial aqui.

### Crates

```
cdp-server/    WebSocket + roteamento CDP. Não conhece renderização.
render-core/   HTML -> documento blitz com layout resolvido.
               Única crate que toca blitz. Resolve data: URI (e só data:).
paginate/      Layout -> Vec<Page>. Corte A4, margens, portrait/landscape,
               posicionamento de header/footer. Aritmética pura, sem render.
pdf-out/       Vec<Page> -> PDF. Glifos como texto vetorial, fontes embutidas.
png-out/       Documento -> PNG.
```

`paginate` recebe uma estrutura própria (caixas + runs de texto posicionados),
não tipos do blitz. Isso mantém a troca do miolo local a `render-core`.

## Pipeline de PDF

Layout único e corte, decidido contra a alternativa de re-layout por página:
dá-se layout ao documento numa largura fixa (folha menos margens laterais),
toma-se a altura total e corta-se em faixas de altura de página útil. O conteúdo
do TinyMCE é fluxo linear, onde isso se comporta bem.

O corte é ajustado para o limite de caixa anterior mais próximo, evitando partir
uma linha de texto ao meio — um `break-inside: avoid` pobre, suficiente para o
conteúdo real.

`headerTemplate` e `footerTemplate` são documentos blitz independentes, com layout
próprio, pintados na faixa de margem de cada página. Nunca entram no fluxo do
conteúdo — daí `margin.top: 51mm` em `_pdf.js` precisar comportar a logo.

### Texto selecionável

Verificado por spike e corrigido na implementação. `blitz-dom` expõe o
`parley::Layout` por elemento em `ElementData::inline_layout_data`, e o caminho
até os glifos posicionados é `Layout::lines() -> Line::items()`, filtrando
`PositionedLayoutItem::GlyphRun`, que dá `baseline()`, `offset()`, `glyphs()`
(id + x/y) e `run()` (com `font()` = blob + índice, e `font_size()`).

Atenção à versão: `blitz-dom` 0.2.4 fixa **parley 0.6**, não 0.11. Declarar 0.11
duplica a crate e os tipos de `inline_layout_data` deixam de casar. O acesso ao
estilo computado exige somar `style = { package = "stylo", version = "0.8" }`,
porque `blitz-dom` não reexporta `ComputedValues`.

## Superfície CDP

- `Target`: `createTarget`, `attachToTarget`, `closeTarget`, `getTargets`, `setDiscoverTargets`
- `Page`: `enable`, `setDocumentContent`, `printToPDF`, `captureScreenshot`
- `Page.lifecycleEvent`: emitir `init`, `load` e `networkIdle`
- `Emulation`: `setDeviceMetricsOverride`, `setScriptExecutionDisabled`
- `Network.enable` / `Fetch.enable`: aceitos, nunca emitem requisição

Puppeteer 25.11 envia `Page.setDocumentContent` diretamente, sem passar por
`evaluate` — verificado em `lib/puppeteer/cdp/Frame.js`. É o que sustenta a
ausência de motor de JS.

Depois do `setDocumentContent`, o Puppeteer bloqueia num `LifecycleWatcher`
esperando os eventos de ciclo de vida. Sem a emissão deles o `setContent` trava
até o timeout — inclusive o `networkidle0` de `rh/_funcionarios.js`.

## Limitações aceitas

Verificadas no código do blitz durante o spike:

- **`border-collapse` não é implementado** — células ficam com 1px de deslocamento
  em vez de bordas colapsadas. O template de `_documento.js` usa `collapse`.
- **`rowspan` não é lido** (só `colspan`). Assume-se que o TinyMCE em uso produz
  grades simples; se aparecerem células mescladas no conteúdo real, vira trabalho
  próprio sobre o blitz.
- Larguras de coluna saem quase uniformes, não dimensionadas por conteúdo como o
  auto table layout do Chromium.
- Fidelidade de `text-align: justify` do parley vs Chromium não foi medida.

## Testes

Aceitação: os quatro fluxos de PDF e o de PNG, com os HTMLs reais, comparados
contra golden files gerados uma vez pelo `launch()` atual. Diff perceptual com
tolerância, não byte a byte.

Unidade: `paginate` testada sem renderizar (corte, margens, landscape, faixa de
header); `render-core` testada como no spike — asserções sobre retângulos de
layout e contagem de glifos.

## Ordem de implementação

1. `render-core`: HTML -> layout, com a restrição `data:`-only.
2. `paginate`: corte A4 portrait e landscape, margens, header/footer.
3. `pdf-out`: glifos -> PDF com texto selecionável e fontes embutidas.
4. `cdp-server`: handshake, lifecycle events, os quatro fluxos de PDF.
5. `png-out` e o fluxo de screenshot.

Os quatro fluxos de PDF entram em produção antes do PNG.

## Pendências

- HTML real do fluxo de screenshot (`_agendamentos.js`) não foi coletado; o
  subconjunto CSS do Caso PNG (flex, grid, SVG inline, `@font-face` WOFF2,
  `background: url() center/cover`) continua não verificado contra o blitz.
- Confirmar se as tabelas do TinyMCE em uso empregam células mescladas.

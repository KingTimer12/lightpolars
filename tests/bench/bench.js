// Benchmark comparativo: o motor e o Chromium, medidos pelo mesmo cliente
// (puppeteer-core) e com o mesmo HTML. O cliente é igual de propósito: a
// diferença medida tem que vir do motor, não do driver.
//
// Uso (o puppeteer-core pode morar em outro projeto):
//   NODE_PATH=/caminho/para/api/node_modules node tests/bench/bench.js
//
// Variáveis:
//   WS_MOTOR    ws:// do cdp-server             (padrão ws://127.0.0.1:9345)
//   CHROME_WS   ws:// de um Chromium já no ar   (senão sobe um com CHROME_PATH)
//   CHROME_PATH executável do Chrome/Chromium
//   ITERACOES   repetições medidas por documento (padrão 20)
//   AQUECIMENTO repetições descartadas antes de medir (padrão 3)
//   MOTORES     lista separada por vírgula: motor,chromium
//   JSON        caminho para gravar o resultado bruto
//   PID_MOTOR / PID_CHROMIUM
//               PID do processo do motor, para medir CPU e RAM. Só faz falta
//               quando o motor foi subido por fora (o `rodar.sh` preenche, e o
//               Chromium lançado aqui é descoberto sozinho). Sem o PID, o tempo
//               continua sendo medido e as colunas de recurso saem vazias.
//
// Um motor que não responder é reportado como indisponível e os demais seguem —
// medir só o motor, sem Chromium, é um uso legítimo.
const fs = require('node:fs')
const puppeteer = require('puppeteer-core')
const documentos = require('./documentos')
const { amostrar } = require('./recursos')

const ITERACOES = Number(process.env.ITERACOES ?? 20)
const AQUECIMENTO = Number(process.env.AQUECIMENTO ?? 3)

const OPCOES_PDF = {
  format: 'A4',
  printBackground: true,
  margin: { top: '20mm', right: '15mm', bottom: '20mm', left: '15mm' },
}

async function conectar(motor) {
  if (motor === 'chromium' && !process.env.CHROME_WS) {
    const executablePath = process.env.CHROME_PATH ?? caminhoPadraoDoChrome()
    const browser = await puppeteer.launch({
      executablePath,
      headless: 'shell',
      args: ['--no-sandbox', '--disable-dev-shm-usage'],
    })
    return { browser, pid: browser.process()?.pid, fechar: () => browser.close() }
  }

  const browserWSEndpoint = {
    motor: process.env.WS_MOTOR ?? 'ws://127.0.0.1:9345',
    chromium: process.env.CHROME_WS,
  }[motor]

  const browser = await puppeteer.connect({ browserWSEndpoint, protocolTimeout: 60000 })
  const pid = Number(process.env[`PID_${motor.toUpperCase()}`]) || null
  return { browser, pid, fechar: () => browser.disconnect() }
}

function caminhoPadraoDoChrome() {
  const candidatos = [
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/Applications/Chromium.app/Contents/MacOS/Chromium',
    '/usr/bin/google-chrome',
    '/usr/bin/chromium',
    '/usr/bin/chromium-browser',
  ]
  const achado = candidatos.find((c) => fs.existsSync(c))
  if (!achado) {
    throw new Error('Chromium não encontrado: defina CHROME_PATH ou CHROME_WS')
  }
  return achado
}

// Uma página nova por iteração: reaproveitar página deixaria cache de fonte e de
// layout do Chromium contar a favor dele, e o motor cria sessão limpa a cada
// documento de qualquer jeito.
async function umaRodada(browser, html) {
  const page = await browser.newPage()
  try {
    const t0 = process.hrtime.bigint()
    await page.setContent(html, { waitUntil: 'load' })
    const t1 = process.hrtime.bigint()
    const pdf = Buffer.from(await page.pdf(OPCOES_PDF))
    const t2 = process.hrtime.bigint()

    await page.setViewport({ width: 800, height: 600 })
    const png = Buffer.from(await page.screenshot({ fullPage: true }))
    const t3 = process.hrtime.bigint()

    if (!pdf.subarray(0, 5).equals(Buffer.from('%PDF-'))) {
      throw new Error('saída não é um PDF')
    }
    if (!png.subarray(0, 8).equals(Buffer.from('89504e470d0a1a0a', 'hex'))) {
      throw new Error('saída não é um PNG')
    }

    const ms = (a, b) => Number(b - a) / 1e6
    return {
      conteudo: ms(t0, t1),
      pdf: ms(t1, t2),
      png: ms(t2, t3),
      total: ms(t0, t3),
      bytesPdf: pdf.length,
      bytesPng: png.length,
    }
  } finally {
    await page.close()
  }
}

function percentil(valores, p) {
  const ordenado = [...valores].sort((a, b) => a - b)
  const i = Math.min(ordenado.length - 1, Math.ceil((p / 100) * ordenado.length) - 1)
  return ordenado[i]
}

function resumir(amostras) {
  const totais = amostras.map((a) => a.total)
  const soma = (f) => amostras.reduce((acc, a) => acc + f(a), 0)
  return {
    n: amostras.length,
    min: Math.min(...totais),
    p50: percentil(totais, 50),
    p95: percentil(totais, 95),
    max: Math.max(...totais),
    media: soma((a) => a.total) / amostras.length,
    conteudo: soma((a) => a.conteudo) / amostras.length,
    pdfMs: soma((a) => a.pdf) / amostras.length,
    pngMs: soma((a) => a.png) / amostras.length,
    bytesPdf: Math.round(soma((a) => a.bytesPdf) / amostras.length),
    bytesPng: Math.round(soma((a) => a.bytesPng) / amostras.length),
  }
}

async function medirMotor(motor) {
  let conexao
  try {
    conexao = await conectar(motor)
  } catch (e) {
    console.error(`  ${motor}: indisponível (${e.message})`)
    return null
  }

  const porDocumento = {}
  try {
    for (const doc of documentos) {
      for (let i = 0; i < AQUECIMENTO; i++) {
        await umaRodada(conexao.browser, doc.html)
      }
      // O amostrador só cobre a janela medida: o aquecimento acima já saiu, e
      // com ele o custo de subida do processo, que senão apareceria como CPU
      // do primeiro documento.
      const monitor = amostrar(conexao.pid)
      const amostras = []
      for (let i = 0; i < ITERACOES; i++) {
        amostras.push(await umaRodada(conexao.browser, doc.html))
      }
      const recursos = await monitor.parar()

      porDocumento[doc.nome] = { ...resumir(amostras), recursos }
      const r = porDocumento[doc.nome]
      const custo = recursos
        ? `  cpu ${recursos.cpuPct.toFixed(0)}%  rss ${recursos.picoRssMb.toFixed(0)}MB`
        : ''
      console.error(
        `  ${motor}/${doc.nome}: p50 ${r.p50.toFixed(1)}ms  p95 ${r.p95.toFixed(1)}ms${custo}`
      )
    }
  } catch (e) {
    console.error(`  ${motor}: falhou durante a medição (${e.message})`)
    return Object.keys(porDocumento).length ? porDocumento : null
  } finally {
    await conexao.fechar().catch(() => {})
  }
  return porDocumento
}

function tabela(resultados) {
  const motores = Object.keys(resultados)
  const base = motores.includes('chromium') ? 'chromium' : motores[0]
  const linhas = [
    [
      'documento',
      'motor',
      'p50 ms',
      'p95 ms',
      'média',
      'setContent',
      'pdf',
      'png',
      'cpu ms/doc',
      'cpu %',
      'rss pico MB',
      'proc',
      `vs ${base}`,
    ],
  ]

  for (const doc of documentos) {
    for (const motor of motores) {
      const r = resultados[motor]?.[doc.nome]
      if (!r) continue
      const referencia = resultados[base]?.[doc.nome]
      const razao =
        referencia && motor !== base ? `${(referencia.p50 / r.p50).toFixed(2)}x` : '—'
      const c = r.recursos
      linhas.push([
        doc.nome,
        motor,
        r.p50.toFixed(1),
        r.p95.toFixed(1),
        r.media.toFixed(1),
        r.conteudo.toFixed(1),
        r.pdfMs.toFixed(1),
        r.pngMs.toFixed(1),
        c ? ((c.cpuSeg * 1000) / r.n).toFixed(1) : '—',
        c ? c.cpuPct.toFixed(0) : '—',
        c ? c.picoRssMb.toFixed(0) : '—',
        c ? String(c.picoProcessos) : '—',
        razao,
      ])
    }
  }

  const larguras = linhas[0].map((_, c) => Math.max(...linhas.map((l) => String(l[c]).length)))
  return linhas
    .map((l) => l.map((celula, c) => String(celula).padEnd(larguras[c])).join('  '))
    .join('\n')
}

;(async () => {
  const motores = (process.env.MOTORES ?? 'motor,chromium').split(',').map((m) => m.trim())
  console.error(
    `bench: ${ITERACOES} iterações (+${AQUECIMENTO} de aquecimento) por documento\n`
  )

  const resultados = {}
  for (const motor of motores) {
    const medido = await medirMotor(motor)
    if (medido) resultados[motor] = medido
  }

  if (!Object.keys(resultados).length) {
    console.error('FALHA: nenhum motor respondeu')
    process.exit(1)
  }

  console.log(`\n${tabela(resultados)}`)
  console.log(
    '\n(“vs” é razão de p50: acima de 1.00x o motor da linha é mais rápido)\n' +
      '(“cpu ms/doc” é tempo de CPU por documento, somando a árvore de processos;\n' +
      ' “cpu %” acima de 100 significa mais de um núcleo ocupado em média;\n' +
      ' “rss pico” soma o RSS da árvore, então conta memória compartilhada mais de uma vez)'
  )

  if (process.env.JSON) {
    fs.writeFileSync(
      process.env.JSON,
      JSON.stringify({ iteracoes: ITERACOES, quando: new Date().toISOString(), resultados }, null, 2)
    )
    console.error(`\nresultado bruto em ${process.env.JSON}`)
  }
})().catch((e) => {
  console.error(`FALHA: ${e.constructor.name}: ${e.message}`)
  process.exit(1)
})

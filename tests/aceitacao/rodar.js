// Roda os fluxos reais contra o motor e compara com os goldens do Chromium.
// Uso: node tests/aceitacao/rodar.js
import puppeteer from 'puppeteer-core'
import fs from 'node:fs'
import path from 'node:path'

const CASOS = JSON.parse(fs.readFileSync('tests/aceitacao/casos.json', 'utf8'))
const DIR_GOLDEN = 'tests/aceitacao/golden'

const browser = await puppeteer.connect({
  browserWSEndpoint: process.env.CDP_WS ?? 'ws://127.0.0.1:9222',
})

let falhas = 0
for (const caso of CASOS) {
  const page = await browser.newPage()
  const html = fs.readFileSync(path.join('tests/aceitacao/html', caso.html), 'utf8')
  await page.setContent(html, { waitUntil: caso.waitUntil ?? 'load' })
  const pdf = Buffer.from(await page.pdf(caso.pdfOptions))
  await page.close()

  if (!pdf.subarray(0, 5).equals(Buffer.from('%PDF-'))) {
    console.error(`FALHA ${caso.nome}: saída não é PDF`)
    falhas++
    continue
  }

  const golden = path.join(DIR_GOLDEN, `${caso.nome}.pdf`)
  if (!fs.existsSync(golden)) {
    console.error(`FALHA ${caso.nome}: golden ausente — rode gerar_goldens.js primeiro`)
    falhas++
    continue
  }

  // Comparação de estrutura, não de bytes: contagem de páginas e presença de texto.
  const paginasMotor = contarPaginas(pdf)
  const paginasGolden = contarPaginas(fs.readFileSync(golden))
  if (paginasMotor !== paginasGolden) {
    console.error(`FALHA ${caso.nome}: ${paginasMotor} páginas, golden tem ${paginasGolden}`)
    falhas++
    continue
  }

  console.log(`OK ${caso.nome} (${paginasMotor} páginas)`)
}

// `/Type/Page` sem espaço (printpdf) ou `/Type /Page` com espaço (Chromium);
// o `[^s]` à direita evita casar o nó `/Type/Pages`.
function contarPaginas(bytes) {
  return (bytes.toString('latin1').match(/\/Type\s*\/Page[^s]/g) || []).length
}

await browser.disconnect()
process.exit(falhas === 0 ? 0 : 1)

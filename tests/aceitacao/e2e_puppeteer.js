// Ponta a ponta com o Puppeteer real, sem fixtures nem goldens: conectar, abrir
// página, setContent e imprimir. É o que pega quebra de handshake — o smoke fala
// CDP cru e não exerce o bootstrap de página do Puppeteer.
//
// Uso (o puppeteer-core pode morar em outro projeto):
//   NODE_PATH=/caminho/para/api/node_modules node tests/aceitacao/e2e_puppeteer.js
const puppeteer = require('puppeteer-core')

const alvo = process.env.CDP_WS ?? 'ws://127.0.0.1:9222'

;(async () => {
  const browser = await puppeteer.connect({
    browserWSEndpoint: alvo,
    protocolTimeout: 15000,
  })
  const page = await browser.newPage()
  await page.setContent('<h1>Título</h1><p>Parágrafo de teste.</p>', { waitUntil: 'load' })
  const pdf = Buffer.from(await page.pdf({
    format: 'A4',
    printBackground: true,
    margin: { top: '20mm', right: '15mm', bottom: '20mm', left: '15mm' },
  }))
  await browser.disconnect()

  if (!pdf.subarray(0, 5).equals(Buffer.from('%PDF-'))) {
    console.error('FALHA: page.pdf() não devolveu um PDF')
    process.exit(1)
  }
  const paginas = (pdf.toString('latin1').match(/\/Type\s*\/Page[^s]/g) || []).length
  console.log(`OK e2e: ${pdf.length} bytes, ${paginas} página(s)`)
})().catch((e) => {
  console.error(`FALHA: ${e.constructor.name}: ${e.message}`)
  process.exit(1)
})

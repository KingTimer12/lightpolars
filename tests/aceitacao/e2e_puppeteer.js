// Ponta a ponta com o Puppeteer real, sem fixtures nem goldens: conectar, abrir
// página, setContent, imprimir e fotografar. É o que pega quebra de handshake —
// o smoke fala
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

  // O screenshot passa por outro caminho: getLayoutMetrics, viewport emulado e
  // captureScreenshot. Uma regressão só nele não apareceria no PDF.
  await page.setViewport({ width: 400, height: 300 })
  const png = Buffer.from(await page.screenshot({ fullPage: true }))
  await browser.disconnect()

  if (!pdf.subarray(0, 5).equals(Buffer.from('%PDF-'))) {
    console.error('FALHA: page.pdf() não devolveu um PDF')
    process.exit(1)
  }
  if (!png.subarray(0, 8).equals(Buffer.from('89504e470d0a1a0a', 'hex'))) {
    console.error('FALHA: page.screenshot() não devolveu um PNG')
    process.exit(1)
  }
  const largura = png.readUInt32BE(16)
  if (largura !== 400) {
    console.error(`FALHA: screenshot saiu com ${largura}px de largura, esperava 400`)
    process.exit(1)
  }
  const paginas = (pdf.toString('latin1').match(/\/Type\s*\/Page[^s]/g) || []).length
  console.log(
    `OK e2e: PDF ${pdf.length} bytes, ${paginas} página(s); ` +
      `PNG ${png.length} bytes, ${largura}x${png.readUInt32BE(20)}`
  )
})().catch((e) => {
  console.error(`FALHA: ${e.constructor.name}: ${e.message}`)
  process.exit(1)
})

// Gera os PDFs de referência com o Chromium, para congelar como golden.
// Uso: node tests/aceitacao/gerar_goldens.js
import puppeteer from 'puppeteer'
import fs from 'node:fs'
import path from 'node:path'

const CASOS = JSON.parse(fs.readFileSync('tests/aceitacao/casos.json', 'utf8'))
fs.mkdirSync('tests/aceitacao/golden', { recursive: true })

const browser = await puppeteer.launch({
  headless: true,
  args: ['--no-sandbox', '--disable-setuid-sandbox', '--disable-gpu'],
})

for (const caso of CASOS) {
  const page = await browser.newPage()
  const html = fs.readFileSync(path.join('tests/aceitacao/html', caso.html), 'utf8')
  await page.setContent(html, { waitUntil: caso.waitUntil ?? 'load' })
  const pdf = await page.pdf(caso.pdfOptions)
  fs.writeFileSync(path.join('tests/aceitacao/golden', `${caso.nome}.pdf`), pdf)
  await page.close()
  console.log(`golden gerado: ${caso.nome}`)
}

await browser.close()

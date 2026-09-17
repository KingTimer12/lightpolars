// Smoke do transporte: fala CDP cru pelo WebSocket nativo do Node, sem puppeteer
// e sem fixtures. Separa falha de transporte de falha de layout.
// Uso: CDP_WS=ws://127.0.0.1:9222 node tests/aceitacao/smoke.js
const url = process.env.CDP_WS ?? 'ws://127.0.0.1:9222'
const ws = new WebSocket(url)
const pendentes = new Map()
const eventos = []
const aguardando = new Map()
let proximoId = 0

function enviar(method, params = {}) {
  const id = ++proximoId
  return new Promise((resolve, reject) => {
    pendentes.set(id, { resolve, reject })
    ws.send(JSON.stringify({ id, method, params }))
    setTimeout(() => reject(new Error(`timeout em ${method}`)), 15000)
  })
}

ws.addEventListener('message', (ev) => {
  const msg = JSON.parse(ev.data)
  if (msg.id != null && pendentes.has(msg.id)) {
    pendentes.get(msg.id).resolve(msg.result)
    pendentes.delete(msg.id)
  } else if (msg.method) {
    eventos.push(msg)
    const espera = aguardando.get(msg.method)
    if (espera) {
      aguardando.delete(msg.method)
      espera(msg)
    }
  }
})

// Eventos podem chegar depois da resposta do comando que os dispara; esperar pelo
// evento em si evita corrida no teste.
function aguardarEvento(method) {
  return new Promise((resolve, reject) => {
    if (eventos.some((e) => e.method === method)) return resolve()
    aguardando.set(method, resolve)
    setTimeout(() => reject(new Error(`timeout esperando ${method}`)), 15000)
  })
}

ws.addEventListener('error', () => {
  console.error(`FALHA: não consegui conectar em ${url} — o motor está no ar?`)
  process.exit(1)
})

ws.addEventListener('open', async () => {
  let falhas = 0
  const alvo = await enviar('Target.createTarget', { url: 'about:blank' })
  if (typeof alvo?.targetId !== 'string') {
    console.error('FALHA: Target.createTarget não devolveu targetId')
    falhas++
  }

  const anexo = await enviar('Target.attachToTarget', {
    targetId: alvo.targetId,
    flatten: true,
  })
  if (typeof anexo?.sessionId !== 'string') {
    console.error('FALHA: Target.attachToTarget não devolveu sessionId')
    falhas++
  }
  if (!eventos.some((e) => e.method === 'Target.attachedToTarget')) {
    console.error('FALHA: faltou o evento Target.attachedToTarget')
    falhas++
  }

  await enviar('Page.enable')
  await enviar('Page.setDocumentContent', {
    frameId: 'f1',
    html: '<p>Documento de fumaça</p>',
  })
  await aguardarEvento('Page.lifecycleEvent')

  const ciclo = eventos
    .filter((e) => e.method === 'Page.lifecycleEvent')
    .map((e) => e.params.name)
  for (const nome of ['init', 'load', 'networkIdle']) {
    if (!ciclo.includes(nome)) {
      console.error(`FALHA: faltou o evento de ciclo de vida ${nome} (${ciclo})`)
      falhas++
    }
  }

  const impresso = await enviar('Page.printToPDF', {
    paperWidth: 8.27,
    paperHeight: 11.7,
    marginTop: 0,
    marginRight: 0,
    marginBottom: 0,
    marginLeft: 0,
    printBackground: true,
  })
  const pdf = Buffer.from(impresso.data, 'base64')
  if (!pdf.subarray(0, 5).equals(Buffer.from('%PDF-'))) {
    console.error('FALHA: printToPDF não devolveu um PDF')
    falhas++
  } else {
    const paginas = (pdf.toString('latin1').match(/\/Type\s*\/Page[^s]/g) || []).length
    console.log(`OK transporte: ${pdf.length} bytes de PDF, ${paginas} página(s)`)
  }

  ws.close()
  process.exit(falhas === 0 ? 0 : 1)
})

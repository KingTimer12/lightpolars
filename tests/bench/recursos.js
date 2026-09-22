// Amostrador de CPU e memória da árvore de processos de um motor.
//
// Precisa da árvore inteira, não só do PID raiz: o Chromium espalha o trabalho
// por processos de GPU, renderer e utility, e medir só o browser process daria
// a ele um consumo artificialmente baixo. O motor Rust é um processo só, mas
// passa pelo mesmo caminho para que a comparação seja simétrica.
//
// CPU vem do tempo acumulado que o `ps` reporta (delta entre a primeira e a
// última amostra), então processos que nascem e morrem entre duas amostras são
// perdidos — com intervalo de 100ms isso só afeta processos muito curtos.
const { execFile } = require('node:child_process')

const INTERVALO_MS = 100

function ps() {
  return new Promise((resolve) => {
    execFile('ps', ['-Ao', 'pid=,ppid=,rss=,time='], { maxBuffer: 8 << 20 }, (erro, saida) => {
      if (erro) return resolve([])
      const linhas = saida.split('\n')
      const processos = []
      for (const linha of linhas) {
        const campos = linha.trim().split(/\s+/)
        if (campos.length < 4) continue
        processos.push({
          pid: Number(campos[0]),
          ppid: Number(campos[1]),
          rssKb: Number(campos[2]),
          cpuSeg: segundosDeCpu(campos[3]),
        })
      }
      resolve(processos)
    })
  })
}

// `ps` escreve o tempo de CPU como [[dd-]hh:]mm:ss[.ss], variando entre Linux e
// macOS. Parse tolerante: qualquer campo ausente vale zero.
function segundosDeCpu(texto) {
  const [dias, resto] = texto.includes('-') ? texto.split('-') : ['0', texto]
  const partes = resto.split(':').map(Number)
  while (partes.length < 3) partes.unshift(0)
  const [h, m, s] = partes
  return Number(dias) * 86400 + h * 3600 + m * 60 + s
}

function arvore(processos, raiz) {
  const filhos = new Map()
  for (const p of processos) {
    if (!filhos.has(p.ppid)) filhos.set(p.ppid, [])
    filhos.get(p.ppid).push(p)
  }
  const selecionados = []
  const fila = [raiz]
  const vistos = new Set()
  while (fila.length) {
    const pid = fila.pop()
    if (vistos.has(pid)) continue
    vistos.add(pid)
    const proprio = processos.find((p) => p.pid === pid)
    if (proprio) selecionados.push(proprio)
    for (const filho of filhos.get(pid) ?? []) fila.push(filho.pid)
  }
  return selecionados
}

function agregar(processos, raiz) {
  const nos = arvore(processos, raiz)
  return {
    rssKb: nos.reduce((acc, p) => acc + p.rssKb, 0),
    cpuSeg: nos.reduce((acc, p) => acc + p.cpuSeg, 0),
    processos: nos.length,
  }
}

// Retorna um amostrador já rodando. `parar()` devolve o consumo da janela.
function amostrar(raiz) {
  if (!raiz) return { parar: async () => null }

  let primeira = null
  let ultima = null
  let picoRssKb = 0
  let picoProcessos = 0
  let amostras = 0
  const inicio = process.hrtime.bigint()
  let ativo = true

  const coletar = async () => {
    const agora = agregar(await ps(), raiz)
    if (!ativo) return
    if (!primeira) primeira = agora
    ultima = agora
    picoRssKb = Math.max(picoRssKb, agora.rssKb)
    picoProcessos = Math.max(picoProcessos, agora.processos)
    amostras++
  }

  const timer = setInterval(() => {
    coletar().catch(() => {})
  }, INTERVALO_MS)
  const primeiraColeta = coletar().catch(() => {})

  return {
    async parar() {
      await primeiraColeta
      clearInterval(timer)
      await coletar().catch(() => {})
      ativo = false
      if (!primeira || !ultima) return null
      const segundosDeParede = Number(process.hrtime.bigint() - inicio) / 1e9
      const cpuSeg = Math.max(0, ultima.cpuSeg - primeira.cpuSeg)
      return {
        picoRssMb: picoRssKb / 1024,
        rssFinalMb: ultima.rssKb / 1024,
        cpuSeg,
        // Acima de 100% significa mais de um núcleo ocupado em média.
        cpuPct: segundosDeParede > 0 ? (cpuSeg / segundosDeParede) * 100 : 0,
        picoProcessos,
        amostras,
      }
    },
  }
}

module.exports = { amostrar }

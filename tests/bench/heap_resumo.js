// Resume o `dhat-heap.json` do exemplo `heap`: quem detém os bytes vivos no
// pico do processo.
//
//   cargo run --profile profiling --features dhat-heap --example heap -p cdp-server
//   node tests/bench/heap_resumo.js dhat-heap.json
//
// O campo que interessa é `gb` — bytes ainda vivos no instante de maior uso.
// `tb` (total alocado ao longo da execução) mede tráfego, não ocupação, e é o
// engano fácil de cometer aqui: um buffer reciclado mil vezes aparece enorme em
// `tb` e não custa nada de RSS.
const fs = require('node:fs')

const arquivo = process.argv[2] ?? 'dhat-heap.json'
const dados = JSON.parse(fs.readFileSync(arquivo, 'utf8'))
const quadros = dados.ftbl
const pontos = dados.pps

const total = pontos.reduce((acc, p) => acc + (p.gb ?? 0), 0)

// Agrupa por crate de origem: o primeiro quadro da pilha que não é da std, do
// alocador ou do próprio dhat. É a atribuição que responde "de quem é a
// memória", que é a pergunta em jogo.
// Casa tanto `alloc::vec::...` quanto as formas qualificadas
// (`<T as alloc::slice::...>`, `<u8 as alloc::vec::...>`), que são as que
// escondem o verdadeiro dono da alocação atrás de um `to_vec`.
const IGNORAR =
  /^(<[^>]*\bas\s+(alloc|core|std)::|(alloc|core|std|hashbrown|dhat)::|__rust|_rust|malloc|realloc)/
function dono(pp) {
  for (const indice of pp.fs ?? []) {
    const quadro = quadros[indice] ?? ''
    const nome = quadro.replace(/^0x[0-9a-f]+:\s*/, '')
    if (!IGNORAR.test(nome)) {
      const crate = nome.split(/[:<(]/)[0].trim().split(/\s+/).pop() ?? nome
      return { crate: crate.split('::')[0] || nome, quadro: nome }
    }
  }
  return { crate: '(desconhecido)', quadro: '(sem quadro)' }
}

const porCrate = new Map()
const porQuadro = new Map()
for (const pp of pontos) {
  const bytes = pp.gb ?? 0
  if (!bytes) continue
  const { crate, quadro } = dono(pp)
  porCrate.set(crate, (porCrate.get(crate) ?? 0) + bytes)
  porQuadro.set(quadro, (porQuadro.get(quadro) ?? 0) + bytes)
}

const mb = (b) => (b / 1048576).toFixed(2).padStart(8)
const pct = (b) => `${((b / total) * 100).toFixed(1).padStart(5)}%`

console.log(`\nbytes vivos no pico: ${mb(total)} MB\n`)

console.log('por crate de origem:')
for (const [crate, bytes] of [...porCrate].sort((a, b) => b[1] - a[1]).slice(0, 12)) {
  console.log(`  ${mb(bytes)} MB  ${pct(bytes)}  ${crate}`)
}

console.log('\npor sítio de alocação:')
for (const [quadro, bytes] of [...porQuadro].sort((a, b) => b[1] - a[1]).slice(0, 15)) {
  console.log(`  ${mb(bytes)} MB  ${pct(bytes)}  ${quadro.slice(0, 96)}`)
}

// As pilhas dos maiores blocos isolados: é aí que se vê se o custo é uma cópia
// evitável ou o buffer que o trabalho exige de fato.
console.log('\npilhas dos maiores blocos:')
for (const pp of [...pontos].sort((a, b) => (b.gb ?? 0) - (a.gb ?? 0)).slice(0, 4)) {
  console.log(`\n  ${mb(pp.gb)} MB em ${pp.gbk} bloco(s):`)
  for (const indice of (pp.fs ?? []).slice(0, 12)) {
    console.log(`    ${(quadros[indice] ?? '').replace(/^0x[0-9a-f]+:\s*/, '').slice(0, 104)}`)
  }
}

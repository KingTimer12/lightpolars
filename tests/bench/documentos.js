// Os três perfis de documento do bench, em JS para que o Chromium receba
// exatamente o mesmo HTML que o motor. Qualquer divergência aqui invalida a
// comparação, então mantenha em sincronia com `crates/cdp-server/examples/`,
// que roda os mesmos documentos no processo.

// PNG 1x1 vermelho opaco.
const PNG =
  'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=='

// Muitos parágrafos curtos: o caso que domina relatórios e notas reais, e o que
// gasta o tempo em shaping e paginação.
function textoPesado(paragrafos = 60) {
  let html = "<!DOCTYPE html><html><body style='font-family:sans-serif;font-size:12px'>"
  for (let i = 0; i < paragrafos; i++) {
    html +=
      `<h2 style='color:#334'>Seção ${i}</h2><p>Texto de exemplo com acentuação, ` +
      'números 1234567890 e pontuação — repetido para dar volume ao documento ' +
      'e forçar a paginação a cortar em pontos diferentes.</p>'
  }
  return html + '</body></html>'
}

// SVG inline mais um raster em data URI: exercita o rewrite, o usvg/resvg e os
// decodificadores de imagem.
function comGraficos(blocos = 20) {
  let html = '<!DOCTYPE html><html><body>'
  for (let i = 0; i < blocos; i++) {
    html +=
      "<div style='background:#eef;padding:8px;margin:4px'>" +
      "<svg width='120' height='60' viewBox='0 0 120 60'>" +
      "<rect x='0' y='0' width='120' height='60' fill='#4a90d9'/>" +
      `<circle cx='${20 + i * 4}' cy='30' r='20' fill='#ffcc00' opacity='0.7'/>` +
      "<path d='M10 50 L60 10 L110 50 Z' fill='none' stroke='#fff' stroke-width='3'/>" +
      '</svg>' +
      `<img src='${PNG}' style='width:80px;height:40px'>` +
      `<p>Bloco gráfico ${i}</p></div>`
  }
  return html + '</body></html>'
}

// Tabelas largas: muitas caixas e bordas, ou seja, o caminho de box-drawing e
// não o de texto.
function tabelas(linhas = 40) {
  let html =
    "<!DOCTYPE html><html><body><table style='width:100%;border-collapse:collapse'>"
  for (let r = 0; r < linhas; r++) {
    html += '<tr>'
    for (let c = 0; c < 6; c++) {
      const fundo = r % 2 === 0 ? '#fff' : '#f4f4f4'
      html += `<td style='border:1px solid #999;padding:4px;background:${fundo}'>${r}.${c}</td>`
    }
    html += '</tr>'
  }
  return html + '</table></body></html>'
}

module.exports = [
  { nome: 'texto-pesado', html: textoPesado(60) },
  { nome: 'graficos', html: comGraficos(20) },
  { nome: 'tabelas', html: tabelas(40) },
]

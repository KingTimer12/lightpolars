# Aceitação

Os HTMLs de entrada (`html/`) e os PDFs de referência (`golden/`) não são
versionados: contêm conteúdo identificável do sistema consumidor.

Para preparar a suíte numa máquina nova:

1. Copie os quatro HTMLs reais dos fluxos de `page.pdf` para `html/`, com os nomes
   declarados em `casos.json` (`pdf_ia.html`, `funcionarios.html`,
   `documento.html`, `recibo.html`). Substitua apenas as interpolações de template
   por conteúdo representativo — mantenha o CSS intacto, é ele que está sob teste.
2. Instale as dependências: `npm i -D puppeteer puppeteer-core` na raiz do
   repositório (nenhuma das duas é versionada aqui).
3. Rode `node tests/aceitacao/gerar_goldens.js` a partir da raiz do repositório
   para produzir `golden/` com o Chromium atual.
4. Suba o motor (`cargo run -p cdp-server`) e rode `node tests/aceitacao/rodar.js`.

`smoke.js` não precisa de nada disso: fala CDP cru pelo WebSocket nativo do Node e
verifica que o binário responde e devolve um PDF. Use-o para separar falha de
transporte de falha de layout.

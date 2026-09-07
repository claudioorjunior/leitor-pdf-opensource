# Tsuro

Tsuro abre e lê qualquer PDF sem travar. Completo, neste produto, é ler sem travar. Não é uma suíte.

A marca é um tsuru de três dobras.

![Marca Tsuro](public/tsuro-mark.png)

Ainda não há seleção para anotar, destaque nem notas adesivas. Gravação automática na nuvem fica bem mais tarde.

## Repositório

O canônico fica no Cursor Origin. O GitHub é o espelho público.

Clone pelo Origin (preferido):

```bash
git clone https://origin.cursor.com/claudioorjunior/leitor-pdf-opensource.git
```

Com a CLI do Origin:

```bash
origin repo clone claudioorjunior/leitor-pdf-opensource
```

Clone pelo espelho no GitHub:

```bash
git clone https://github.com/claudioorjunior/leitor-pdf-opensource.git
```

Remotes locais típicos: `origin` (Cursor) e `github` (GitHub). Ao publicar, empurre os dois: `git push origin HEAD` e `git push github HEAD`.

## Assinaturas (`tsuro-sign`)

O reconhecimento e a verificação das assinaturas vivem no crate Rust `tsuro-sign`:

- percorre AcroForm e dicionários `/Sig`
- lê o PKCS#7/CMS (perfil `adbe.pkcs7.detached`)
- confere o `ByteRange` e o `messageDigest`
- verifica RSA + SHA-256 sobre os atributos assinados

## Visor nativo

O visor nativo em construção é o crate `tsuro` (iced + PDFium). Precisa da biblioteca Pdfium no cwd ou no sistema.

```bash
cargo test -p tsuro-sign
cargo run -p tsuro -- public/samples/guia-folio.pdf
```

## Requisitos

- Rust 1.85+ (`rustup default stable`)
- Pdfium no cwd ou no sistema, para o crate `tsuro`
- Node.js 22+ só se você for mexer no legado Tauri

## Legado (Tauri e PDF.js)

A casca [Tauri](https://tauri.app/) e o visor React + [PDF.js](https://mozilla.github.io/pdf.js/) 6 continuam no repositório. Eles não são o produto.

```bash
npm install
npm run test:sign
npm run dev          # visor legado no navegador (http://127.0.0.1:43177)
npm run tauri dev
npm run tauri build
```

No macOS o legado gera `.app` / `.dmg`. No Windows, instalador NSIS / MSI.

## Uso

A janela nativa é o crate `tsuro`. Os controles estão na barra, não em atalhos de teclado.

- **Abrir.** Botão, ou arrastar o arquivo para a janela.
- **Fechar.** Fecha o documento aberto.
- **Anterior / Próxima.** Troca de página. A barra mostra Página N / total.
- **Ajustar à largura.** Encaixa a página na largura da janela.
- **Página.** Encaixa a página inteira na janela.
- **+ / −.** Zoom manual. A barra mostra a porcentagem.
- **Buscar.** Campo na barra. Conta as ocorrências no texto extraído.
- **Assinaturas.** Painel à direita. Signatário e estado criptográfico.

O legado Tauri e PDF.js continua no repositório. Não é o produto. A marca do legado também é Tsuro.

Há dois PDFs de exemplo em `public/samples/`:

| Arquivo | O que testa |
| --- | --- |
| `guia-folio.pdf` | Tipografia, tabela, acentos, busca |
| `contrato-assinado.pdf` | Campo `/Sig` com certificado autoassinado de demonstração |

O certificado do contrato é **autoassinado**. Tsuro trata isso como assinatura criptograficamente íntegra, mas sem cadeia de confiança pública. O estado esperado é “íntegra (sem confiança pública)”.

## Arquitetura

```
crates/tsuro            visor nativo (iced + PDFium)
crates/tsuro-sign       motor Rust (PDF + CMS)
src-tauri               legado Tauri
src                     legado React + PDF.js
public/tsuro-mark.png   marca (tsuru de três dobras)
public/samples          documentos de exemplo
```

O visor web legado funciona sozinho. No binário Tauri, a verificação CMS passa pelo crate Rust via comando Tauri.

## Licença

MIT. Contribuições de leitura, verificação e empacotamento são bem-vindas. Tsuro pretende continuar pequeno.

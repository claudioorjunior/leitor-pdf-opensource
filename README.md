# Tsuro

Leitor de PDF de código aberto. Abre o arquivo, mostra a página, busca o texto e verifica assinaturas digitais.

Não é uma suíte. Completo, neste produto, é **ler sem travar**.

![Marca Tsuro](public/tsuro-mark.png)

A marca é um tsuru de três dobras.

## O que é

Tsuro é um leitor nativo para o desktop, escrito em Rust ([iced](https://iced.rs/) + [Pdfium](https://pdfium.googlesource.com/pdfium/)). O alvo é qualquer PDF: grande, com tipografia exigente ou com assinatura digital.

O que ele faz cabe na barra:

- **Abrir** pelo botão ou arrastando o arquivo para a janela
- **Páginas** — anterior, próxima, e o contador Página N / total
- **Zoom** — ajustar à largura, encaixar a página, `+` / `−`
- **Buscar** no texto extraído, com a conta de ocorrências
- **Assinaturas** — painel com o signatário e o estado criptográfico
- **Copiar** o texto selecionado, quando há seleção

Ainda não há anotação, destaque, notas adesivas nem gravação na nuvem. Isso não é um atraso de roadmap disfarçado: Tsuro pretende continuar pequeno.

## Instalar (macOS)

```bash
git clone https://github.com/claudioorjunior/tsuro-pdf.git
cd tsuro-pdf
./scripts/bundle-macos.sh
```

Arraste `dist/Tsuro.app` para `/Applications` e abra pelo ícone. O script baixa o Pdfium, compila o visor nativo e monta o `.app`.

## Requisitos para desenvolver

- [Rust](https://rustup.rs/) estável (`rustup default stable`)
- Biblioteca [Pdfium](https://github.com/bblanchon/pdfium-binaries) no diretório do projeto ou no sistema — o `bundle-macos.sh` resolve isso no Mac

Node.js só entra se você for mexer no visor legado (Tauri + PDF.js), que não é o produto.

## Rodar a partir do código

```bash
cargo test -p tsuro-sign
cargo run -p tsuro -- public/samples/guia-folio.pdf
```

Sem argumento, a janela abre vazia. Controles ficam na barra, não em atalhos.

## Exemplos

Há dois PDFs em `public/samples/`:

| Arquivo | O que testa |
| --- | --- |
| `guia-folio.pdf` | Tipografia, tabela, acentos, busca |
| `contrato-assinado.pdf` | Campo `/Sig` com certificado autoassinado de demonstração |

O certificado do contrato é **autoassinado**. Tsuro trata isso como assinatura criptograficamente íntegra, mas sem cadeia de confiança pública. O estado esperado é “íntegra (sem confiança pública)”.

## Assinaturas digitais

O reconhecimento e a verificação vivem no crate Rust `tsuro-sign`:

- percorre AcroForm e dicionários `/Sig`
- lê o PKCS#7/CMS (perfil `adbe.pkcs7.detached`)
- confere o `ByteRange` e o `messageDigest`
- verifica RSA + SHA-256 sobre os atributos assinados

Estados que o painel pode mostrar: válida, íntegra (sem confiança pública), documento alterado, inválida, não suportada, certificado expirado ou ainda não válido.

## Estrutura

```
crates/tsuro            visor nativo (iced + Pdfium) — o produto
crates/tsuro-sign       motor de assinaturas (PDF + CMS)
public/samples          PDFs de exemplo
public/tsuro-mark.png   marca
src-tauri, src          visor legado (Tauri + React + PDF.js)
```

O legado continua no repositório para consulta. Não é o que se empacota como Tsuro.

```bash
npm install
npm run test:sign
npm run dev          # visor legado no navegador (http://127.0.0.1:43177)
npm run tauri dev
```

## Licença

[MIT](LICENSE). Contribuições de leitura, verificação e empacotamento são bem-vindas. Tsuro pretende continuar pequeno.

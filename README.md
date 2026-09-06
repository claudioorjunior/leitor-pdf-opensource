# Folio

Leitor de PDF de código aberto para **macOS** e **Windows**. Leve, fiel à tipografia e capaz de reconhecer assinaturas digitais.

Folio não é uma suíte. Não pede conta, não envia o arquivo para a nuvem e não tenta editar o documento. Abre o PDF, desenha cada página no lugar certo, deixa copiar o texto e diz se a assinatura criptográfica ainda cobre o arquivo.

## Por que Rust

O visor usa uma casca nativa [Tauri](https://tauri.app/) (WebView do sistema, não Electron). O reconhecimento e a verificação das assinaturas vivem num crate Rust puro, `folio-sign`:

- percorre AcroForm e dicionários `/Sig`
- lê o PKCS#7/CMS (perfil `adbe.pkcs7.detached`)
- confere o `ByteRange` e o `messageDigest`
- verifica RSA + SHA-256 sobre os atributos assinados

A renderização das páginas usa [PDF.js](https://mozilla.github.io/pdf.js/) 6, com camada de texto alinhada aos glifos — acentos portugueses, cifras e seleção de texto caem no sítio certo.

## Requisitos

- Node.js 22+
- Rust 1.85+ (`rustup default stable`)
- Para o app nativo: pré-requisitos do [Tauri 2](https://v2.tauri.app/start/prerequisites/) no seu sistema

## Desenvolvimento

```bash
npm install
npm run test:sign    # testes do motor de assinaturas
npm run dev          # visor no navegador (http://127.0.0.1:43177)
```

App nativo:

```bash
npm run tauri dev
```

Pacotes de instalação:

```bash
npm run tauri build
```

No macOS gera `.app` / `.dmg`. No Windows, instalador NSIS / MSI.

## Uso

- **Abrir** — botão, arrastar o arquivo, ou `⌘/Ctrl+O`
- **Zoom** — `⌘/Ctrl +` e `−`; `⌘/Ctrl+0` ajusta à largura
- **Busca** — `⌘/Ctrl+F` percorre o texto extraído de cada página
- **Assinaturas** — `⌘/Ctrl+I` abre o painel com signatário, motivo, cobertura do arquivo e estado criptográfico

Há dois PDFs de exemplo em `public/samples/`:

| Arquivo | O que testa |
| --- | --- |
| `guia-folio.pdf` | Tipografia, tabela, acentos, busca |
| `contrato-assinado.pdf` | Campo `/Sig` com certificado autoassinado de demonstração |

O certificado do contrato é **autoassinado**. O Folio trata isso como assinatura criptograficamente íntegra, mas sem cadeia de confiança pública — o estado esperado é “íntegra (sem confiança pública)”.

## Arquitetura

```
crates/folio-sign   motor Rust (PDF + CMS)
src-tauri           app Tauri (macOS / Windows / Linux)
src                 visor React + PDF.js
public/samples      documentos de exemplo
```

O visor web funciona sozinho (útil para desenvolver a leitura). No binário nativo, a verificação CMS passa pelo crate Rust via comando Tauri.

## Licença

MIT. Contribuições de leitura, verificação e empacotamento são bem-vindas — o Folio pretende continuar pequeno.

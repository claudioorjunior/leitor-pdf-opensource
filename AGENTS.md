# AGENTS.md

## Mission

Tsuro PDF é um leitor de PDF veloz, otimizado e leve, para quem apenas quer ler um PDF e fazer anotações e marcações, sem funções complicadas ou desnecessárias.

Antes de propor ou implementar uma mudança, pergunte: isso deixa ler, anotar ou marcar mais rápido, mais leve ou mais claro? Se a resposta for um recurso fora disso — suíte, nuvem, formulário, impressão, colaboração — recuse ou adie.

## Commands

- `cargo test -p tsuro` — session, browse, engine
- `cargo test -p tsuro-sign` — digital signatures
- `cargo run -p tsuro -- public/samples/guia-folio.pdf` — native viewer
- `./scripts/bundle-macos.sh` — macOS `.app` + DMG
- `powershell -File scripts/bundle-windows.ps1` — NSIS CurrentUser
- `bun run install:tsuro` — instalador provisório (GitHub Releases)
- `bun run test:install` — plano de install (fixtures, sem rede)

The Tauri/React tree (`src`, `src-tauri`) is legacy. Do not extend it.

## Code Map

- `crates/tsuro` — iced + Pdfium viewer (the product)
- `crates/tsuro/src/session.rs` — document session, messages, panels
- `crates/tsuro/src/browse.rs` — empty-state folders and recents
- `crates/tsuro/src/view.rs` — chrome
- `crates/tsuro-sign` — PDF + CMS signature engine
- `public/samples` — fixture PDFs
- `scripts/install-tsuro.ts` — instalador Bun (não entra no .app)
- `.github/workflows/release.yml` — DMG + NSIS no tag `v*`

## Conventions

- Use `use` and `pub` with `crate::` paths.
- Keep chrome in `view.rs`; session state stays in `session.rs`.
- Completo = ler e marcar sem travar. New work must stay small.

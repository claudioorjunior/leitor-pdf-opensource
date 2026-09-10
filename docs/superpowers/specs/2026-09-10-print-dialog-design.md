# Print dialog próprio (estilo PDFgear) — design

Data: 2026-09-10 · Branch: `claudioorjunior/feat-print-imprimir-como-pdf-de-impress-o-200-dp` · Issue: #27
Status: aprovado por seção pelo usuário, aguardando revisão do spec escrito.

## Contexto

A branch atual imprime por dois extremos: folha nativa do macOS (NSPrintPanel via
objc2, commit 6a020fe) ou PDF temporário aberto no visualizador padrão (demais SOs).
A decisão de produto é trocar ambos por um **diálogo de impressão próprio do Tsuro**,
estilo PDFgear: preview + impressora + intervalo + cópias + orientação, com spool
direto para a impressora nas 3 plataformas.

Decisões registradas:

- Abordagem A: diálogo próprio em iced + crate `printers` 2.x (CUPS no mac/Linux,
  winspool no Windows) para listar impressoras e spool. Alternativas descartadas:
  B (comandos do SO — parsing frágil, sem CLI decente no Windows) e C (híbrido
  com folha nativa — já descartado pelo usuário).
- Controles v1 (essencial): impressora, intervalo (Todas/Atual/De–Até), cópias
  (1–99), orientação (Auto/Retrato/Paisagem), preview. Fora: papel, duplex,
  cor/P&B, salvar-como-PDF.
- Critério de pronto: **papel correto** nas 3 plataformas (teste físico).
- Spool pode usar ferramentas do SO (CUPS/spooler); sem exigência de binário
  autocontido.

## 1. Arquitetura e módulos

- `crates/tsuro/src/print.rs` continua o núcleo, estendido com função pura
  `print_selection(pages, dpi, range, copies, orientation) -> jobs`. Intervalo,
  cópias (páginas repetidas N×, teto 99) e orientação vão **assados no PDF**,
  então funcionam mesmo onde o spooler ignora opções. Sem IO, testável.
- Novo `crates/tsuro/src/spool.rs`, fino: `list_printers()`,
  `default_printer()`, `spool_pdf(printer, bytes, title) -> job_id`. Envolve o
  crate `printers` e traduz para tipos próprios — `session.rs` nunca importa
  `printers` direto, para testes não precisarem de impressora.
- `session.rs`: `Ready` ganha `print_dialog: Option<PrintDialog>` (impressoras,
  seleção, intervalo, cópias, orientação, busy, erro) + mensagens no padrão
  atual (`OpenPrintDialog`, `PrintSubmit`, `PrintSubmitted { doc_gen, result }`).
- `view.rs`: painel modal em iced renderizado do `PrintDialog`, sem estado próprio.
- **Deleta**: caminho NSPrintPanel + deps objc2 (substituído pelo spool). O caminho
  de PDF temporário vira o botão "Abrir PDF" (rota de fuga, todas as plataformas).
- Deps no fim: `pdf-writer` + `miniz_oxide` + `printers`. Saldo: +1 nova
  (`printers` precisa de aprovação formal pelo AGENTS.md "Ask first").

## 2. UI do diálogo

Modal em iced sobre o documento (fundo escurecido, fecha com Cancelar ou Esc):

- Esquerda: **preview** — bitmap da página em escala de tela vindo do cache atual
  (sem render novo; placeholder "carregando…" se o cache não tem a página).
  Setas + "página X de Y" navegando dentro do intervalo escolhido.
- Direita: impressora (`pick_list`, default pré-selecionada), intervalo
  (Todas | Atual | De–Até com dois campos numéricos), cópias (stepper 1–99),
  orientação (Automática | Retrato | Paisagem).
- Rodapé: **[Imprimir]** (primário), **[Abrir PDF]** (secundário, rota de fuga),
  **[Cancelar]**. Durante o envio: botões travam, rótulo vira "Enviando…"; erro
  aparece em banner dentro do diálogo, sem fechar.

## 3. Fluxo de dados

1. ⋯ → Imprimir dispara `OpenPrintDialog`: lista impressoras em `spawn_blocking`
   e abre o modal com a default pré-selecionada.
2. Trocar intervalo/cópias/orientação só mexe no estado do diálogo — zero IO.
3. `PrintSubmit` valida (há impressora? intervalo dentro do documento?) e roda em
   `spawn_blocking`: monta o PDF filtrado/expandido/rotacionado → `spool_pdf` →
   volta `PrintSubmitted { doc_gen, result }`.
4. Sucesso fecha o modal + 1 linha de status ("Enviado para {impressora}, job
   {id}"); documento continua aberto, cache intacto.
5. `doc_gen` guarda contra troca de documento no meio do envio. "Abrir PDF" grava
   o temporário e abre no visualizador, sem spool.

Nota de implementação (vai no plano): preferir `printers::print(buffer)` para o
crate cuidar do temporário dele; confirmar num spike que ele trata PDF nas 3
plataformas, senão cair para `print_file` com o `print_temp_path` atual.

## 4. Erros

- **Sem impressora no SO**: diálogo abre, mostra "Nenhuma impressora encontrada",
  Imprimir desabilitado, "Abrir PDF" continua valendo.
- **Falha no spool** (offline, job recusado, erro CUPS/winspool): banner no diálogo
  com a mensagem do SO, diálogo aberto, documento intacto. Sem `Session::Failed`.
- **Intervalo inválido** (De > Até, fora do documento): validação inline, Imprimir
  desabilitado até corrigir. Sem popup.
- **Cancelar durante o envio**: não há — após o submit o job é do spooler; v1 não
  cancela job (evolução futura, fora desta spec).
- **Windows sem PDF nativo**: erro do spooler no banner; rota de fuga "Abrir PDF".
  Limitação documentada, não silenciosa.

Todo IO em `spawn_blocking`; nada perde documento, descarrega página ou trava a UI.

## 5. Testes

- **Unitários** (sem impressora, sem IO): expansão intervalo × cópias; matriz de
  rotação por orientação; mapeamento de opções CUPS (função pura); validação de
  intervalo; mapeamento `printers::Printer` → tipo próprio via fakes.
- **Sessão**: abrir/fechar, trocar seleção, submit monta os jobs certos — spool com
  stub em `cfg(test)` (padrão do `present_print_pdf` atual, job fake).
  `cargo test -p tsuro` verde sem impressora instalada.
- **Smoke manual por SO** (critério de pronto = papel correto): listar
  impressoras, imprimir Todas, intervalo De–Até, 2 cópias, forçar paisagem, sem
  impressora (banner + Abrir PDF), cancelar diálogo. Um roteiro, 1× por plataforma.
- **Fora**: spool real automatizado (exigiria CUPS/winspool no CI), teste da UI iced.

## Limitações conhecidas

- Windows winspool RAW só imprime PDF em impressora com suporte nativo a PDF;
  demais casos falham com erro visível + rota "Abrir PDF" (v1).
- `printers` 2.3.0 (MIT, repo talesluna/rust-printers): verificado API
  (`get_printers`, `get_default_printer`, `print`, `print_file`, opções raw CUPS);
  spike deve confirmar `print(buffer)` com PDF nas 3 plataformas antes do plano final.

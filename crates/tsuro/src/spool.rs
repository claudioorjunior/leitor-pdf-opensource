//! Spool de impressão: lista do SO + envio do PDF, fronteira com o crate `printers`.
//!
//! `session.rs` só vê estes tipos — nunca `printers` direto — para os testes
//! rodarem sem impressora (stub em `cfg(test)`).

use crate::print::PrintError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrinterInfo {
    pub name: String,
    pub is_default: bool,
}

#[cfg(test)]
pub fn list_printers() -> Vec<PrinterInfo> {
    vec![PrinterInfo {
        name: "Impressora de teste".into(),
        is_default: true,
    }]
}

#[cfg(not(test))]
pub fn list_printers() -> Vec<PrinterInfo> {
    printers::get_printers()
        .into_iter()
        .map(|printer| PrinterInfo {
            name: printer.name,
            is_default: printer.is_default,
        })
        .collect()
}

#[cfg(test)]
pub fn spool_pdf(_printer: &str, _pdf: &[u8], _job_title: &str) -> Result<u64, PrintError> {
    Ok(1)
}

/// Envia o PDF assado (intervalo/cópias/orientação já aplicados); devolve o job id.
#[cfg(not(test))]
pub fn spool_pdf(printer: &str, pdf: &[u8], job_title: &str) -> Result<u64, PrintError> {
    use printers::common::base::job::PrinterJobOptions;
    use printers::get_printer_by_name;

    if pdf.is_empty() {
        return Err(PrintError("PDF de impressão vazio".into()));
    }
    let Some(target) = get_printer_by_name(printer) else {
        return Err(PrintError(format!("impressora não encontrada: {printer}")));
    };
    let mut options = PrinterJobOptions::none();
    options.name = Some(job_title);
    target
        .print(pdf, options)
        .map_err(|err| PrintError(format!("falha no spool: {}", err.message)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_spool_returns_fake_job() {
        assert_eq!(
            spool_pdf("qualquer", b"%PDF-1.4", "titulo").expect("stub job"),
            1
        );
        assert_eq!(list_printers().len(), 1);
        assert!(list_printers()[0].is_default);
    }
}

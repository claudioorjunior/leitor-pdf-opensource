//! Digital signature inspection and CMS verification for PDF files.
//!
//! Recognition walks the PDF object graph (AcroForm fields and signature
//! dictionaries). Cryptographic checks decode the PKCS#7/CMS blob, compare the
//! message-digest signed attribute with the ByteRange payload, and verify the
//! signer’s RSA signature over the signed attributes.

mod cms;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use lopdf::{Dictionary, Document, Object};
use rsa::pkcs1v15::{Signature as RsaPkcs1Signature, VerifyingKey};
use rsa::signature::Verifier as _;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use cms::parse_signed_data;

#[derive(Debug, thiserror::Error)]
pub enum SigError {
    #[error("não foi possível ler o PDF: {0}")]
    Pdf(String),
    #[error("CMS inválido: {0}")]
    Cms(String),
    #[error("algoritmo não suportado: {0}")]
    Unsupported(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SignatureStatus {
    Valid,
    IntactButUntrusted,
    DocumentModified,
    Invalid,
    Unsupported,
    CertificateExpired,
    CertificateNotYetValid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateInfo {
    pub subject: String,
    pub common_name: Option<String>,
    pub organization: Option<String>,
    pub issuer: String,
    pub serial: String,
    pub not_before: String,
    pub not_after: String,
    pub is_self_signed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureInfo {
    pub field_name: Option<String>,
    pub signer_name: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact_info: Option<String>,
    pub signing_time: Option<String>,
    pub filter: Option<String>,
    pub sub_filter: Option<String>,
    pub byte_range: Option<[i64; 4]>,
    pub covers_whole_document: bool,
    pub status: SignatureStatus,
    pub status_detail: String,
    pub certificate: Option<CertificateInfo>,
    pub digest_algorithm: Option<String>,
    pub signature_algorithm: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfAnalysis {
    pub page_count_hint: Option<u32>,
    pub has_acro_form: bool,
    pub signatures: Vec<SignatureInfo>,
}

pub fn analyze_pdf(bytes: &[u8]) -> Result<PdfAnalysis, SigError> {
    let document = Document::load_mem(bytes).map_err(|e| SigError::Pdf(e.to_string()))?;
    let mut signatures = Vec::new();
    let mut visited: Vec<Vec<u8>> = Vec::new();

    let has_acro_form = document
        .catalog()
        .ok()
        .and_then(|c| c.get(b"AcroForm").ok())
        .is_some();

    for object in document.objects.values() {
        let Object::Dictionary(dict) = object else {
            continue;
        };
        if let Some(sig) = signature_from_field(&document, dict, bytes, &mut visited) {
            signatures.push(sig);
        } else if looks_like_sig_dict(dict) {
            if let Some(sig) = signature_from_dict(&document, dict, None, bytes, &mut visited) {
                signatures.push(sig);
            }
        }
    }

    let page_count_hint = Some(document.get_pages().len() as u32);

    Ok(PdfAnalysis {
        page_count_hint,
        has_acro_form,
        signatures,
    })
}

pub fn verify_cms_b64(pkcs7_b64: &str, sha256_hex: &str) -> Result<SignatureInfo, SigError> {
    let pkcs7 = B64.decode(pkcs7_b64).map_err(|e| SigError::Cms(e.to_string()))?;
    let digest = hex_decode(sha256_hex).map_err(|e| SigError::Cms(e))?;
    verify_cms(&pkcs7, Some(digest.as_slice()), None)
}

pub fn verify_cms(
    pkcs7: &[u8],
    sha256: Option<&[u8]>,
    sha1: Option<&[u8]>,
) -> Result<SignatureInfo, SigError> {
    let parsed = parse_signed_data(pkcs7)?;
    let certificate = parsed.certificate.clone();
    let mut info = SignatureInfo {
        field_name: None,
        signer_name: certificate.as_ref().and_then(|c| c.common_name.clone()),
        reason: None,
        location: None,
        contact_info: None,
        signing_time: None,
        filter: None,
        sub_filter: Some("adbe.pkcs7.detached".into()),
        byte_range: None,
        covers_whole_document: true,
        status: SignatureStatus::Unsupported,
        status_detail: String::new(),
        certificate: certificate.clone(),
        digest_algorithm: Some(parsed.digest_algorithm.to_string()),
        signature_algorithm: Some(parsed.signature_algorithm.to_string()),
    };

    let provided_digest: Option<&[u8]> = match parsed.digest_algorithm {
        "SHA-256" => sha256,
        "SHA-1" => sha1,
        _ => None,
    };

    let digest_ok = match (&parsed.message_digest, provided_digest) {
        (Some(got), Some(expected)) => got.as_slice() == expected,
        (None, Some(_)) => {
            info.status_detail =
                "CMS sem atributos assinados; não é o perfil destacado usual de PDF.".into();
            false
        }
        _ => {
            info.status = SignatureStatus::Unsupported;
            info.status_detail = format!(
                "Algoritmo de resumo {} sem payload correspondente.",
                parsed.digest_algorithm
            );
            return Ok(info);
        }
    };

    if !digest_ok {
        info.status = SignatureStatus::DocumentModified;
        info.status_detail =
            "O resumo do intervalo assinado não confere. O arquivo foi alterado após a assinatura."
                .into();
        return Ok(info);
    }

    let Some(signed_attrs) = parsed.signed_attrs_for_verify.as_ref() else {
        info.status = SignatureStatus::IntactButUntrusted;
        info.status_detail =
            "O resumo do documento confere, mas o CMS não traz atributos assinados.".into();
        return Ok(info);
    };
    let Some(public) = parsed.public_key else {
        info.status = SignatureStatus::IntactButUntrusted;
        info.status_detail =
            "O documento coberto está íntegro, mas a chave pública RSA não pôde ser extraída."
                .into();
        return Ok(info);
    };

    let signature = RsaPkcs1Signature::try_from(parsed.signature.as_slice())
        .map_err(|e| SigError::Cms(e.to_string()))?;
    let crypto_ok = match parsed.digest_algorithm {
        "SHA-256" => VerifyingKey::<Sha256>::new(public)
            .verify(signed_attrs, &signature)
            .is_ok(),
        "SHA-1" => VerifyingKey::<Sha1>::new_unprefixed(public)
            .verify(signed_attrs, &signature)
            .is_ok(),
        other => {
            info.status = SignatureStatus::IntactButUntrusted;
            info.status_detail = format!(
                "O documento coberto está íntegro, mas o algoritmo {other} ainda não é verificado."
            );
            return Ok(info);
        }
    };

    if !crypto_ok {
        info.status = SignatureStatus::Invalid;
        info.status_detail = "A criptografia da assinatura não confere.".into();
        return Ok(info);
    }

    if certificate
        .as_ref()
        .map(|c| c.is_self_signed)
        .unwrap_or(true)
    {
        info.status = SignatureStatus::IntactButUntrusted;
        info.status_detail = "Assinatura criptograficamente válida, mas o certificado é autoassinado — não há cadeia de confiança pública.".into();
    } else {
        info.status = SignatureStatus::Valid;
        info.status_detail = "Assinatura íntegra: o conteúdo coberto não foi alterado.".into();
    }

    Ok(info)
}

fn signature_from_field(
    document: &Document,
    field: &Dictionary,
    bytes: &[u8],
    visited: &mut Vec<Vec<u8>>,
) -> Option<SignatureInfo> {
    let ft = name_of(field.get(b"FT").ok()?)?;
    if ft != "Sig" {
        return None;
    }
    let field_name = string_of(field.get(b"T").ok());
    let value = field.get(b"V").ok()?;
    let dict = deref_dict(document, value)?;
    signature_from_dict(document, &dict, field_name, bytes, visited)
}

fn signature_from_dict(
    _document: &Document,
    dict: &Dictionary,
    field_name: Option<String>,
    bytes: &[u8],
    visited: &mut Vec<Vec<u8>>,
) -> Option<SignatureInfo> {
    let contents = contents_bytes(dict)?;
    if visited.iter().any(|c| c == &contents) {
        return None;
    }
    visited.push(contents.clone());

    let byte_range = parse_byte_range(dict);
    let covers = byte_range
        .map(|br| covers_whole_document(bytes, br))
        .unwrap_or(false);

    let sha256 = byte_range.map(|br| hash_byte_range::<Sha256>(bytes, br));
    let sha1 = byte_range.map(|br| hash_byte_range::<Sha1>(bytes, br));

    let mut info = verify_cms(
        &contents,
        sha256.as_deref(),
        sha1.as_deref(),
    )
    .unwrap_or_else(|e| SignatureInfo {
        field_name: None,
        signer_name: string_of(dict.get(b"Name").ok()),
        reason: string_of(dict.get(b"Reason").ok()),
        location: string_of(dict.get(b"Location").ok()),
        contact_info: string_of(dict.get(b"ContactInfo").ok()),
        signing_time: string_of(dict.get(b"M").ok()),
        filter: name_of(dict.get(b"Filter").ok().unwrap_or(&Object::Null)),
        sub_filter: name_of(dict.get(b"SubFilter").ok().unwrap_or(&Object::Null)),
        byte_range,
        covers_whole_document: covers,
        status: SignatureStatus::Invalid,
        status_detail: e.to_string(),
        certificate: None,
        digest_algorithm: None,
        signature_algorithm: None,
    });

    if info.field_name.is_none() {
        info.field_name = field_name;
    }
    if info.signer_name.is_none() {
        info.signer_name = string_of(dict.get(b"Name").ok());
    }
    if info.reason.is_none() {
        info.reason = string_of(dict.get(b"Reason").ok());
    }
    if info.location.is_none() {
        info.location = string_of(dict.get(b"Location").ok());
    }
    if info.contact_info.is_none() {
        info.contact_info = string_of(dict.get(b"ContactInfo").ok());
    }
    if info.signing_time.is_none() {
        info.signing_time = string_of(dict.get(b"M").ok()).map(|m| format_pdf_date(&m));
    } else if let Some(raw) = info.signing_time.clone() {
        info.signing_time = Some(format_pdf_date(&raw));
    }
    info.filter = name_of(dict.get(b"Filter").ok().unwrap_or(&Object::Null)).or(info.filter);
    info.sub_filter =
        name_of(dict.get(b"SubFilter").ok().unwrap_or(&Object::Null)).or(info.sub_filter);
    info.byte_range = byte_range.or(info.byte_range);
    info.covers_whole_document = covers;

    if !covers && matches!(info.status, SignatureStatus::Valid | SignatureStatus::IntactButUntrusted)
    {
        info.status = SignatureStatus::DocumentModified;
        info.status_detail =
            "Há bytes no arquivo fora do intervalo assinado. O documento foi alterado depois da assinatura."
                .into();
    }

    Some(info)
}

fn looks_like_sig_dict(dict: &Dictionary) -> bool {
    match dict.get(b"Type").ok().and_then(name_of) {
        Some(name) if name == "Sig" => true,
        _ => dict.get(b"ByteRange").is_ok() && dict.get(b"Contents").is_ok(),
    }
}

fn deref_dict(document: &Document, object: &Object) -> Option<Dictionary> {
    match object {
        Object::Dictionary(dict) => Some(dict.clone()),
        Object::Reference(id) => match document.get_object(*id).ok()? {
            Object::Dictionary(dict) => Some(dict.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn name_of(object: &Object) -> Option<String> {
    match object {
        Object::Name(n) => Some(String::from_utf8_lossy(n).into_owned()),
        Object::Reference(_) => None,
        _ => None,
    }
}

fn string_of(object: Option<&Object>) -> Option<String> {
    match object? {
        Object::String(s, _) => Some(String::from_utf8_lossy(s).into_owned()),
        Object::Name(n) => Some(String::from_utf8_lossy(n).into_owned()),
        _ => None,
    }
}

fn contents_bytes(dict: &Dictionary) -> Option<Vec<u8>> {
    match dict.get(b"Contents").ok()? {
        Object::String(s, _) => Some(s.clone()),
        _ => None,
    }
}

fn parse_byte_range(dict: &Dictionary) -> Option<[i64; 4]> {
    let Object::Array(items) = dict.get(b"ByteRange").ok()? else {
        return None;
    };
    if items.len() != 4 {
        return None;
    }
    let mut out = [0i64; 4];
    for (i, item) in items.iter().enumerate() {
        out[i] = match item {
            Object::Integer(n) => *n,
            Object::Real(n) => *n as i64,
            _ => return None,
        };
    }
    Some(out)
}

fn covers_whole_document(bytes: &[u8], br: [i64; 4]) -> bool {
    let [a, b, c, d] = br.map(|n| n as usize);
    if a != 0 || a + b > bytes.len() || c + d > bytes.len() || a + b > c {
        return false;
    }
    let gap = &bytes[a + b..c];
    if gap.first() != Some(&b'<') || gap.last() != Some(&b'>') {
        return false;
    }
    let tail = &bytes[c + d..];
    tail.iter()
        .all(|b| matches!(b, b' ' | b'\n' | b'\r' | b'\t' | 0 | b'\x0c'))
}

fn hash_byte_range<D: Digest>(bytes: &[u8], br: [i64; 4]) -> Vec<u8> {
    let [a, b, c, d] = br.map(|n| n as usize);
    let mut hasher = D::new();
    let start = a.min(bytes.len());
    let mid = (a + b).min(bytes.len());
    let second = c.min(bytes.len());
    let end = (c + d).min(bytes.len());
    hasher.update(&bytes[start..mid]);
    hasher.update(&bytes[second..end]);
    hasher.finalize().to_vec()
}

fn format_pdf_date(raw: &str) -> String {
    let s = raw.strip_prefix("D:").unwrap_or(raw);
    if s.len() >= 14 {
        format!(
            "{}-{}-{} {}:{}:{}",
            &s[0..4],
            &s[4..6],
            &s[6..8],
            &s[8..10],
            &s[10..12],
            &s[12..14]
        )
    } else {
        raw.to_string()
    }
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("hex inválido".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn unsigned_guide_has_no_signatures() {
        let bytes = std::fs::read(fixture("guia-folio.pdf")).expect("guia");
        let analysis = analyze_pdf(&bytes).expect("parse");
        assert!(analysis.signatures.is_empty());
    }

    #[test]
    fn signed_contract_is_recognized() {
        let bytes = std::fs::read(fixture("contrato-assinado.pdf")).expect("contrato");
        let analysis = analyze_pdf(&bytes).expect("parse");
        assert_eq!(analysis.signatures.len(), 1, "{analysis:?}");
        let sig = &analysis.signatures[0];
        assert!(sig.covers_whole_document, "{}", sig.status_detail);
        assert!(
            matches!(
                sig.status,
                SignatureStatus::Valid | SignatureStatus::IntactButUntrusted
            ),
            "{:?} {}",
            sig.status,
            sig.status_detail
        );
        let cn = sig
            .certificate
            .as_ref()
            .and_then(|c| c.common_name.as_deref())
            .or(sig.signer_name.as_deref())
            .unwrap_or("");
        assert!(cn.contains("Maria"), "{cn}");
    }

    #[test]
    fn tampering_after_signature_is_detected() {
        let mut bytes = std::fs::read(fixture("contrato-assinado.pdf")).expect("contrato");
        let last = bytes.len() - 1;
        bytes[last] ^= 0x7f;
        if bytes[last] == 0 || bytes[last].is_ascii_whitespace() {
            bytes[last] = b'X';
        }
        // Flip a byte inside the first range if the tail is ignored whitespace.
        if bytes.len() > 80 {
            bytes[40] ^= 0x11;
        }
        let analysis = analyze_pdf(&bytes).expect("parse");
        assert_eq!(analysis.signatures.len(), 1);
        assert!(
            matches!(
                analysis.signatures[0].status,
                SignatureStatus::DocumentModified | SignatureStatus::Invalid
            ),
            "{:?}",
            analysis.signatures[0]
        );
    }
}

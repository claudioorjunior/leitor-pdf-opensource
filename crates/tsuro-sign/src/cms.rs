//! Lenient CMS/PKCS#7 walker.
//!
//! Real-world PDF signatures often violate strict DER SET uniqueness, so the
//! rustcrypto `cms` crate rejects them. This walker reads TLV structure without
//! that check.

use crate::{CertificateInfo, SigError};
use rsa::BigUint;
use rsa::RsaPublicKey;
use x509_parser::prelude::{FromDer, X509Certificate};
use x509_parser::public_key::PublicKey;

#[derive(Clone, Copy)]
struct Tlv<'a> {
    tag: u8,
    value: &'a [u8],
    full: &'a [u8],
}

pub struct ParsedCms {
    pub certificate: Option<CertificateInfo>,
    pub public_key: Option<RsaPublicKey>,
    pub message_digest: Option<Vec<u8>>,
    pub digest_algorithm: &'static str,
    pub signature_algorithm: &'static str,
    pub signature: Vec<u8>,
    pub signed_attrs_for_verify: Option<Vec<u8>>,
    pub cert_not_before: Option<i64>,
    pub cert_not_after: Option<i64>,
}

pub fn parse_signed_data(pkcs7: &[u8]) -> Result<ParsedCms, SigError> {
    let der = trim_der(pkcs7);
    let (content_info, _) = read_tlv(der)?;
    expect_tag(content_info, 0x30, "ContentInfo")?;
    let ci_children = children(content_info)?;
    if ci_children.len() < 2 {
        return Err(SigError::Cms("ContentInfo incompleto".into()));
    }
    let signed_wrapper = ci_children[1];
    // [0] EXPLICIT SignedData
    let signed_seq = if signed_wrapper.tag & 0xc0 == 0x80 {
        let (inner, _) = read_tlv(signed_wrapper.value)?;
        inner
    } else {
        signed_wrapper
    };
    expect_tag(signed_seq, 0x30, "SignedData")?;
    let sd = children(signed_seq)?;
    if sd.len() < 4 {
        return Err(SigError::Cms("SignedData incompleto".into()));
    }

    let mut idx = 3; // skip version, digestAlgs, encapContentInfo
    let mut certs_blob: Option<&[u8]> = None;
    while idx < sd.len() && sd[idx].tag & 0xc0 == 0x80 {
        if sd[idx].tag == 0xa0 {
            certs_blob = Some(sd[idx].value);
        }
        idx += 1;
    }
    if idx >= sd.len() {
        return Err(SigError::Cms("SignerInfos ausente".into()));
    }
    let signer_infos = sd[idx];
    let first_signer = children(signer_infos)?
        .into_iter()
        .next()
        .ok_or_else(|| SigError::Cms("CMS sem SignerInfo".into()))?;
    expect_tag(first_signer, 0x30, "SignerInfo")?;
    let si = children(first_signer)?;
    if si.len() < 5 {
        return Err(SigError::Cms("SignerInfo incompleto".into()));
    }

    // version, sid, digestAlg, [signedAttrs], sigAlg, signature
    let mut cursor = 2;
    let digest_algorithm = algorithm_name(&si[cursor])?;
    cursor += 1;

    let mut signed_attrs_for_verify = None;
    let mut message_digest = None;
    if si[cursor].tag == 0xa0 {
        let mut rewritten = Vec::with_capacity(si[cursor].full.len());
        rewritten.push(0x31);
        rewritten.extend_from_slice(&si[cursor].full[1..]);
        signed_attrs_for_verify = Some(rewritten);
        message_digest = extract_message_digest(si[cursor].value)?;
        cursor += 1;
    }
    if cursor >= si.len() {
        return Err(SigError::Cms("signatureAlgorithm ausente".into()));
    }
    let signature_algorithm = algorithm_name(&si[cursor])?;
    cursor += 1;
    if cursor >= si.len() {
        return Err(SigError::Cms("signature ausente".into()));
    }
    expect_tag(si[cursor], 0x04, "signature")?;
    let signature = si[cursor].value.to_vec();

    let (certificate, public_key, cert_not_before, cert_not_after) = match certs_blob {
        Some(blob) => parse_first_cert(blob),
        None => (None, None, None, None),
    };

    Ok(ParsedCms {
        certificate,
        public_key,
        message_digest,
        digest_algorithm,
        signature_algorithm,
        signature,
        signed_attrs_for_verify,
        cert_not_before,
        cert_not_after,
    })
}

fn parse_first_cert(
    mut data: &[u8],
) -> (
    Option<CertificateInfo>,
    Option<RsaPublicKey>,
    Option<i64>,
    Option<i64>,
) {
    while !data.is_empty() {
        let Ok((tlv, rest)) = read_tlv(data) else {
            break;
        };
        data = rest;
        if tlv.tag != 0x30 {
            continue;
        }
        let Ok((_, cert)) = X509Certificate::from_der(tlv.full) else {
            continue;
        };
        let common_name = cert
            .subject()
            .iter_common_name()
            .next()
            .and_then(|a| a.as_str().ok().map(str::to_string));
        let organization = cert
            .subject()
            .iter_organization()
            .next()
            .and_then(|a| a.as_str().ok().map(str::to_string));
        let is_self_signed = cert.subject() == cert.issuer();
        let info = CertificateInfo {
            subject: cert.subject().to_string(),
            common_name,
            organization,
            issuer: cert.issuer().to_string(),
            serial: format!("{:x}", cert.serial),
            not_before: cert.validity().not_before.to_string(),
            not_after: cert.validity().not_after.to_string(),
            is_self_signed,
        };
        let key = match cert.public_key().parsed() {
            Ok(PublicKey::RSA(rsa_key)) => RsaPublicKey::new(
                BigUint::from_bytes_be(rsa_key.modulus),
                BigUint::from_bytes_be(rsa_key.exponent),
            )
            .ok(),
            _ => None,
        };
        return (
            Some(info),
            key,
            Some(cert.validity().not_before.timestamp()),
            Some(cert.validity().not_after.timestamp()),
        );
    }
    (None, None, None, None)
}

fn extract_message_digest(attrs_blob: &[u8]) -> Result<Option<Vec<u8>>, SigError> {
    let mut data = attrs_blob;
    let oid = oid_from_str("1.2.840.113549.1.9.4");
    while !data.is_empty() {
        let (attr, rest) = read_tlv(data)?;
        data = rest;
        if attr.tag != 0x30 {
            continue;
        }
        let parts = children(attr)?;
        if parts.len() < 2 || parts[0].tag != 0x06 {
            continue;
        }
        if parts[0].value != oid {
            continue;
        }
        let set = parts[1];
        let (octet, _) = read_tlv(set.value)?;
        if octet.tag == 0x04 {
            return Ok(Some(octet.value.to_vec()));
        }
        return Ok(Some(octet.full.to_vec()));
    }
    Ok(None)
}

fn algorithm_name(alg_id: &Tlv<'_>) -> Result<&'static str, SigError> {
    let parts = children(*alg_id)?;
    let oid = parts
        .first()
        .filter(|p| p.tag == 0x06)
        .ok_or_else(|| SigError::Cms("AlgorithmIdentifier sem OID".into()))?;
    Ok(oid_digest_name(oid.value))
}

fn oid_digest_name(bytes: &[u8]) -> &'static str {
    // SHA-256: 2.16.840.1.101.3.4.2.1
    if bytes == [0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01] {
        return "SHA-256";
    }
    // SHA-1: 1.3.14.3.2.26
    if bytes == [0x2b, 0x0e, 0x03, 0x02, 0x1a] {
        return "SHA-1";
    }
    // SHA-512: 2.16.840.1.101.3.4.2.3
    if bytes == [0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03] {
        return "SHA-512";
    }
    // rsaEncryption 1.2.840.113549.1.1.1
    if bytes == [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01] {
        return "RSA";
    }
    // sha256WithRSAEncryption 1.2.840.113549.1.1.11
    if bytes == [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b] {
        return "RSA";
    }
    "desconhecido"
}

fn oid_from_str(s: &str) -> Vec<u8> {
    let parts: Vec<u128> = s.split('.').filter_map(|p| p.parse().ok()).collect();
    if parts.len() < 2 {
        return Vec::new();
    }
    let mut out = vec![(parts[0] * 40 + parts[1]) as u8];
    for component in &parts[2..] {
        let mut n = *component;
        let mut stack = Vec::new();
        stack.push((n & 0x7f) as u8);
        n >>= 7;
        while n > 0 {
            stack.push(((n & 0x7f) as u8) | 0x80);
            n >>= 7;
        }
        stack.reverse();
        out.extend(stack);
    }
    out
}

fn expect_tag(tlv: Tlv<'_>, tag: u8, what: &str) -> Result<(), SigError> {
    if tlv.tag != tag {
        Err(SigError::Cms(format!(
            "{what}: esperado tag {tag:#x}, veio {:#x}",
            tlv.tag
        )))
    } else {
        Ok(())
    }
}

fn children(seq: Tlv<'_>) -> Result<Vec<Tlv<'_>>, SigError> {
    let mut data = seq.value;
    let mut out = Vec::new();
    while !data.is_empty() {
        let (tlv, rest) = read_tlv(data)?;
        out.push(tlv);
        data = rest;
    }
    Ok(out)
}

fn read_tlv(data: &[u8]) -> Result<(Tlv<'_>, &[u8]), SigError> {
    if data.len() < 2 {
        return Err(SigError::Cms("TLV curto".into()));
    }
    let tag = data[0];
    let (hlen, vlen) = if data[1] & 0x80 == 0 {
        (2usize, data[1] as usize)
    } else {
        let n = (data[1] & 0x7f) as usize;
        if n == 0 || n > 4 || data.len() < 2 + n {
            return Err(SigError::Cms("comprimento DER inválido".into()));
        }
        let mut vlen = 0usize;
        for i in 0..n {
            vlen = (vlen << 8) | data[2 + i] as usize;
        }
        (2 + n, vlen)
    };
    if data.len() < hlen + vlen {
        return Err(SigError::Cms("TLV truncado".into()));
    }
    let full = &data[..hlen + vlen];
    let value = &data[hlen..hlen + vlen];
    let rest = &data[hlen + vlen..];
    Ok((Tlv { tag, value, full }, rest))
}

pub fn trim_der(data: &[u8]) -> &[u8] {
    if data.is_empty() {
        return data;
    }
    let start = data.iter().position(|b| *b == 0x30).unwrap_or(0);
    let rest = &data[start..];
    match der_len(rest) {
        Some(len) if len <= rest.len() => &rest[..len],
        _ => {
            let end = rest
                .iter()
                .rposition(|b| *b != 0)
                .map(|i| i + 1)
                .unwrap_or(rest.len());
            &rest[..end]
        }
    }
}

fn der_len(data: &[u8]) -> Option<usize> {
    if data.len() < 2 || data[0] != 0x30 {
        return None;
    }
    let b1 = data[1];
    if b1 & 0x80 == 0 {
        return Some(2 + b1 as usize);
    }
    let n = (b1 & 0x7f) as usize;
    if n == 0 || n > 4 || data.len() < 2 + n {
        return None;
    }
    let mut len = 0usize;
    for i in 0..n {
        len = (len << 8) | data[2 + i] as usize;
    }
    Some(2 + n + len)
}

use tsuro_sign::{analyze_pdf, verify_cms_b64, PdfAnalysis, SignatureInfo};

#[tauri::command]
fn analyze_pdf_bytes(bytes: Vec<u8>) -> Result<PdfAnalysis, String> {
    analyze_pdf(&bytes).map_err(|e| e.to_string())
}

#[tauri::command]
fn verify_cms_signature(pkcs7_b64: String, sha256_hex: String) -> Result<SignatureInfo, String> {
    verify_cms_b64(&pkcs7_b64, &sha256_hex).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            analyze_pdf_bytes,
            verify_cms_signature
        ])
        .run(tauri::generate_context!())
        .expect("error while running Tsuro");
}

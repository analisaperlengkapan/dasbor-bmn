//! Uji integrasi penyimpanan konfigurasi.
//!
//! Dijalankan sebagai proses tersendiri (bukan uji unit) supaya dapat mengatur
//! `SIMAN_DATA_DIR` ke direktori sementara tanpa berebut dengan basis data uji
//! yang dipakai uji unit lain. Seluruh skenario ada dalam satu fungsi uji karena
//! konfigurasi aktif disimpan pada `OnceLock` global dan variabel environment
//! bersifat proses — uji paralel akan saling mengganggu.
//!
//! Menjalankan: `cargo test --features ssr --test config_konkuren`

#![cfg(feature = "ssr")]

use dasbor_bmn::config::{self, FileConfig};
use dasbor_bmn::dto::Asumsi;

fn dir_baru(tanda: &str) -> std::path::PathBuf {
    let unik = format!(
        "dasbor-bmn-config-uji-{tanda}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let dir = std::env::temp_dir().join(unik);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("SIMAN_DATA_DIR", &dir);
    dir
}

#[test]
fn penyimpanan_konfigurasi_aman() {
    // (1) Menyimpan sebagian medan tidak boleh menghapus medan lain.
    let d1 = dir_baru("sebagian");
    config::save_file_config(FileConfig {
        client_id: "id-1".into(),
        client_secret: "rahasia-1".into(),
        grant_type: "client_credentials".into(),
        ba_key: "ba-1".into(),
        ..Default::default()
    })
    .unwrap();
    // Hanya satu medan diisi ulang; sisanya dibiarkan kosong oleh pengguna.
    config::save_file_config(FileConfig {
        client_id: "id-2".into(),
        ..Default::default()
    })
    .unwrap();
    let aktif = config::get();
    assert_eq!(aktif.client_id, "id-2");
    assert_eq!(aktif.client_secret, "rahasia-1", "rahasia tidak boleh hilang");
    assert_eq!(aktif.grant_type, "client_credentials");
    assert_eq!(aktif.ba_key, "ba-1");
    let _ = std::fs::remove_dir_all(&d1);

    // (2) Kredensial dari environment yang dibiarkan kosong di berkas tetap
    // berlaku pada konfigurasi aktif setelah penyimpanan (tidak hilang sampai
    // proses dimulai ulang). Dulu hanya medan yang diisi pengguna yang diterapkan.
    let d2 = dir_baru("environment");
    std::env::set_var("SIMAN_CLIENT_SECRET", "dari-environment");
    config::save_file_config(FileConfig {
        client_id: "id-env".into(),
        ..Default::default()
    })
    .unwrap();
    let aktif = config::get();
    assert_eq!(aktif.client_id, "id-env");
    assert_eq!(
        aktif.client_secret, "dari-environment",
        "kredensial environment hilang setelah menyimpan"
    );
    std::env::remove_var("SIMAN_CLIENT_SECRET");
    let _ = std::fs::remove_dir_all(&d2);

    // (3) Penyimpanan kredensial dan asumsi yang berjalan bersamaan tidak boleh
    // saling menimpa: keduanya menulis ke berkas JSON yang sama.
    let d3 = dir_baru("bersamaan");
    config::save_file_config(FileConfig {
        client_id: "id-awal".into(),
        client_secret: "rahasia-awal".into(),
        ..Default::default()
    })
    .unwrap();
    let mut utas = Vec::new();
    for _ in 0..8 {
        utas.push(std::thread::spawn(|| {
            config::save_file_config(FileConfig {
                client_secret: "rahasia-baru".into(),
                ..Default::default()
            })
            .unwrap();
        }));
        utas.push(std::thread::spawn(|| {
            config::simpan_asumsi(&Asumsi {
                eskalasi: 0.1,
                faktor_rehab: 0.6,
                satuan_umur_sisa: "tahun".into(),
            })
            .unwrap();
        }));
    }
    for t in utas {
        t.join().unwrap();
    }
    let isi = std::fs::read_to_string(d3.join("config.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&isi).unwrap();
    assert_eq!(
        v["client_secret"], "rahasia-baru",
        "kredensial hilang oleh penyimpanan asumsi: {isi}"
    );
    assert_eq!(
        v["eskalasi"], 0.1,
        "asumsi hilang oleh penyimpanan kredensial: {isi}"
    );
    assert_eq!(v["faktor_rehab"], 0.6);
    let _ = std::fs::remove_dir_all(&d3);
}

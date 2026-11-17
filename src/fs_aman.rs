//! Utilitas berkas untuk data sensitif.
//!
//! Berkas yang memuat rahasia (kredensial SSO, kata sandi dasbor) ditulis
//! dengan mode `0600` dan direktori induknya `0700`, sehingga pengguna lokal
//! lain tidak dapat membacanya — apa pun umask proses.

#![cfg(feature = "ssr")]

use std::io::Write;
use std::path::Path;

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

/// Tulis berkas rahasia dengan mode `0600`, mengganti isi lama sepenuhnya.
///
/// Isi ditulis ke berkas sementara di direktori yang sama lalu diganti dengan
/// `rename`. Karena `rename` bersifat atomik di dalam satu sistem berkas,
/// pembaca tidak pernah melihat dokumen terpotong atau setengah tertulis —
/// kegagalan penulisan pun tidak merusak isi lama. Berkas sementara memakai
/// mode `0600` sejak dibuat agar isinya tidak pernah terbaca pihak lain.
pub fn tulis_privat(path: &Path, isi: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    // Direktori induk juga dibatasi agar isi tidak dapat dibaca pihak lain.
    let _ = set_mode(parent, 0o700);

    let sementara = parent.join(format!(
        ".{}.tmp-{}-{}",
        path.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "berkas".to_string()),
        std::process::id(),
        sisa_unicode()
    ));
    // Mode 0600 sejak berkas dibuat: tanpa ini berkas sementara sempat terbaca
    // pengguna lokal lain selama penulisan.
    {
        let mut opsi = std::fs::OpenOptions::new();
        opsi.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opsi.mode(0o600);
        }
        let mut f = opsi.open(&sementara)?;
        let hasil = f.write_all(isi).and_then(|_| f.sync_all());
        drop(f);
        if let Err(e) = hasil {
            let _ = std::fs::remove_file(&sementara);
            return Err(e);
        }
    }
    if let Err(e) = set_mode(&sementara, 0o600) {
        let _ = std::fs::remove_file(&sementara);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&sementara, path) {
        let _ = std::fs::remove_file(&sementara);
        return Err(e);
    }
    set_mode(path, 0o600)
}

/// Pembeda unik untuk nama berkas sementara.
///
/// Menggabungkan waktu resolusi tinggi dengan penghitung proses, sehingga dua
/// penulisan bersamaan (bahkan dari utas berbeda dalam proses yang sama) selalu
/// mendapat nama berbeda.
fn sisa_unicode() -> u128 {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed) as u128;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    nanos << 16 | (n & 0xffff)
}

/// Batasi direktori data agar hanya pemilik proses yang dapat membukanya.
///
/// Berkas pendamping SQLite (`-wal`, `-shm`) dibuat pustaka dengan izin bawaan;
/// membatasi direktorinya menutup akses ke seluruh isinya sekaligus.
pub fn rapatkan_dir(path: &Path) {
    if path.is_dir() {
        let _ = set_mode(path, 0o700);
    }
}

/// Perketat izin berkas yang sudah ada (dipakai saat memuat konfigurasi lama).
pub fn rapatkan(path: &Path) {
    if path.exists() {
        let _ = set_mode(path, 0o600);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn berkas_ditulis_dengan_mode_0600() {
        let dir = std::env::temp_dir().join(format!("uji-fs-aman-{}", std::process::id()));
        let path = dir.join("rahasia.json");
        tulis_privat(&path, br#"{"a":1}"#).unwrap();
        let isi = std::fs::read_to_string(&path).unwrap();
        assert_eq!(isi, r#"{"a":1}"#);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "mode berkas harus 0600, dapat {mode:o}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[cfg(unix)]
    fn direktori_data_hanya_untuk_pemilik() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("uji-fs-aman-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        rapatkan_dir(&dir);
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn menulis_ulang_mengganti_isi_lama() {
        let dir = std::env::temp_dir().join(format!("uji-fs-aman2-{}", std::process::id()));
        let path = dir.join("rahasia.json");
        tulis_privat(&path, b"panjang-panjang-panjang").unwrap();
        tulis_privat(&path, b"pendek").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "pendek");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Pembaca tidak boleh pernah melihat dokumen kosong atau terpotong.
    ///
    /// Dulu berkas tujuan dipotong lebih dulu (`truncate`) sebelum ditulis,
    /// sehingga pembaca bersamaan sempat melihat dokumen kosong dan pemuatan
    /// konfigurasi menggantinya dengan nilai bawaan. Penulisan kini memakai
    /// berkas sementara lalu `rename`, jadi isi lama utuh sampai penggantian
    /// atomik dan pembaca hanya melihat isi penuh (lama atau baru).
    #[test]
    fn pembaca_tidak_melihat_dokumen_kosong() {
        let dir = std::env::temp_dir().join(format!("uji-fs-aman3-{}", std::process::id()));
        let path = dir.join("config.json");
        tulis_privat(&path, b"{\"lama\":1}").unwrap();

        let selesai = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let berhenti = selesai.clone();
        let p = path.clone();
        let pembaca = std::thread::spawn(move || {
            let mut kosong = 0usize;
            let mut n = 0usize;
            while !berhenti.load(std::sync::atomic::Ordering::Relaxed) {
                let isi = std::fs::read_to_string(&p).unwrap();
                n += 1;
                if isi.is_empty() {
                    kosong += 1;
                }
            }
            (kosong, n)
        });
        for i in 0..500 {
            tulis_privat(&path, format!("{{\"isi\":{i}}}").as_bytes()).unwrap();
        }
        selesai.store(true, std::sync::atomic::Ordering::Relaxed);
        let (kosong, n) = pembaca.join().unwrap();
        assert!(n > 0, "pembaca tidak sempat membaca");
        assert_eq!(kosong, 0, "pembaca melihat berkas kosong {kosong}x dari {n}x");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Tidak boleh ada berkas sementara yang tertinggal setelah penulisan.
    #[test]
    fn tidak_meninggalkan_berkas_sementara() {
        let dir = std::env::temp_dir().join(format!("uji-fs-aman4-{}", std::process::id()));
        let path = dir.join("config.json");
        for i in 0..20 {
            tulis_privat(&path, format!("{{\"isi\":{i}}}").as_bytes()).unwrap();
        }
        let sisa: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(sisa, vec!["config.json".to_string()], "sisa: {sisa:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

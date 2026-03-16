/// archive.rs — Detect and extract compressed archive formats.
///
/// Supports OVA (plain tar), gzip/bzip2/xz/zstd-compressed tar, and ZIP
/// archives. Detection is based on file extension (case-insensitive).
use anyhow::{Context, Result};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

/// Recognised archive formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Ova,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    TarZst,
    Zip,
}

impl ArchiveFormat {
    /// Human-readable label for status messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::Ova => "OVA",
            Self::Tar => "tar",
            Self::TarGz => "tar.gz",
            Self::TarBz2 => "tar.bz2",
            Self::TarXz => "tar.xz",
            Self::TarZst => "tar.zst",
            Self::Zip => "ZIP",
        }
    }
}

/// Detect archive format from the file extension (case-insensitive).
///
/// Checks double extensions first (e.g. `.tar.gz`) before single ones,
/// because `Path::extension()` only returns the last component.
pub fn detect_format(path: &Path) -> Option<ArchiveFormat> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();

    // Double extensions first
    if name.ends_with(".tar.gz") {
        return Some(ArchiveFormat::TarGz);
    }
    if name.ends_with(".tar.bz2") {
        return Some(ArchiveFormat::TarBz2);
    }
    if name.ends_with(".tar.xz") {
        return Some(ArchiveFormat::TarXz);
    }
    if name.ends_with(".tar.zst") {
        return Some(ArchiveFormat::TarZst);
    }

    // Single extensions
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "ova" => Some(ArchiveFormat::Ova),
        "tar" => Some(ArchiveFormat::Tar),
        "tgz" => Some(ArchiveFormat::TarGz),
        "tbz2" => Some(ArchiveFormat::TarBz2),
        "txz" => Some(ArchiveFormat::TarXz),
        "tzst" => Some(ArchiveFormat::TarZst),
        "zip" => Some(ArchiveFormat::Zip),
        _ => None,
    }
}

/// Extract the archive at `path` (of the given `format`) into `target_dir`.
///
/// Returns the path to the target directory on success.
pub fn extract_archive(path: &Path, format: ArchiveFormat, target_dir: &Path) -> Result<PathBuf> {
    match format {
        ArchiveFormat::Ova | ArchiveFormat::Tar => extract_tar(path, target_dir),
        ArchiveFormat::TarGz => extract_tar_gz(path, target_dir),
        ArchiveFormat::TarBz2 => extract_tar_bz2(path, target_dir),
        ArchiveFormat::TarXz => extract_tar_xz(path, target_dir),
        ArchiveFormat::TarZst => extract_tar_zst(path, target_dir),
        ArchiveFormat::Zip => extract_zip(path, target_dir),
    }
}

// ─── Internal helpers ────────────────────────────────────────────────────────

fn open_file(path: &Path) -> Result<File> {
    File::open(path).with_context(|| format!("Cannot open archive: {}", path.display()))
}

fn unpack_tar<R: std::io::Read>(reader: R, path: &Path, target_dir: &Path) -> Result<PathBuf> {
    let mut archive = tar::Archive::new(reader);
    archive
        .unpack(target_dir)
        .with_context(|| format!("Failed to extract archive: {}", path.display()))?;
    Ok(target_dir.to_path_buf())
}

fn extract_tar(path: &Path, target_dir: &Path) -> Result<PathBuf> {
    let file = open_file(path)?;
    unpack_tar(file, path, target_dir)
}

fn extract_tar_gz(path: &Path, target_dir: &Path) -> Result<PathBuf> {
    let file = open_file(path)?;
    let decoder = flate2::read::GzDecoder::new(BufReader::new(file));
    unpack_tar(decoder, path, target_dir)
}

fn extract_tar_bz2(path: &Path, target_dir: &Path) -> Result<PathBuf> {
    let file = open_file(path)?;
    let decoder = bzip2::read::BzDecoder::new(BufReader::new(file));
    unpack_tar(decoder, path, target_dir)
}

fn extract_tar_xz(path: &Path, target_dir: &Path) -> Result<PathBuf> {
    let file = open_file(path)?;
    let decoder = xz2::read::XzDecoder::new(BufReader::new(file));
    unpack_tar(decoder, path, target_dir)
}

fn extract_tar_zst(path: &Path, target_dir: &Path) -> Result<PathBuf> {
    let file = open_file(path)?;
    let decoder = zstd::Decoder::new(BufReader::new(file))
        .with_context(|| format!("Failed to initialise zstd decoder: {}", path.display()))?;
    unpack_tar(decoder, path, target_dir)
}

fn extract_zip(path: &Path, target_dir: &Path) -> Result<PathBuf> {
    let file = open_file(path)?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file))
        .with_context(|| format!("Failed to read ZIP archive: {}", path.display()))?;
    archive
        .extract(target_dir)
        .with_context(|| format!("Failed to extract ZIP archive: {}", path.display()))?;
    Ok(target_dir.to_path_buf())
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    /// Build a tar archive in memory containing a fake .ovf and .vmdk.
    fn make_test_tar_bytes() -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());

        let ovf_content = b"<?xml version=\"1.0\"?><Envelope/>";
        let mut header = tar::Header::new_gnu();
        header.set_size(ovf_content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "test.ovf", &ovf_content[..])
            .unwrap();

        let vmdk_content = b"fake vmdk data";
        let mut header = tar::Header::new_gnu();
        header.set_size(vmdk_content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "test.vmdk", &vmdk_content[..])
            .unwrap();

        builder.into_inner().unwrap()
    }

    fn assert_extracted_files(dir: &Path) {
        assert!(dir.join("test.ovf").exists());
        assert!(dir.join("test.vmdk").exists());
    }

    // ── Detection tests ──────────────────────────────────────────────────────

    #[test]
    fn detect_ova() {
        assert_eq!(detect_format(Path::new("vm.ova")), Some(ArchiveFormat::Ova));
        assert_eq!(detect_format(Path::new("VM.OVA")), Some(ArchiveFormat::Ova));
    }

    #[test]
    fn detect_tar() {
        assert_eq!(detect_format(Path::new("vm.tar")), Some(ArchiveFormat::Tar));
    }

    #[test]
    fn detect_tar_gz() {
        assert_eq!(
            detect_format(Path::new("vm.tar.gz")),
            Some(ArchiveFormat::TarGz)
        );
        assert_eq!(
            detect_format(Path::new("vm.tgz")),
            Some(ArchiveFormat::TarGz)
        );
        assert_eq!(
            detect_format(Path::new("VM.TAR.GZ")),
            Some(ArchiveFormat::TarGz)
        );
    }

    #[test]
    fn detect_tar_bz2() {
        assert_eq!(
            detect_format(Path::new("vm.tar.bz2")),
            Some(ArchiveFormat::TarBz2)
        );
        assert_eq!(
            detect_format(Path::new("vm.tbz2")),
            Some(ArchiveFormat::TarBz2)
        );
    }

    #[test]
    fn detect_tar_xz() {
        assert_eq!(
            detect_format(Path::new("vm.tar.xz")),
            Some(ArchiveFormat::TarXz)
        );
        assert_eq!(
            detect_format(Path::new("vm.txz")),
            Some(ArchiveFormat::TarXz)
        );
    }

    #[test]
    fn detect_tar_zst() {
        assert_eq!(
            detect_format(Path::new("vm.tar.zst")),
            Some(ArchiveFormat::TarZst)
        );
        assert_eq!(
            detect_format(Path::new("vm.tzst")),
            Some(ArchiveFormat::TarZst)
        );
    }

    #[test]
    fn detect_zip() {
        assert_eq!(detect_format(Path::new("vm.zip")), Some(ArchiveFormat::Zip));
        assert_eq!(detect_format(Path::new("VM.Zip")), Some(ArchiveFormat::Zip));
    }

    #[test]
    fn detect_non_archive_returns_none() {
        assert_eq!(detect_format(Path::new("vm.ovf")), None);
        assert_eq!(detect_format(Path::new("vm.vmdk")), None);
        assert_eq!(detect_format(Path::new("readme.txt")), None);
        assert_eq!(detect_format(Path::new("noext")), None);
    }

    // ── Extraction tests ─────────────────────────────────────────────────────

    #[test]
    fn extract_plain_tar() {
        let tmp = TempDir::new().unwrap();
        let tar_path = tmp.path().join("test.tar");
        std::fs::write(&tar_path, make_test_tar_bytes()).unwrap();

        let out = tmp.path().join("out");
        std::fs::create_dir(&out).unwrap();
        extract_archive(&tar_path, ArchiveFormat::Tar, &out).unwrap();
        assert_extracted_files(&out);
    }

    #[test]
    fn extract_ova() {
        let tmp = TempDir::new().unwrap();
        let ova_path = tmp.path().join("test.ova");
        std::fs::write(&ova_path, make_test_tar_bytes()).unwrap();

        let out = tmp.path().join("out");
        std::fs::create_dir(&out).unwrap();
        extract_archive(&ova_path, ArchiveFormat::Ova, &out).unwrap();
        assert_extracted_files(&out);
    }

    #[test]
    fn extract_tar_gz() {
        let tmp = TempDir::new().unwrap();
        let archive_path = tmp.path().join("test.tar.gz");

        let tar_bytes = make_test_tar_bytes();
        let file = File::create(&archive_path).unwrap();
        let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap();

        let out = tmp.path().join("out");
        std::fs::create_dir(&out).unwrap();
        extract_archive(&archive_path, ArchiveFormat::TarGz, &out).unwrap();
        assert_extracted_files(&out);
    }

    #[test]
    fn extract_tar_bz2() {
        let tmp = TempDir::new().unwrap();
        let archive_path = tmp.path().join("test.tar.bz2");

        let tar_bytes = make_test_tar_bytes();
        let file = File::create(&archive_path).unwrap();
        let mut encoder = bzip2::write::BzEncoder::new(file, bzip2::Compression::fast());
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap();

        let out = tmp.path().join("out");
        std::fs::create_dir(&out).unwrap();
        extract_archive(&archive_path, ArchiveFormat::TarBz2, &out).unwrap();
        assert_extracted_files(&out);
    }

    #[test]
    fn extract_tar_xz() {
        let tmp = TempDir::new().unwrap();
        let archive_path = tmp.path().join("test.tar.xz");

        let tar_bytes = make_test_tar_bytes();
        let file = File::create(&archive_path).unwrap();
        let mut encoder = xz2::write::XzEncoder::new(file, 1);
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap();

        let out = tmp.path().join("out");
        std::fs::create_dir(&out).unwrap();
        extract_archive(&archive_path, ArchiveFormat::TarXz, &out).unwrap();
        assert_extracted_files(&out);
    }

    #[test]
    fn extract_tar_zst() {
        let tmp = TempDir::new().unwrap();
        let archive_path = tmp.path().join("test.tar.zst");

        let tar_bytes = make_test_tar_bytes();
        let file = File::create(&archive_path).unwrap();
        let mut encoder = zstd::Encoder::new(file, 1).unwrap();
        encoder.write_all(&tar_bytes).unwrap();
        encoder.finish().unwrap();

        let out = tmp.path().join("out");
        std::fs::create_dir(&out).unwrap();
        extract_archive(&archive_path, ArchiveFormat::TarZst, &out).unwrap();
        assert_extracted_files(&out);
    }

    #[test]
    fn extract_zip_archive() {
        let tmp = TempDir::new().unwrap();
        let zip_path = tmp.path().join("test.zip");

        {
            let file = File::create(&zip_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            writer.start_file("test.ovf", options).unwrap();
            writer
                .write_all(b"<?xml version=\"1.0\"?><Envelope/>")
                .unwrap();
            writer.start_file("test.vmdk", options).unwrap();
            writer.write_all(b"fake vmdk data").unwrap();
            writer.finish().unwrap();
        }

        let out = tmp.path().join("out");
        std::fs::create_dir(&out).unwrap();
        extract_archive(&zip_path, ArchiveFormat::Zip, &out).unwrap();
        assert_extracted_files(&out);
    }

    // ── Error tests ──────────────────────────────────────────────────────────

    #[test]
    fn extract_nonexistent_file_errors() {
        let tmp = TempDir::new().unwrap();
        let result = extract_archive(
            Path::new("/nonexistent/path.tar.gz"),
            ArchiveFormat::TarGz,
            tmp.path(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn extract_corrupt_tar_gz_errors() {
        let tmp = TempDir::new().unwrap();
        let bad_path = tmp.path().join("corrupt.tar.gz");
        std::fs::write(&bad_path, b"this is not a valid archive").unwrap();

        let out = tmp.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let result = extract_archive(&bad_path, ArchiveFormat::TarGz, &out);
        assert!(result.is_err());
    }

    #[test]
    fn extract_corrupt_zip_errors() {
        let tmp = TempDir::new().unwrap();
        let bad_path = tmp.path().join("corrupt.zip");
        std::fs::write(&bad_path, b"this is not a zip").unwrap();

        let out = tmp.path().join("out");
        std::fs::create_dir(&out).unwrap();
        let result = extract_archive(&bad_path, ArchiveFormat::Zip, &out);
        assert!(result.is_err());
    }
}

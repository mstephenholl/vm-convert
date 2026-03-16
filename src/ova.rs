/// ova.rs — Extract OVA (Open Virtual Appliance) archives.
///
/// An OVA file is a tar archive containing an OVF descriptor, disk images,
/// and optionally a manifest (.mf) file. This module extracts the archive
/// contents into a target directory.
use anyhow::{Context, Result};
use std::fs::File;
use std::path::{Path, PathBuf};

/// Returns `true` when the path has an `.ova` extension (case-insensitive).
pub fn is_ova(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("ova"))
        .unwrap_or(false)
}

/// Extract an OVA tar archive into `target_dir`.
///
/// Returns the path to the target directory on success.
pub fn extract_ova(ova_path: &Path, target_dir: &Path) -> Result<PathBuf> {
    let file = File::open(ova_path)
        .with_context(|| format!("Cannot open OVA file: {}", ova_path.display()))?;

    let mut archive = tar::Archive::new(file);
    archive
        .unpack(target_dir)
        .with_context(|| format!("Failed to extract OVA archive: {}", ova_path.display()))?;

    Ok(target_dir.to_path_buf())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_is_ova_true() {
        assert!(is_ova(Path::new("/tmp/myvm.ova")));
        assert!(is_ova(Path::new("test.OVA")));
        assert!(is_ova(Path::new("vm.Ova")));
    }

    #[test]
    fn test_is_ova_false() {
        assert!(!is_ova(Path::new("/tmp/myvm.ovf")));
        assert!(!is_ova(Path::new("/tmp/myvm")));
        assert!(!is_ova(Path::new("/tmp/myvm.tar")));
    }

    #[test]
    fn test_extract_ova_creates_files() {
        let tmp = TempDir::new().unwrap();
        let ova_path = tmp.path().join("test.ova");

        // Create a tar archive with an .ovf and a fake .vmdk
        {
            let file = File::create(&ova_path).unwrap();
            let mut builder = tar::Builder::new(file);

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

            builder.finish().unwrap();
        }

        let extract_dir = tmp.path().join("extracted");
        std::fs::create_dir(&extract_dir).unwrap();
        extract_ova(&ova_path, &extract_dir).unwrap();

        assert!(extract_dir.join("test.ovf").exists());
        assert!(extract_dir.join("test.vmdk").exists());
    }

    #[test]
    fn test_extract_ova_nonexistent_file_errors() {
        let tmp = TempDir::new().unwrap();
        let result = extract_ova(Path::new("/nonexistent/path.ova"), tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_ova_corrupt_file_errors() {
        let tmp = TempDir::new().unwrap();
        let bad_ova = tmp.path().join("corrupt.ova");
        let mut f = File::create(&bad_ova).unwrap();
        f.write_all(b"this is not a tar file").unwrap();

        let extract_dir = tmp.path().join("out");
        std::fs::create_dir(&extract_dir).unwrap();
        let result = extract_ova(&bad_ova, &extract_dir);
        assert!(result.is_err());
    }
}

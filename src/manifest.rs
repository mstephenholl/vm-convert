/// manifest.rs — Parse and verify OVF manifest (.mf) files.
///
/// Manifest files contain one line per referenced file in the format:
///   `ALGORITHM(filename)= hexhash`
///
/// Supported algorithms: SHA1, SHA256, SHA512.
use anyhow::{bail, Context, Result};
use digest::Digest;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// A single manifest entry.
#[derive(Debug, Clone, PartialEq)]
struct ManifestEntry {
    algorithm: HashAlgorithm,
    filename: String,
    expected_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum HashAlgorithm {
    Sha1,
    Sha256,
    Sha512,
}

/// Verify all files listed in a `.mf` manifest.
///
/// `mf_path` — path to the manifest file
/// `base_dir` — directory containing the referenced files
///
/// Returns `Ok(())` when all hashes match, or an error describing the first mismatch.
pub fn verify_manifest(mf_path: &Path, base_dir: &Path) -> Result<()> {
    let content = std::fs::read_to_string(mf_path)
        .with_context(|| format!("Cannot read manifest file: {}", mf_path.display()))?;

    let entries = parse_manifest(&content)?;

    if entries.is_empty() {
        bail!(
            "Manifest file contains no valid entries: {}",
            mf_path.display()
        );
    }

    for entry in &entries {
        let file_path = base_dir.join(&entry.filename);
        if !file_path.exists() {
            bail!(
                "Manifest references missing file: {} (expected in {})",
                entry.filename,
                base_dir.display()
            );
        }

        let actual_hash = compute_hash(&file_path, entry.algorithm)
            .with_context(|| format!("Failed to hash file: {}", entry.filename))?;

        if actual_hash != entry.expected_hash.to_lowercase() {
            bail!(
                "Hash mismatch for '{}': expected {} but got {}",
                entry.filename,
                entry.expected_hash,
                actual_hash,
            );
        }
    }

    Ok(())
}

/// Parse manifest content into entries.
fn parse_manifest(content: &str) -> Result<Vec<ManifestEntry>> {
    let mut entries = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(entry) = parse_manifest_line(line) {
            entries.push(entry);
        }
        // Silently skip malformed lines (some tools add comments)
    }

    Ok(entries)
}

/// Parse a single manifest line: `ALGORITHM(filename)= hexhash`
fn parse_manifest_line(line: &str) -> Option<ManifestEntry> {
    let paren_open = line.find('(')?;
    let paren_close = line.find(')')?;
    if paren_close <= paren_open {
        return None;
    }

    let algo_str = line[..paren_open].trim();
    let algorithm = match algo_str.to_uppercase().as_str() {
        "SHA1" => HashAlgorithm::Sha1,
        "SHA256" => HashAlgorithm::Sha256,
        "SHA512" => HashAlgorithm::Sha512,
        _ => return None,
    };

    let filename = line[paren_open + 1..paren_close].trim().to_string();
    if filename.is_empty() {
        return None;
    }

    // After ")" there should be "=" followed by the hash
    let rest = line[paren_close + 1..].trim();
    let rest = rest.strip_prefix('=')?;
    let expected_hash = rest.trim().to_lowercase();
    if expected_hash.is_empty() {
        return None;
    }

    Some(ManifestEntry {
        algorithm,
        filename,
        expected_hash,
    })
}

/// Compute the hex-encoded hash of a file using streaming reads.
fn compute_hash(path: &Path, algorithm: HashAlgorithm) -> Result<String> {
    let mut file = File::open(path)
        .with_context(|| format!("Cannot open file for hashing: {}", path.display()))?;

    let mut buf = [0u8; 8192];

    match algorithm {
        HashAlgorithm::Sha1 => {
            let mut hasher = sha1::Sha1::new();
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
            Ok(hex_encode(&hasher.finalize()))
        }
        HashAlgorithm::Sha256 => {
            let mut hasher = sha2::Sha256::new();
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
            Ok(hex_encode(&hasher.finalize()))
        }
        HashAlgorithm::Sha512 => {
            let mut hasher = sha2::Sha512::new();
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
            Ok(hex_encode(&hasher.finalize()))
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_parse_sha256_line() {
        let entry = parse_manifest_line(
            "SHA256(disk.vmdk)= abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        )
        .unwrap();
        assert_eq!(entry.algorithm, HashAlgorithm::Sha256);
        assert_eq!(entry.filename, "disk.vmdk");
        assert_eq!(
            entry.expected_hash,
            "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"
        );
    }

    #[test]
    fn test_parse_sha1_line() {
        let entry =
            parse_manifest_line("SHA1(vm.ovf)= da39a3ee5e6b4b0d3255bfef95601890afd80709").unwrap();
        assert_eq!(entry.algorithm, HashAlgorithm::Sha1);
        assert_eq!(entry.filename, "vm.ovf");
    }

    #[test]
    fn test_parse_sha512_line() {
        let hash = "a".repeat(128);
        let line = format!("SHA512(big.vmdk)= {hash}");
        let entry = parse_manifest_line(&line).unwrap();
        assert_eq!(entry.algorithm, HashAlgorithm::Sha512);
    }

    #[test]
    fn test_parse_line_no_space_after_equals() {
        let entry = parse_manifest_line(
            "SHA256(disk.vmdk)=abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        )
        .unwrap();
        assert_eq!(entry.filename, "disk.vmdk");
    }

    #[test]
    fn test_parse_malformed_line_returns_none() {
        assert!(parse_manifest_line("garbage line").is_none());
        assert!(parse_manifest_line("SHA256()= abc").is_none());
        assert!(parse_manifest_line("UNKNOWN(file)= abc").is_none());
        assert!(parse_manifest_line("SHA256(file)= ").is_none());
        assert!(parse_manifest_line("").is_none());
    }

    #[test]
    fn test_verify_correct_hash_passes() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("test.txt");
        std::fs::write(&file_path, b"hello world").unwrap();

        // SHA256 of "hello world"
        let hash = {
            let mut hasher = sha2::Sha256::new();
            hasher.update(b"hello world");
            hex_encode(&hasher.finalize())
        };

        let mf_content = format!("SHA256(test.txt)= {hash}");
        let mf_path = tmp.path().join("test.mf");
        std::fs::write(&mf_path, mf_content).unwrap();

        verify_manifest(&mf_path, tmp.path()).unwrap();
    }

    #[test]
    fn test_verify_wrong_hash_fails() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("test.txt");
        std::fs::write(&file_path, b"hello world").unwrap();

        let mf_content =
            "SHA256(test.txt)= 0000000000000000000000000000000000000000000000000000000000000000";
        let mf_path = tmp.path().join("test.mf");
        std::fs::write(&mf_path, mf_content).unwrap();

        let err = verify_manifest(&mf_path, tmp.path()).unwrap_err();
        assert!(err.to_string().contains("Hash mismatch"));
    }

    #[test]
    fn test_verify_missing_file_fails() {
        let tmp = TempDir::new().unwrap();

        let mf_content = "SHA256(nonexistent.vmdk)= abcdef";
        let mf_path = tmp.path().join("test.mf");
        std::fs::write(&mf_path, mf_content).unwrap();

        let err = verify_manifest(&mf_path, tmp.path()).unwrap_err();
        assert!(err.to_string().contains("missing file"));
    }

    #[test]
    fn test_verify_sha1_passes() {
        let tmp = TempDir::new().unwrap();
        let file_path = tmp.path().join("test.txt");
        std::fs::write(&file_path, b"test data").unwrap();

        let hash = {
            let mut hasher = sha1::Sha1::new();
            hasher.update(b"test data");
            hex_encode(&hasher.finalize())
        };

        let mf_content = format!("SHA1(test.txt)= {hash}");
        let mf_path = tmp.path().join("test.mf");
        std::fs::write(&mf_path, mf_content).unwrap();

        verify_manifest(&mf_path, tmp.path()).unwrap();
    }
}

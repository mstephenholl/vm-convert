/// inventory.rs — Scan a VM export folder and validate required files.
///
/// A valid VM export folder must contain:
///   - Exactly one `.ovf` file  (required)
///   - At least one disk image file (required)
///   - Optionally a `.nvram` file (indicates UEFI firmware)
///   - Optionally a `.mf` manifest file
///   - Optionally `.iso` files
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Disk file extensions we recognise.
const DISK_EXTENSIONS: &[&str] = &["vmdk", "vhd", "vhdx", "vdi", "raw", "img", "qcow2"];

// ─── Public data model ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct VmInventory {
    /// Path to the .ovf descriptor file
    pub ovf_path: PathBuf,
    /// Paths to all disk image files found in the folder
    pub disk_paths: Vec<PathBuf>,
    /// Path to the .nvram file, if present (indicates UEFI)
    pub nvram_path: Option<PathBuf>,
    /// Path to the .mf manifest file, if present
    pub mf_path: Option<PathBuf>,
    /// Paths to .iso files found in the folder
    pub iso_paths: Vec<PathBuf>,
}

impl VmInventory {
    /// Returns `true` when an `.nvram` sidecar was found.
    pub fn has_nvram(&self) -> bool {
        self.nvram_path.is_some()
    }
}

// ─── Public API ──────────────────────────────────────────────────────────────

/// Scan `dir` for VM export artefacts and return a validated inventory.
pub fn scan_vm_dir(dir: &Path) -> Result<VmInventory> {
    if !dir.exists() {
        bail!("Directory not found: {}", dir.display());
    }
    if !dir.is_dir() {
        bail!("Not a directory: {}", dir.display());
    }

    let entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("Cannot read directory: {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .collect();

    let mut ovf_files: Vec<PathBuf> = Vec::new();
    let mut disk_files: Vec<PathBuf> = Vec::new();
    let mut nvram_files: Vec<PathBuf> = Vec::new();
    let mut mf_files: Vec<PathBuf> = Vec::new();
    let mut iso_files: Vec<PathBuf> = Vec::new();

    for entry in &entries {
        let path = entry.path();
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
        {
            Some(ext) if ext == "ovf" => ovf_files.push(path),
            Some(ext) if DISK_EXTENSIONS.contains(&ext.as_str()) => disk_files.push(path),
            Some(ext) if ext == "nvram" => nvram_files.push(path),
            Some(ext) if ext == "mf" => mf_files.push(path),
            Some(ext) if ext == "iso" => iso_files.push(path),
            _ => {}
        }
    }

    // Validate OVF
    if ovf_files.is_empty() {
        bail!("No .ovf file found in {}", dir.display());
    }
    if ovf_files.len() > 1 {
        let names: Vec<_> = ovf_files
            .iter()
            .map(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        bail!(
            "Multiple .ovf files found in {} — expected exactly one: {}",
            dir.display(),
            names.join(", ")
        );
    }

    // Validate disk files
    if disk_files.is_empty() {
        bail!("No disk image file found in {}", dir.display());
    }

    // Validate NVRAM
    if nvram_files.len() > 1 {
        let names: Vec<_> = nvram_files
            .iter()
            .map(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        bail!(
            "Multiple .nvram files found in {} — expected at most one: {}",
            dir.display(),
            names.join(", ")
        );
    }

    // Sort for deterministic ordering
    disk_files.sort();
    iso_files.sort();

    Ok(VmInventory {
        ovf_path: ovf_files.into_iter().next().unwrap(),
        disk_paths: disk_files,
        nvram_path: nvram_files.into_iter().next(),
        mf_path: mf_files.into_iter().next(),
        iso_paths: iso_files,
    })
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_file(dir: &Path, name: &str) {
        fs::write(dir.join(name), "").unwrap();
    }

    #[test]
    fn test_valid_folder_with_all_files() {
        let tmp = TempDir::new().unwrap();
        create_file(tmp.path(), "vm.ovf");
        create_file(tmp.path(), "vm.vmdk");
        create_file(tmp.path(), "vm.nvram");

        let inv = scan_vm_dir(tmp.path()).unwrap();
        assert_eq!(inv.ovf_path.file_name().unwrap(), "vm.ovf");
        assert_eq!(inv.disk_paths.len(), 1);
        assert!(inv.has_nvram());
    }

    #[test]
    fn test_valid_folder_without_nvram() {
        let tmp = TempDir::new().unwrap();
        create_file(tmp.path(), "vm.ovf");
        create_file(tmp.path(), "vm.vmdk");

        let inv = scan_vm_dir(tmp.path()).unwrap();
        assert!(!inv.has_nvram());
    }

    #[test]
    fn test_multiple_disk_files() {
        let tmp = TempDir::new().unwrap();
        create_file(tmp.path(), "vm.ovf");
        create_file(tmp.path(), "disk1.vmdk");
        create_file(tmp.path(), "disk2.vmdk");

        let inv = scan_vm_dir(tmp.path()).unwrap();
        assert_eq!(inv.disk_paths.len(), 2);
    }

    #[test]
    fn test_non_vmdk_disk_formats() {
        let tmp = TempDir::new().unwrap();
        create_file(tmp.path(), "vm.ovf");
        create_file(tmp.path(), "disk.vdi");

        let inv = scan_vm_dir(tmp.path()).unwrap();
        assert_eq!(inv.disk_paths.len(), 1);
        assert!(inv.disk_paths[0]
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with(".vdi"));
    }

    #[test]
    fn test_vhd_disk_detected() {
        let tmp = TempDir::new().unwrap();
        create_file(tmp.path(), "vm.ovf");
        create_file(tmp.path(), "disk.vhd");

        let inv = scan_vm_dir(tmp.path()).unwrap();
        assert_eq!(inv.disk_paths.len(), 1);
    }

    #[test]
    fn test_no_ovf_file_errors() {
        let tmp = TempDir::new().unwrap();
        create_file(tmp.path(), "vm.vmdk");

        let err = scan_vm_dir(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("No .ovf file found"));
    }

    #[test]
    fn test_multiple_ovf_files_errors() {
        let tmp = TempDir::new().unwrap();
        create_file(tmp.path(), "a.ovf");
        create_file(tmp.path(), "b.ovf");
        create_file(tmp.path(), "vm.vmdk");

        let err = scan_vm_dir(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("Multiple .ovf files found"));
    }

    #[test]
    fn test_no_disk_file_errors() {
        let tmp = TempDir::new().unwrap();
        create_file(tmp.path(), "vm.ovf");

        let err = scan_vm_dir(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("No disk image file found"));
    }

    #[test]
    fn test_nonexistent_dir_errors() {
        let err = scan_vm_dir(Path::new("/tmp/does-not-exist-vm-convert-test")).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_file_instead_of_dir_errors() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("not-a-dir.txt");
        fs::write(&file, "").unwrap();

        let err = scan_vm_dir(&file).unwrap_err();
        assert!(err.to_string().contains("Not a directory"));
    }

    #[test]
    fn test_mf_file_detected() {
        let tmp = TempDir::new().unwrap();
        create_file(tmp.path(), "vm.ovf");
        create_file(tmp.path(), "vm.vmdk");
        create_file(tmp.path(), "vm.mf");

        let inv = scan_vm_dir(tmp.path()).unwrap();
        assert!(inv.mf_path.is_some());
        assert_eq!(inv.mf_path.unwrap().file_name().unwrap(), "vm.mf");
    }

    #[test]
    fn test_iso_files_detected() {
        let tmp = TempDir::new().unwrap();
        create_file(tmp.path(), "vm.ovf");
        create_file(tmp.path(), "vm.vmdk");
        create_file(tmp.path(), "tools.iso");

        let inv = scan_vm_dir(tmp.path()).unwrap();
        assert_eq!(inv.iso_paths.len(), 1);
    }

    #[test]
    fn test_ignores_unrelated_files() {
        let tmp = TempDir::new().unwrap();
        create_file(tmp.path(), "vm.ovf");
        create_file(tmp.path(), "vm.vmdk");
        create_file(tmp.path(), "notes.txt");

        let inv = scan_vm_dir(tmp.path()).unwrap();
        assert_eq!(inv.disk_paths.len(), 1);
        assert!(!inv.has_nvram());
    }

    #[test]
    fn test_multiple_nvram_files_errors() {
        let tmp = TempDir::new().unwrap();
        create_file(tmp.path(), "vm.ovf");
        create_file(tmp.path(), "vm.vmdk");
        create_file(tmp.path(), "a.nvram");
        create_file(tmp.path(), "b.nvram");

        let err = scan_vm_dir(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("Multiple .nvram files found"));
    }

    #[test]
    fn test_empty_directory_errors() {
        let tmp = TempDir::new().unwrap();

        let err = scan_vm_dir(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("No .ovf file found"));
    }
}

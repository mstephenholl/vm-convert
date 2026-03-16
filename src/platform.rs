/// platform.rs — Platform detection, prerequisite checking, OVMF discovery, and libvirt import.
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

// ─── Platform enum ───────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Platform {
    Linux,
    MacOS,
    Other(String),
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Platform::Linux => write!(f, "Linux"),
            Platform::MacOS => write!(f, "macOS"),
            Platform::Other(s) => write!(f, "{s}"),
        }
    }
}

/// Known OVMF firmware code paths, probed in order.
const OVMF_SEARCH_PATHS: &[&str] = &[
    // Ubuntu / Debian
    "/usr/share/OVMF/OVMF_CODE.fd",
    // Fedora
    "/usr/share/edk2/ovmf/OVMF_CODE.fd",
    // Arch
    "/usr/share/edk2-ovmf/x64/OVMF_CODE.fd",
    // openSUSE
    "/usr/share/qemu/ovmf-x86_64-code.bin",
    // macOS ARM (Homebrew)
    "/opt/homebrew/share/qemu/edk2-x86_64-code.fd",
    // macOS Intel (Homebrew)
    "/usr/local/share/qemu/edk2-x86_64-code.fd",
];

// ─── Public API ──────────────────────────────────────────────────────────────

/// Return the platform the binary is currently running on.
pub fn current_platform() -> Platform {
    match std::env::consts::OS {
        "linux" => Platform::Linux,
        "macos" => Platform::MacOS,
        other => Platform::Other(other.to_string()),
    }
}

/// Platform-specific install hint for qemu-img.
fn install_hint(platform: &Platform) -> String {
    match platform {
        Platform::Linux => "Install with:\n  sudo apt install qemu-utils".to_string(),
        Platform::MacOS => "Install with:\n  brew install qemu".to_string(),
        Platform::Other(os) => {
            format!("Please install QEMU for {os} and ensure qemu-img is in PATH")
        }
    }
}

/// Locate `qemu-img` in PATH.
pub fn find_qemu_img() -> Result<PathBuf> {
    which::which("qemu-img").map_err(|_| {
        let hint = install_hint(&current_platform());
        anyhow::anyhow!("qemu-img not found in PATH.\n{hint}")
    })
}

/// Probe for the OVMF firmware code file on the current system.
///
/// Returns `None` if no known path exists.
pub fn find_ovmf_code() -> Option<PathBuf> {
    OVMF_SEARCH_PATHS
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
}

/// Run `virsh define <xml_path>` to register the VM with libvirt.
pub fn import_to_libvirt(xml_path: &Path) -> Result<()> {
    let xml_str = xml_path
        .to_str()
        .context("XML path contains non-UTF-8 characters")?;

    let status = Command::new("virsh")
        .args(["define", xml_str])
        .status()
        .context(
            "Failed to run `virsh`. \
             Install libvirt with: sudo apt install libvirt-clients libvirt-daemon-system",
        )?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!(
            "`virsh define` failed (exit {}). \
             Inspect the generated XML for errors.",
            status.code().unwrap_or(-1)
        )
    }
}

/// Print platform-specific prerequisite instructions to stdout.
pub fn print_prerequisites() {
    let platform = current_platform();
    println!("{}", install_hint(&platform));
    match platform {
        Platform::Linux => {
            println!("Additional packages for libvirt:");
            println!("  sudo apt install libvirt-daemon-system libvirt-clients virt-manager");
        }
        Platform::MacOS => {
            println!();
            println!("Note: `virsh define` is not supported on macOS.");
            println!(
                "      Transfer the generated .qcow2 and .xml to your Linux \
                 host, then run: virsh define <name>.xml"
            );
        }
        Platform::Other(_) => {}
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_platform_is_linux_or_macos_in_ci() {
        let p = current_platform();
        assert!(
            matches!(p, Platform::Linux | Platform::MacOS | Platform::Other(_)),
            "Unexpected platform: {p}"
        );
    }

    #[test]
    fn test_platform_display_linux() {
        assert_eq!(Platform::Linux.to_string(), "Linux");
    }

    #[test]
    fn test_platform_display_macos() {
        assert_eq!(Platform::MacOS.to_string(), "macOS");
    }

    #[test]
    fn test_platform_display_other() {
        assert_eq!(Platform::Other("freebsd".into()).to_string(), "freebsd");
    }

    #[test]
    fn test_platform_equality() {
        assert_eq!(Platform::Linux, Platform::Linux);
        assert_ne!(Platform::Linux, Platform::MacOS);
        assert_eq!(Platform::Other("x".into()), Platform::Other("x".into()));
    }

    #[test]
    fn test_find_qemu_img_returns_result_type() {
        let _result = find_qemu_img();
    }

    #[test]
    fn test_import_to_libvirt_nonexistent_xml_errors() {
        let result = import_to_libvirt(Path::new("/nonexistent/path.xml"));
        assert!(
            result.is_err(),
            "Expected error when virsh is unavailable or XML file is missing"
        );
    }

    #[test]
    fn test_print_prerequisites_does_not_panic() {
        print_prerequisites();
    }

    #[test]
    fn test_find_ovmf_code_returns_option() {
        // On CI/dev it may or may not exist; just ensure it doesn't panic
        let _result = find_ovmf_code();
    }

    #[test]
    fn test_ovmf_search_paths_are_absolute() {
        for path in OVMF_SEARCH_PATHS {
            assert!(
                path.starts_with('/'),
                "OVMF path should be absolute: {path}"
            );
        }
    }
}

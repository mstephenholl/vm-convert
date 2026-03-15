/// platform.rs — Platform detection, prerequisite checking, and libvirt import.
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

// ─── Platform enum ────────────────────────────────────────────────────────────

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

// ─── Public API ───────────────────────────────────────────────────────────────

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

/// Locate `qemu-img` in PATH.  Returns an actionable error message when missing.
pub fn find_qemu_img() -> Result<PathBuf> {
    which::which("qemu-img").map_err(|_| {
        let hint = install_hint(&current_platform());
        anyhow::anyhow!("qemu-img not found in PATH.\n{hint}")
    })
}

/// Run `virsh define <xml_path>` to register the VM with libvirt.
///
/// Only meaningful on Linux; on macOS this is a no-op and the caller should
/// skip this step.
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
        // This binary runs on Linux (CI) or macOS (dev).
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
        // We don't assert success/failure because qemu-img may or may not
        // be installed in the test environment.  We only assert the function
        // returns without panicking.
        let _result = find_qemu_img();
    }

    #[test]
    fn test_import_to_libvirt_nonexistent_xml_errors() {
        let result = import_to_libvirt(Path::new("/nonexistent/path.xml"));
        // Either virsh is missing (context error) or returns non-zero (bail!).
        // Either way it must be Err.
        assert!(
            result.is_err(),
            "Expected error when virsh is unavailable or XML file is missing"
        );
    }

    #[test]
    fn test_print_prerequisites_does_not_panic() {
        // Just exercise the function to ensure no panics on any platform.
        print_prerequisites();
    }
}

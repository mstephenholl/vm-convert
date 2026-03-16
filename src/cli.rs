use clap::Parser;
use std::path::PathBuf;

/// Convert VMware OVF/VMDK images to QEMU/KVM format for use with
/// Virtual Machine Manager (virt-manager) on Linux, or as portable
/// QCOW2 + libvirt XML on macOS.
#[derive(Parser, Debug)]
#[command(
    name    = "vm-convert",
    version,
    about   = "Convert VMware OVF/VMDK → QEMU/KVM (libvirt)",
    long_about = None
)]
pub struct Args {
    /// Path to a VM export folder (.ovf + disks) or a compressed archive
    /// (.ova, .tar, .tar.gz/.tgz, .tar.bz2/.tbz2, .tar.xz/.txz,
    /// .tar.zst/.tzst, .zip)
    pub input: PathBuf,

    /// Output directory for converted .qcow2 and .xml files
    /// [default: same directory as the VM folder]
    #[arg(short = 'o', long)]
    pub output_dir: Option<PathBuf>,

    /// Override the VM name extracted from the OVF metadata
    #[arg(short = 'n', long)]
    pub name: Option<String>,

    /// Skip automatic libvirt import (just produce the XML file)
    #[arg(long, default_value_t = false)]
    pub no_import: bool,

    /// Disk output format passed to qemu-img -O
    #[arg(long, default_value = "qcow2",
          value_parser = ["qcow2", "raw"],
          help = "Output disk format [qcow2 | raw]")]
    pub format: String,

    /// Skip .mf manifest verification (if a manifest file is present)
    #[arg(long, default_value_t = false)]
    pub skip_verify: bool,

    /// Override all disk bus types to VirtIO (ignore OVF controller mappings)
    #[arg(long, default_value_t = false)]
    pub force_virtio: bool,

    /// Path to the OVMF firmware code file (overrides auto-detection)
    #[arg(long, value_name = "PATH")]
    pub ovmf_code: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_format_is_qcow2() {
        let args = Args::parse_from(["vm-convert", "/tmp/myvm"]);
        assert_eq!(args.format, "qcow2");
    }

    #[test]
    fn test_no_import_flag() {
        let args = Args::parse_from(["vm-convert", "--no-import", "/tmp/myvm"]);
        assert!(args.no_import);
    }

    #[test]
    fn test_name_override() {
        let args = Args::parse_from(["vm-convert", "--name", "my-vm", "/tmp/myvm"]);
        assert_eq!(args.name.as_deref(), Some("my-vm"));
    }

    #[test]
    fn test_output_dir_optional() {
        let args = Args::parse_from(["vm-convert", "/tmp/myvm"]);
        assert!(args.output_dir.is_none());
    }

    #[test]
    fn test_output_dir_set() {
        let args = Args::parse_from(["vm-convert", "--output-dir", "/tmp/output", "/tmp/myvm"]);
        assert_eq!(
            args.output_dir.as_deref(),
            Some(std::path::Path::new("/tmp/output"))
        );
    }

    #[test]
    fn test_skip_verify_flag() {
        let args = Args::parse_from(["vm-convert", "--skip-verify", "/tmp/myvm"]);
        assert!(args.skip_verify);
    }

    #[test]
    fn test_force_virtio_flag() {
        let args = Args::parse_from(["vm-convert", "--force-virtio", "/tmp/myvm"]);
        assert!(args.force_virtio);
    }

    #[test]
    fn test_ovmf_code_flag() {
        let args = Args::parse_from([
            "vm-convert",
            "--ovmf-code",
            "/custom/OVMF_CODE.fd",
            "/tmp/myvm",
        ]);
        assert_eq!(
            args.ovmf_code.as_deref(),
            Some(std::path::Path::new("/custom/OVMF_CODE.fd"))
        );
    }

    #[test]
    fn test_input_accepts_ova_path() {
        let args = Args::parse_from(["vm-convert", "/tmp/myvm.ova"]);
        assert_eq!(args.input, PathBuf::from("/tmp/myvm.ova"));
    }
}

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
    /// Path to the .ovf file (the .vmdk must be in the same directory)
    pub ovf_file: PathBuf,

    /// Output directory for converted .qcow2 and .xml files
    /// [default: same directory as the .ovf file]
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_format_is_qcow2() {
        let args = Args::parse_from(["vm-convert", "/tmp/test.ovf"]);
        assert_eq!(args.format, "qcow2");
    }

    #[test]
    fn test_no_import_flag() {
        let args = Args::parse_from(["vm-convert", "--no-import", "/tmp/test.ovf"]);
        assert!(args.no_import);
    }

    #[test]
    fn test_name_override() {
        let args =
            Args::parse_from(["vm-convert", "--name", "my-vm", "/tmp/test.ovf"]);
        assert_eq!(args.name.as_deref(), Some("my-vm"));
    }

    #[test]
    fn test_output_dir_optional() {
        let args = Args::parse_from(["vm-convert", "/tmp/test.ovf"]);
        assert!(args.output_dir.is_none());
    }

    #[test]
    fn test_output_dir_set() {
        let args = Args::parse_from([
            "vm-convert",
            "--output-dir", "/tmp/output",
            "/tmp/test.ovf",
        ]);
        assert_eq!(
            args.output_dir.as_deref(),
            Some(std::path::Path::new("/tmp/output"))
        );
    }
}

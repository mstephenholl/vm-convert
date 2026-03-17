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

    /// Compress output qcow2 disk images (reduces file size, especially
    /// when converting from compressed VMDK formats)
    #[arg(short = 'c', long)]
    pub compress: bool,

    /// Enable out-of-order writes for faster conversion (qemu-img -W)
    #[arg(short = 'W', long)]
    pub parallel_writes: bool,

    /// Number of parallel coroutines for conversion (qemu-img -m)
    #[arg(short = 'm', long, value_name = "N")]
    pub coroutines: Option<u32>,

    /// Target cache mode for conversion (qemu-img -t)
    /// [none | writeback | writethrough | unsafe]
    #[arg(short = 't', long, value_name = "MODE",
          value_parser = ["none", "writeback", "writethrough", "unsafe"])]
    pub target_cache: Option<String>,

    /// Path to the OVMF firmware code file (overrides auto-detection)
    #[arg(long, value_name = "PATH")]
    pub ovmf_code: Option<PathBuf>,

    /// Disable USB controller and SPICE USB redirection
    /// (USB 3.0 passthrough via qemu-xhci is enabled by default)
    #[arg(long, default_value_t = false)]
    pub no_usb: bool,
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
    fn test_compress_flag() {
        let args = Args::parse_from(["vm-convert", "--compress", "/tmp/myvm"]);
        assert!(args.compress);
    }

    #[test]
    fn test_compress_short_flag() {
        let args = Args::parse_from(["vm-convert", "-c", "/tmp/myvm"]);
        assert!(args.compress);
    }

    #[test]
    fn test_compress_default_false() {
        let args = Args::parse_from(["vm-convert", "/tmp/myvm"]);
        assert!(!args.compress);
    }

    #[test]
    fn test_input_accepts_ova_path() {
        let args = Args::parse_from(["vm-convert", "/tmp/myvm.ova"]);
        assert_eq!(args.input, PathBuf::from("/tmp/myvm.ova"));
    }

    #[test]
    fn test_parallel_writes_flag() {
        let args = Args::parse_from(["vm-convert", "-W", "/tmp/myvm"]);
        assert!(args.parallel_writes);
    }

    #[test]
    fn test_parallel_writes_long_flag() {
        let args = Args::parse_from(["vm-convert", "--parallel-writes", "/tmp/myvm"]);
        assert!(args.parallel_writes);
    }

    #[test]
    fn test_parallel_writes_default_false() {
        let args = Args::parse_from(["vm-convert", "/tmp/myvm"]);
        assert!(!args.parallel_writes);
    }

    #[test]
    fn test_coroutines_flag() {
        let args = Args::parse_from(["vm-convert", "-m", "16", "/tmp/myvm"]);
        assert_eq!(args.coroutines, Some(16));
    }

    #[test]
    fn test_coroutines_default_none() {
        let args = Args::parse_from(["vm-convert", "/tmp/myvm"]);
        assert!(args.coroutines.is_none());
    }

    #[test]
    fn test_target_cache_flag() {
        let args = Args::parse_from(["vm-convert", "-t", "none", "/tmp/myvm"]);
        assert_eq!(args.target_cache.as_deref(), Some("none"));
    }

    #[test]
    fn test_target_cache_writeback() {
        let args = Args::parse_from(["vm-convert", "--target-cache", "writeback", "/tmp/myvm"]);
        assert_eq!(args.target_cache.as_deref(), Some("writeback"));
    }

    #[test]
    fn test_target_cache_default_none() {
        let args = Args::parse_from(["vm-convert", "/tmp/myvm"]);
        assert!(args.target_cache.is_none());
    }

    #[test]
    fn test_no_usb_flag() {
        let args = Args::parse_from(["vm-convert", "--no-usb", "/tmp/myvm"]);
        assert!(args.no_usb);
    }

    #[test]
    fn test_usb_enabled_by_default() {
        let args = Args::parse_from(["vm-convert", "/tmp/myvm"]);
        assert!(!args.no_usb);
    }
}

mod cli;
mod convert;
mod inventory;
mod libvirt_xml;
mod ovf;
mod platform;

use anyhow::{Context, Result};
use clap::Parser;
use cli::Args;
use std::path::PathBuf;

fn main() -> Result<()> {
    let args = Args::parse();
    run(args)
}

/// Core logic, extracted so it can be called from integration tests.
fn run(args: Args) -> Result<()> {
    banner();

    // ── Step 1: Scan VM folder ────────────────────────────────────────────────
    let inv = inventory::scan_vm_dir(&args.vm_dir)?;
    println!("✓ VM folder : {}", args.vm_dir.display());
    println!(
        "  .ovf      : {}",
        inv.ovf_path.file_name().unwrap_or_default().to_string_lossy()
    );
    println!(
        "  .vmdk     : {} file(s)",
        inv.vmdk_paths.len()
    );
    if let Some(ref nvram) = inv.nvram_path {
        println!(
            "  .nvram    : {}",
            nvram.file_name().unwrap_or_default().to_string_lossy()
        );
    }
    divider();

    // ── Step 2: Locate qemu-img ───────────────────────────────────────────────
    let qemu_img = platform::find_qemu_img().inspect_err(|_e| {
        platform::print_prerequisites();
    })?;
    println!("✓ qemu-img  : {}", qemu_img.display());

    // ── Step 3: Validate & parse OVF ──────────────────────────────────────────
    let vm_config = ovf::parse_ovf(&inv.ovf_path, inv.has_nvram())?;
    let vm_name = args.name.unwrap_or_else(|| vm_config.name.clone());

    println!("✓ OVF parsed");
    println!("  Name     : {vm_name}");
    println!("  vCPUs    : {}", vm_config.vcpu_count);
    println!("  RAM      : {} MiB", vm_config.memory_mb);
    println!("  Disk src : {}", vm_config.disk_file);
    println!(
        "  Firmware : {}",
        if vm_config.uefi {
            "UEFI (OVMF)"
        } else {
            "BIOS"
        }
    );
    println!("  NICs     : {}", vm_config.nic_count);
    divider();

    // ── Step 4: Resolve paths ─────────────────────────────────────────────────
    let vm_dir = &args.vm_dir;
    let output_dir: PathBuf = args.output_dir.unwrap_or_else(|| vm_dir.to_path_buf());

    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("Cannot create output directory: {}", output_dir.display()))?;

    // Cross-validate: the VMDK referenced in the OVF must exist in the folder
    let vmdk_path = vm_dir.join(&vm_config.disk_file);
    if !vmdk_path.exists() {
        anyhow::bail!(
            "OVF references disk '{}' but it was not found in {}",
            vm_config.disk_file,
            vm_dir.display()
        );
    }

    let qcow2_path = output_dir.join(format!("{vm_name}.qcow2"));
    let xml_path = output_dir.join(format!("{vm_name}.xml"));

    println!("Output dir : {}", output_dir.display());
    println!("VMDK source: {}", vmdk_path.display());
    println!("QCOW2 dest : {}", qcow2_path.display());
    divider();

    // ── Step 5: Convert disk ──────────────────────────────────────────────────
    convert::convert_disk(&qemu_img, &vmdk_path, &qcow2_path, &args.format)?;
    println!("✓ Disk converted → {}", qcow2_path.display());

    // ── Step 6: Generate libvirt XML ──────────────────────────────────────────
    let xml = libvirt_xml::generate(&vm_config, &vm_name, &qcow2_path)?;
    std::fs::write(&xml_path, &xml)
        .with_context(|| format!("Cannot write XML: {}", xml_path.display()))?;
    println!("✓ Libvirt XML → {}", xml_path.display());

    // ── Step 7: Optionally import to libvirt ──────────────────────────────────
    if args.no_import {
        divider();
        println!("Skipped libvirt import (--no-import).");
        println!("To import manually:");
        println!("  virsh define \"{}\"", xml_path.display());
    } else {
        match platform::current_platform() {
            platform::Platform::Linux => {
                print!("Importing to libvirt… ");
                platform::import_to_libvirt(&xml_path)?;
                println!("done ✓");
                divider();
                println!("Next steps:");
                println!("  Start VM : virsh start {vm_name}");
                println!("  Open GUI : virt-manager");
                println!("  Find IP  : virsh domifaddr {vm_name}");
            }
            platform::Platform::MacOS => {
                divider();
                println!("macOS detected — libvirt import skipped.");
                println!("Transfer both files to your Linux host, then:");
                println!(
                    "  virsh define \"{}\"",
                    xml_path.file_name().unwrap_or_default().to_string_lossy()
                );
            }
            platform::Platform::Other(ref os) => {
                divider();
                println!("Platform {os}: manual import required.");
                println!("  virsh define \"{}\"", xml_path.display());
            }
        }
    }

    divider();
    println!("✓ Conversion complete.");
    Ok(())
}

fn banner() {
    println!("vm-convert  ─  VMware OVF/VMDK → QEMU/KVM Converter");
    divider();
}

fn divider() {
    println!("{}", "─".repeat(54));
}

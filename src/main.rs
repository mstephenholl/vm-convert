mod archive;
mod cli;
mod convert;
mod inventory;
mod libvirt_xml;
mod manifest;
mod ovf;
mod platform;

use anyhow::{Context, Result};
use clap::Parser;
use cli::Args;
use ovf::DiskBus;
use std::path::PathBuf;

fn main() -> Result<()> {
    let args = Args::parse();
    run(args)
}

/// Core logic, extracted so it can be called from integration tests.
fn run(args: Args) -> Result<()> {
    banner();

    // ── Step 0: Archive extraction ──────────────────────────────────────────────
    // If the input is a recognised archive, extract it to a temp directory.
    // Keep _archive_tmp alive so the tempdir isn't dropped until run() returns.
    let (_archive_tmp, vm_dir): (Option<tempfile::TempDir>, PathBuf) =
        if let Some(format) = archive::detect_format(&args.input) {
            println!("Extracting {} archive…", format.label());
            let tmp =
                tempfile::tempdir().context("Cannot create temporary directory for extraction")?;
            archive::extract_archive(&args.input, format, tmp.path())?;
            println!("✓ {} extracted to temp dir", format.label());
            divider();
            let dir = tmp.path().to_path_buf();
            (Some(tmp), dir)
        } else {
            (None, args.input.clone())
        };

    // ── Step 1: Scan VM folder ────────────────────────────────────────────────
    let inv = inventory::scan_vm_dir(&vm_dir)?;
    println!("✓ VM folder : {}", vm_dir.display());
    println!(
        "  .ovf      : {}",
        inv.ovf_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    );
    println!("  disks     : {} file(s)", inv.disk_paths.len());
    if let Some(ref nvram) = inv.nvram_path {
        println!(
            "  .nvram    : {}",
            nvram.file_name().unwrap_or_default().to_string_lossy()
        );
    }
    if let Some(ref mf) = inv.mf_path {
        println!(
            "  .mf       : {}",
            mf.file_name().unwrap_or_default().to_string_lossy()
        );
    }
    if !inv.iso_paths.is_empty() {
        println!("  .iso      : {} file(s)", inv.iso_paths.len());
    }
    divider();

    // ── Step 1b: Manifest verification ────────────────────────────────────────
    if let Some(ref mf_path) = inv.mf_path {
        if args.skip_verify {
            println!("⊘ Manifest verification skipped (--skip-verify)");
        } else {
            print!("Verifying manifest… ");
            manifest::verify_manifest(mf_path, &vm_dir)?;
            println!("✓ all hashes match");
        }
        divider();
    }

    // ── Step 2: Locate qemu-img ───────────────────────────────────────────────
    let qemu_img = platform::find_qemu_img().inspect_err(|_e| {
        platform::print_prerequisites();
    })?;
    println!("✓ qemu-img  : {}", qemu_img.display());

    // ── Step 3: Validate & parse OVF ──────────────────────────────────────────
    let mut vm_config = ovf::parse_ovf(&inv.ovf_path, inv.has_nvram())?;
    let vm_name = args.name.unwrap_or_else(|| vm_config.name.clone());

    // Apply --force-virtio to all disks
    if args.force_virtio {
        for disk in &mut vm_config.disks {
            disk.bus = DiskBus::Virtio;
        }
    }

    println!("✓ OVF parsed");
    println!("  Name     : {vm_name}");
    println!("  vCPUs    : {}", vm_config.vcpu_count);
    println!("  RAM      : {} MiB", vm_config.memory_mb);
    for disk in &vm_config.disks {
        println!("  Disk src : {} ({})", disk.href, disk.input_format);
    }
    println!(
        "  Firmware : {}",
        if vm_config.uefi {
            "UEFI (OVMF)"
        } else {
            "BIOS"
        }
    );
    println!("  NICs     : {}", vm_config.nic_count());
    if !vm_config.iso_files.is_empty() {
        for iso in &vm_config.iso_files {
            println!("  ISO      : {iso}");
        }
    }
    divider();

    // ── Step 4: Resolve paths ─────────────────────────────────────────────────
    let output_dir: PathBuf = args.output_dir.unwrap_or_else(|| vm_dir.to_path_buf());

    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("Cannot create output directory: {}", output_dir.display()))?;

    // Cross-validate: every disk referenced in the OVF must exist in the folder
    let mut disk_pairs: Vec<(PathBuf, PathBuf, &str)> = Vec::new();
    for (i, disk_ref) in vm_config.disks.iter().enumerate() {
        let disk_path = vm_dir.join(&disk_ref.href);
        if !disk_path.exists() {
            anyhow::bail!(
                "OVF references disk '{}' but it was not found in {}",
                disk_ref.href,
                vm_dir.display()
            );
        }
        let suffix = if i == 0 {
            String::new()
        } else {
            format!("_{i}")
        };
        let qcow2_path = output_dir.join(format!("{vm_name}{suffix}.qcow2"));
        disk_pairs.push((
            disk_path,
            qcow2_path,
            disk_ref.input_format.qemu_format_str(),
        ));
    }

    let xml_path = output_dir.join(format!("{vm_name}.xml"));

    println!("Output dir : {}", output_dir.display());
    for (src, dst, _) in &disk_pairs {
        println!(
            "  {} → {}",
            src.file_name().unwrap_or_default().to_string_lossy(),
            dst.file_name().unwrap_or_default().to_string_lossy()
        );
    }
    divider();

    // ── Step 5: Convert disks ─────────────────────────────────────────────────
    for (input_path, qcow2_path, input_format) in &disk_pairs {
        convert::convert_disk(
            &qemu_img,
            input_path,
            qcow2_path,
            input_format,
            &args.format,
            args.compress,
        )?;
        println!("✓ Disk converted → {}", qcow2_path.display());
    }

    // ── Step 5b: Copy NVRAM file ──────────────────────────────────────────────
    let nvram_output_path = if let Some(ref nvram_src) = inv.nvram_path {
        let nvram_name = nvram_src
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let nvram_dst = output_dir.join(&nvram_name);
        if nvram_src != &nvram_dst {
            std::fs::copy(nvram_src, &nvram_dst).with_context(|| {
                format!(
                    "Failed to copy NVRAM: {} → {}",
                    nvram_src.display(),
                    nvram_dst.display()
                )
            })?;
            println!("✓ NVRAM copied → {}", nvram_dst.display());
        }
        Some(nvram_dst)
    } else {
        None
    };

    // ── Step 5c: Copy ISO files ───────────────────────────────────────────────
    let mut iso_output_paths: Vec<PathBuf> = Vec::new();
    for iso_name in &vm_config.iso_files {
        let iso_src = vm_dir.join(iso_name);
        if iso_src.exists() {
            let iso_dst = output_dir.join(iso_name);
            if iso_src != iso_dst {
                std::fs::copy(&iso_src, &iso_dst).with_context(|| {
                    format!(
                        "Failed to copy ISO: {} → {}",
                        iso_src.display(),
                        iso_dst.display()
                    )
                })?;
                println!("✓ ISO copied → {}", iso_dst.display());
            }
            iso_output_paths.push(iso_dst);
        }
    }

    // ── Step 6: Resolve OVMF path ─────────────────────────────────────────────
    let ovmf_code_path: Option<PathBuf> = args.ovmf_code.or_else(platform::find_ovmf_code);

    // ── Step 7: Generate libvirt XML ──────────────────────────────────────────
    let qcow2_paths: Vec<&PathBuf> = disk_pairs.iter().map(|(_, q, _)| q).collect();
    let xml = libvirt_xml::generate(
        &vm_config,
        &vm_name,
        &qcow2_paths,
        nvram_output_path.as_deref(),
        ovmf_code_path.as_deref(),
        &iso_output_paths,
        args.force_virtio,
    )?;
    std::fs::write(&xml_path, &xml)
        .with_context(|| format!("Cannot write XML: {}", xml_path.display()))?;
    println!("✓ Libvirt XML → {}", xml_path.display());

    // ── Step 8: Optionally import to libvirt ──────────────────────────────────
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
    println!("vm-convert  —  OVF/OVA → QEMU/KVM Converter");
    divider();
}

fn divider() {
    println!("{}", "─".repeat(54));
}

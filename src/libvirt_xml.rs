/// libvirt_xml.rs — Generate a libvirt domain XML definition from a VmConfig.
///
/// Key decisions reflected in the generated XML:
///  - machine type "q35"  — modern PCIe bus, required for UEFI/OVMF
///  - cpu mode "host-passthrough" — best performance on KVM
///  - Bus type per disk from OVF controller mapping (VirtIO, SCSI, SATA, IDE)
///  - SPICE graphics + QXL video — works with virt-viewer and virt-manager
///  - OVMF (UEFI) paths: CLI override → auto-detect → Ubuntu fallback
///  - PCI addresses omitted for dynamic devices — libvirt auto-assigns them
///    and creates PCIe root ports as needed on the q35 bus topology
use crate::ovf::{DiskBus, NicDef, VmConfig};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Default OVMF code path (Ubuntu/Debian packaging).
const DEFAULT_OVMF_CODE: &str = "/usr/share/OVMF/OVMF_CODE.fd";

// ─── Public API ──────────────────────────────────────────────────────────────

/// Generate a libvirt domain XML string for the given VM.
///
/// * `vm_name`        — the domain name (may differ from config.name via --name flag)
/// * `qcow2_paths`    — absolute paths to the converted disk images
/// * `nvram_path`     — path to the NVRAM file (copied to output dir), or None for default
/// * `ovmf_code_path` — path to the OVMF_CODE firmware, or None for default
/// * `iso_paths`      — paths to ISO files for CD-ROM passthrough
/// * `force_virtio`   — override all disk bus types to VirtIO
pub fn generate(
    config: &VmConfig,
    vm_name: &str,
    qcow2_paths: &[&PathBuf],
    nvram_path: Option<&Path>,
    ovmf_code_path: Option<&Path>,
    iso_paths: &[PathBuf],
    force_virtio: bool,
) -> Result<String> {
    let os_block = build_os_block(config, vm_name, nvram_path, ovmf_code_path);

    // Check if any disk uses SCSI bus (need a controller)
    let needs_scsi = !force_virtio && config.disks.iter().any(|d| d.bus == DiskBus::Scsi);

    let disks = build_disk_devices(config, qcow2_paths, force_virtio)?;
    let cdroms = build_cdrom_devices(iso_paths)?;
    let networks = build_network_interfaces(&config.nics);

    let scsi_controller = if needs_scsi {
        r#"    <controller type="scsi" index="0" model="virtio-scsi"/>
"#
        .to_string()
    } else {
        String::new()
    };

    let xml = format!(
        r#"<domain type="kvm">
  <name>{vm_name}</name>
  <memory unit="MiB">{memory}</memory>
  <currentMemory unit="MiB">{memory}</currentMemory>
  <vcpu placement="static">{vcpu}</vcpu>
{os_block}
  <features>
    <acpi/>
    <apic/>
    <vmport state="off"/>
  </features>
  <cpu mode="host-passthrough" check="none" migratable="on"/>
  <clock offset="utc">
    <timer name="rtc" tickpolicy="catchup"/>
    <timer name="pit" tickpolicy="delay"/>
    <timer name="hpet" present="no"/>
  </clock>
  <on_poweroff>destroy</on_poweroff>
  <on_reboot>restart</on_reboot>
  <on_crash>destroy</on_crash>
  <pm>
    <suspend-to-mem enabled="no"/>
    <suspend-to-disk enabled="no"/>
  </pm>
  <devices>
    <emulator>/usr/bin/qemu-system-x86_64</emulator>
{disks}
{cdroms}{scsi_controller}    <controller type="virtio-serial" index="0"/>
{networks}
    <serial type="pty">
      <target type="isa-serial" port="0">
        <model name="isa-serial"/>
      </target>
    </serial>
    <console type="pty">
      <target type="serial" port="0"/>
    </console>
    <channel type="unix">
      <target type="virtio" name="org.qemu.guest_agent.0"/>
    </channel>
    <input type="tablet" bus="usb"/>
    <graphics type="spice" autoport="yes">
      <listen type="address"/>
      <image compression="off"/>
    </graphics>
    <sound model="ich9"/>
    <video>
      <model type="qxl" ram="65536" vram="65536" vgamem="16384" heads="1" primary="yes"/>
    </video>
    <memballoon model="virtio"/>
    <rng model="virtio">
      <backend model="random">/dev/urandom</backend>
    </rng>
  </devices>
</domain>
"#,
        vm_name = vm_name,
        memory = config.memory_mb,
        vcpu = config.vcpu_count,
        os_block = os_block,
        disks = disks,
        cdroms = cdroms,
        scsi_controller = scsi_controller,
        networks = networks,
    );

    Ok(xml)
}

// ─── Internal helpers ────────────────────────────────────────────────────────

fn build_os_block(
    config: &VmConfig,
    vm_name: &str,
    nvram_path: Option<&Path>,
    ovmf_code_path: Option<&Path>,
) -> String {
    if config.uefi {
        let loader = ovmf_code_path
            .and_then(|p| p.to_str())
            .unwrap_or(DEFAULT_OVMF_CODE);

        let nvram_str = match nvram_path {
            Some(p) => p.to_string_lossy().to_string(),
            None => format!("/var/lib/libvirt/qemu/nvram/{vm_name}_VARS.fd"),
        };

        format!(
            r#"  <os>
    <type arch="x86_64" machine="q35">hvm</type>
    <loader readonly="yes" type="pflash">{loader}</loader>
    <nvram>{nvram_str}</nvram>
  </os>"#
        )
    } else {
        r#"  <os>
    <type arch="x86_64" machine="q35">hvm</type>
  </os>"#
            .to_string()
    }
}

/// Generate `<disk>` elements with bus type from OVF controller mapping.
fn build_disk_devices(
    config: &VmConfig,
    qcow2_paths: &[&PathBuf],
    force_virtio: bool,
) -> Result<String> {
    // Track per-bus device letter counters
    let mut virtio_idx: u8 = 0; // vda, vdb, ...
    let mut sd_idx: u8 = 0; // sda, sdb, ... (scsi + sata)
    let mut hd_idx: u8 = 0; // hda, hdb, ... (ide)

    let mut blocks = Vec::new();
    for (i, path) in qcow2_paths.iter().enumerate() {
        let path_str = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("QCOW2 path contains non-UTF-8 characters"))?;

        let bus = if force_virtio {
            DiskBus::Virtio
        } else if i < config.disks.len() {
            config.disks[i].bus
        } else {
            DiskBus::Virtio
        };

        let (bus_str, dev_name) = match bus {
            DiskBus::Virtio => {
                let letter = (b'a' + virtio_idx) as char;
                virtio_idx += 1;
                ("virtio", format!("vd{letter}"))
            }
            DiskBus::Scsi => {
                let letter = (b'a' + sd_idx) as char;
                sd_idx += 1;
                ("scsi", format!("sd{letter}"))
            }
            DiskBus::Sata => {
                let letter = (b'a' + sd_idx) as char;
                sd_idx += 1;
                ("sata", format!("sd{letter}"))
            }
            DiskBus::Ide => {
                let letter = (b'a' + hd_idx) as char;
                hd_idx += 1;
                ("ide", format!("hd{letter}"))
            }
        };

        blocks.push(format!(
            r#"    <disk type="file" device="disk">
      <driver name="qemu" type="qcow2" discard="unmap"/>
      <source file="{path_str}"/>
      <target dev="{dev_name}" bus="{bus_str}"/>
    </disk>"#
        ));
    }
    Ok(blocks.join("\n"))
}

/// Generate `<disk device="cdrom">` elements for ISO files.
fn build_cdrom_devices(iso_paths: &[PathBuf]) -> Result<String> {
    if iso_paths.is_empty() {
        return Ok(String::new());
    }

    let mut blocks = Vec::new();
    for (i, path) in iso_paths.iter().enumerate() {
        let path_str = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("ISO path contains non-UTF-8 characters"))?;

        // Use sr0, sr1, ... for CD-ROM device names
        let dev_name = format!("sr{i}");
        blocks.push(format!(
            r#"    <disk type="file" device="cdrom">
      <driver name="qemu" type="raw"/>
      <source file="{path_str}"/>
      <target dev="{dev_name}" bus="sata"/>
      <readonly/>
    </disk>"#
        ));
    }
    Ok(blocks.join("\n") + "\n")
}

/// Generate `<interface>` elements. Always emit at least one NIC.
fn build_network_interfaces(nics: &[NicDef]) -> String {
    let effective_nics: Vec<&NicDef> = if nics.is_empty() {
        // Default to one NIC with no MAC
        vec![]
    } else {
        nics.iter().collect()
    };

    let count = if effective_nics.is_empty() {
        1
    } else {
        effective_nics.len()
    };

    (0..count)
        .map(|i| {
            let mac_line = effective_nics
                .get(i)
                .and_then(|nic| nic.mac_address.as_deref())
                .map(|mac| format!("\n      <mac address=\"{mac}\"/>"))
                .unwrap_or_default();

            format!(
                r#"    <interface type="network">{mac_line}
      <source network="default"/>
      <model type="virtio"/>
    </interface>"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ovf::{DiskBus, DiskFormat, DiskRef, NicDef};
    use std::path::PathBuf;

    fn bios_config() -> VmConfig {
        VmConfig {
            name: "test-vm".into(),
            vcpu_count: 2,
            memory_mb: 2048,
            disks: vec![DiskRef {
                href: "disk.vmdk".into(),
                input_format: DiskFormat::Vmdk,
                bus: DiskBus::Virtio,
            }],
            nics: vec![NicDef { mac_address: None }],
            iso_files: vec![],
            uefi: false,
        }
    }

    fn uefi_config() -> VmConfig {
        VmConfig {
            name: "uefi-vm".into(),
            vcpu_count: 4,
            memory_mb: 8192,
            disks: vec![DiskRef {
                href: "disk.vmdk".into(),
                input_format: DiskFormat::Vmdk,
                bus: DiskBus::Virtio,
            }],
            nics: vec![NicDef { mac_address: None }, NicDef { mac_address: None }],
            iso_files: vec![],
            uefi: true,
        }
    }

    fn gen(cfg: &VmConfig, name: &str, paths: &[PathBuf]) -> String {
        let refs: Vec<&PathBuf> = paths.iter().collect();
        generate(cfg, name, &refs, None, None, &[], false).unwrap()
    }

    // ── Top-level structure ──────────────────────────────────────────────────

    #[test]
    fn test_xml_opens_and_closes_domain() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(xml.starts_with(r#"<domain type="kvm">"#));
        assert!(xml.trim_end().ends_with("</domain>"));
    }

    #[test]
    fn test_domain_name_element() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(xml.contains("<name>test-vm</name>"));
    }

    // ── Memory ──────────────────────────────────────────────────────────────

    #[test]
    fn test_memory_value() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(xml.contains(r#"<memory unit="MiB">2048</memory>"#));
        assert!(xml.contains(r#"<currentMemory unit="MiB">2048</currentMemory>"#));
    }

    // ── CPU ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_vcpu_value() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(xml.contains(r#"<vcpu placement="static">2</vcpu>"#));
    }

    // ── Disk ────────────────────────────────────────────────────────────────

    #[test]
    fn test_disk_source_path() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/var/lib/libvirt/images/test-vm.qcow2")],
        );
        assert!(xml.contains(r#"<source file="/var/lib/libvirt/images/test-vm.qcow2"/>"#));
    }

    #[test]
    fn test_disk_uses_virtio() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(xml.contains(r#"<target dev="vda" bus="virtio"/>"#));
    }

    #[test]
    fn test_multiple_disks() {
        let mut cfg = bios_config();
        cfg.disks.push(DiskRef {
            href: "data.vmdk".into(),
            input_format: DiskFormat::Vmdk,
            bus: DiskBus::Virtio,
        });
        let paths = [
            PathBuf::from("/tmp/os.qcow2"),
            PathBuf::from("/tmp/data.qcow2"),
        ];
        let refs: Vec<&PathBuf> = paths.iter().collect();
        let xml = generate(&cfg, "test-vm", &refs, None, None, &[], false).unwrap();
        assert!(xml.contains(r#"<source file="/tmp/os.qcow2"/>"#));
        assert!(xml.contains(r#"<target dev="vda" bus="virtio"/>"#));
        assert!(xml.contains(r#"<source file="/tmp/data.qcow2"/>"#));
        assert!(xml.contains(r#"<target dev="vdb" bus="virtio"/>"#));
        assert_eq!(xml.matches(r#"<disk type="file""#).count(), 2);
    }

    #[test]
    fn test_multiple_disks_no_explicit_pci_address() {
        let mut cfg = bios_config();
        cfg.disks.push(DiskRef {
            href: "data.vmdk".into(),
            input_format: DiskFormat::Vmdk,
            bus: DiskBus::Virtio,
        });
        let paths = [
            PathBuf::from("/tmp/os.qcow2"),
            PathBuf::from("/tmp/data.qcow2"),
        ];
        let refs: Vec<&PathBuf> = paths.iter().collect();
        let xml = generate(&cfg, "test-vm", &refs, None, None, &[], false).unwrap();
        // Disk elements should not contain explicit PCI addresses (libvirt auto-assigns)
        let disk_sections: Vec<&str> = xml.split("<disk ").skip(1).collect();
        for section in disk_sections {
            let disk_end = section.find("</disk>").unwrap_or(section.len());
            let disk_block = &section[..disk_end];
            assert!(
                !disk_block.contains(r#"address type="pci""#),
                "Disk should not have an explicit PCI address"
            );
        }
    }

    // ── Firmware ────────────────────────────────────────────────────────────

    #[test]
    fn test_bios_has_no_ovmf_loader() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(!xml.contains("OVMF_CODE.fd"));
        assert!(!xml.contains("nvram"));
    }

    #[test]
    fn test_uefi_has_ovmf_loader() {
        let xml = gen(
            &uefi_config(),
            "uefi-vm",
            &[PathBuf::from("/tmp/uefi.qcow2")],
        );
        assert!(xml.contains("/usr/share/OVMF/OVMF_CODE.fd"));
    }

    #[test]
    fn test_uefi_nvram_contains_vm_name() {
        let xml = gen(
            &uefi_config(),
            "uefi-vm",
            &[PathBuf::from("/tmp/uefi.qcow2")],
        );
        assert!(xml.contains("uefi-vm_VARS.fd"));
    }

    #[test]
    fn test_uefi_custom_nvram_path() {
        let cfg = uefi_config();
        let paths = [PathBuf::from("/tmp/uefi.qcow2")];
        let refs: Vec<&PathBuf> = paths.iter().collect();
        let nvram = Path::new("/output/custom.nvram");
        let xml = generate(&cfg, "uefi-vm", &refs, Some(nvram), None, &[], false).unwrap();
        assert!(xml.contains("/output/custom.nvram"));
    }

    #[test]
    fn test_uefi_custom_ovmf_code_path() {
        let cfg = uefi_config();
        let paths = [PathBuf::from("/tmp/uefi.qcow2")];
        let refs: Vec<&PathBuf> = paths.iter().collect();
        let ovmf = Path::new("/custom/OVMF_CODE.fd");
        let xml = generate(&cfg, "uefi-vm", &refs, None, Some(ovmf), &[], false).unwrap();
        assert!(xml.contains("/custom/OVMF_CODE.fd"));
        assert!(!xml.contains("/usr/share/OVMF/OVMF_CODE.fd"));
    }

    // ── Networking ──────────────────────────────────────────────────────────

    #[test]
    fn test_one_nic_produces_one_interface() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert_eq!(xml.matches(r#"<interface type="network">"#).count(), 1);
    }

    #[test]
    fn test_two_nics_produce_two_interfaces() {
        let xml = gen(
            &uefi_config(),
            "uefi-vm",
            &[PathBuf::from("/tmp/uefi.qcow2")],
        );
        assert_eq!(xml.matches(r#"<interface type="network">"#).count(), 2);
    }

    #[test]
    fn test_zero_nics_defaults_to_one_interface() {
        let mut cfg = bios_config();
        cfg.nics.clear();
        let xml = gen(&cfg, "test-vm", &[PathBuf::from("/tmp/test.qcow2")]);
        assert_eq!(xml.matches(r#"<interface type="network">"#).count(), 1);
    }

    #[test]
    fn test_interfaces_use_virtio_model() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(xml.contains(r#"<model type="virtio"/>"#));
    }

    // ── MAC address ─────────────────────────────────────────────────────────

    #[test]
    fn test_mac_address_in_xml() {
        let mut cfg = bios_config();
        cfg.nics = vec![NicDef {
            mac_address: Some("00:50:56:C0:00:01".into()),
        }];
        let xml = gen(&cfg, "test-vm", &[PathBuf::from("/tmp/test.qcow2")]);
        assert!(xml.contains(r#"<mac address="00:50:56:C0:00:01"/>"#));
    }

    #[test]
    fn test_no_mac_when_none() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(!xml.contains("<mac address="));
    }

    // ── Disk bus types ──────────────────────────────────────────────────────

    #[test]
    fn test_scsi_bus_in_xml() {
        let mut cfg = bios_config();
        cfg.disks[0].bus = DiskBus::Scsi;
        let paths = [PathBuf::from("/tmp/test.qcow2")];
        let refs: Vec<&PathBuf> = paths.iter().collect();
        let xml = generate(&cfg, "test-vm", &refs, None, None, &[], false).unwrap();
        assert!(xml.contains(r#"bus="scsi""#));
        assert!(xml.contains(r#"dev="sda""#));
        assert!(xml.contains(r#"<controller type="scsi" index="0" model="virtio-scsi"/>"#));
    }

    #[test]
    fn test_sata_bus_in_xml() {
        let mut cfg = bios_config();
        cfg.disks[0].bus = DiskBus::Sata;
        let xml = gen(&cfg, "test-vm", &[PathBuf::from("/tmp/test.qcow2")]);
        assert!(xml.contains(r#"bus="sata""#));
        assert!(xml.contains(r#"dev="sda""#));
    }

    #[test]
    fn test_ide_bus_in_xml() {
        let mut cfg = bios_config();
        cfg.disks[0].bus = DiskBus::Ide;
        let xml = gen(&cfg, "test-vm", &[PathBuf::from("/tmp/test.qcow2")]);
        assert!(xml.contains(r#"bus="ide""#));
        assert!(xml.contains(r#"dev="hda""#));
    }

    #[test]
    fn test_force_virtio_overrides_scsi() {
        let mut cfg = bios_config();
        cfg.disks[0].bus = DiskBus::Scsi;
        let paths = [PathBuf::from("/tmp/test.qcow2")];
        let refs: Vec<&PathBuf> = paths.iter().collect();
        let xml = generate(&cfg, "test-vm", &refs, None, None, &[], true).unwrap();
        assert!(xml.contains(r#"bus="virtio""#));
        assert!(!xml.contains(r#"bus="scsi""#));
    }

    // ── CD-ROM ──────────────────────────────────────────────────────────────

    #[test]
    fn test_cdrom_devices() {
        let cfg = bios_config();
        let disk_paths = [PathBuf::from("/tmp/test.qcow2")];
        let iso_paths = vec![PathBuf::from("/tmp/tools.iso")];
        let refs: Vec<&PathBuf> = disk_paths.iter().collect();
        let xml = generate(&cfg, "test-vm", &refs, None, None, &iso_paths, false).unwrap();
        assert!(xml.contains(r#"device="cdrom""#));
        assert!(xml.contains(r#"<source file="/tmp/tools.iso"/>"#));
        assert!(xml.contains(r#"bus="sata""#));
        assert!(xml.contains("<readonly/>"));
    }

    #[test]
    fn test_no_cdrom_when_no_isos() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(!xml.contains("cdrom"));
    }

    // ── Misc devices ────────────────────────────────────────────────────────

    #[test]
    fn test_spice_graphics_present() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(xml.contains(r#"<graphics type="spice""#));
    }

    #[test]
    fn test_machine_type_is_q35() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(xml.contains(r#"machine="q35""#));
    }

    #[test]
    fn test_cpu_host_passthrough() {
        let xml = gen(
            &bios_config(),
            "test-vm",
            &[PathBuf::from("/tmp/test.qcow2")],
        );
        assert!(xml.contains(r#"<cpu mode="host-passthrough""#));
    }

    // ── build_os_block ──────────────────────────────────────────────────────

    #[test]
    fn test_build_os_block_bios() {
        let cfg = bios_config();
        let block = build_os_block(&cfg, "test-vm", None, None);
        assert!(!block.contains("loader"));
        assert!(block.contains(r#"machine="q35""#));
    }

    #[test]
    fn test_build_os_block_uefi() {
        let cfg = uefi_config();
        let block = build_os_block(&cfg, "uefi-vm", None, None);
        assert!(block.contains("loader"));
        assert!(block.contains("uefi-vm_VARS.fd"));
    }
}

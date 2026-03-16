/// ovf.rs — Parse OVF XML to extract VM configuration.
///
/// Supports the full DMTF DSP0243 OVF specification including:
///   - Multiple disk formats (VMDK, VHD, VHDX, VDI, RAW, QCOW2)
///   - Disk controller mapping (IDE, SCSI, SATA → bus type)
///   - MAC address preservation
///   - ISO/CD-ROM references
///
/// CIM ResourceType values we care about:
///   3  = Processor (VirtualQuantity = vCPU count)
///   4  = Memory    (VirtualQuantity in AllocationUnits, default MiB)
///   5  = IDE Controller
///   6  = Parallel SCSI Controller
///   10 = Ethernet Adapter
///   17 = Disk Drive
///   20 = Other Storage Device (SATA Controller)
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;

// ─── Disk format ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskFormat {
    Vmdk,
    Vhd,
    Vhdx,
    Vdi,
    Raw,
    Qcow2,
}

impl DiskFormat {
    /// Detect format from OVF `ovf:format` URI.
    ///
    /// Known URIs include:
    ///   VMware:     `http://www.vmware.com/interfaces/specifications/vmdk.html#streamOptimized`
    ///   VirtualBox: `http://www.virtualbox.org/ovf/machine` (VDI referenced by extension)
    pub fn from_ovf_uri(uri: &str) -> Option<Self> {
        let u = uri.to_lowercase();
        if u.contains("vmdk") {
            Some(DiskFormat::Vmdk)
        } else if u.contains("vhdx") {
            Some(DiskFormat::Vhdx)
        } else if u.contains("vhd") {
            Some(DiskFormat::Vhd)
        } else if u.contains("vdi") {
            Some(DiskFormat::Vdi)
        } else if u.contains("raw") {
            Some(DiskFormat::Raw)
        } else if u.contains("qcow2") {
            Some(DiskFormat::Qcow2)
        } else {
            None
        }
    }

    /// Detect format from file extension.
    pub fn from_extension(filename: &str) -> Option<Self> {
        let lower = filename.to_lowercase();
        if lower.ends_with(".vmdk") {
            Some(DiskFormat::Vmdk)
        } else if lower.ends_with(".vhdx") {
            Some(DiskFormat::Vhdx)
        } else if lower.ends_with(".vhd") {
            Some(DiskFormat::Vhd)
        } else if lower.ends_with(".vdi") {
            Some(DiskFormat::Vdi)
        } else if lower.ends_with(".raw") || lower.ends_with(".img") {
            Some(DiskFormat::Raw)
        } else if lower.ends_with(".qcow2") {
            Some(DiskFormat::Qcow2)
        } else {
            None
        }
    }

    /// Format string for `qemu-img -f` flag.
    pub fn qemu_format_str(&self) -> &'static str {
        match self {
            DiskFormat::Vmdk => "vmdk",
            DiskFormat::Vhd | DiskFormat::Vhdx => "vpc",
            DiskFormat::Vdi => "vdi",
            DiskFormat::Raw => "raw",
            DiskFormat::Qcow2 => "qcow2",
        }
    }
}

impl std::fmt::Display for DiskFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.qemu_format_str())
    }
}

// ─── Disk bus ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskBus {
    Virtio,
    Ide,
    Scsi,
    Sata,
}

// ─── Disk reference ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct DiskRef {
    /// Relative path/href to the disk image file
    pub href: String,
    /// Detected input format
    pub input_format: DiskFormat,
    /// Bus type (from OVF controller mapping, default VirtIO)
    pub bus: DiskBus,
}

// ─── NIC definition ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct NicDef {
    /// MAC address if specified in the OVF
    pub mac_address: Option<String>,
}

// ─── Public data model ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct VmConfig {
    /// Sanitised VM name (safe for use as a filename / libvirt domain name)
    pub name: String,
    /// Number of virtual CPUs
    pub vcpu_count: u32,
    /// RAM in MiB
    pub memory_mb: u64,
    /// Disk image references with format and bus info
    pub disks: Vec<DiskRef>,
    /// Network interface definitions (with optional MAC)
    pub nics: Vec<NicDef>,
    /// Relative paths to .iso files referenced in OVF
    pub iso_files: Vec<String>,
    /// True when a .nvram sidecar file was present alongside the .ovf
    pub uefi: bool,
}

impl VmConfig {
    /// Number of NICs.
    pub fn nic_count(&self) -> usize {
        self.nics.len()
    }
}

// ─── Public API ──────────────────────────────────────────────────────────────

/// Parse an OVF file on disk.
pub fn parse_ovf(ovf_path: &Path, uefi: bool) -> Result<VmConfig> {
    let content = std::fs::read_to_string(ovf_path)
        .with_context(|| format!("Cannot read OVF file: {}", ovf_path.display()))?;

    parse_ovf_str(&content, uefi)
}

/// Parse OVF XML from a string (useful for tests).
pub fn parse_ovf_str(content: &str, uefi: bool) -> Result<VmConfig> {
    let doc = roxmltree::Document::parse(content).context("Failed to parse OVF XML")?;
    let root = doc.root_element();

    let name = extract_vm_name(&root).unwrap_or_else(|| "converted-vm".to_string());
    let mut disks = extract_disk_refs(&root);
    if disks.is_empty() {
        anyhow::bail!("OVF contains no disk file reference");
    }
    let hw = extract_hardware(&root)?;

    // Apply controller bus mappings to disks
    apply_bus_mappings(&root, &mut disks, &hw.controller_map);

    let iso_files = extract_iso_files(&root);

    Ok(VmConfig {
        name,
        vcpu_count: hw.vcpu_count,
        memory_mb: hw.memory_mb,
        disks,
        nics: hw.nics,
        iso_files,
        uefi,
    })
}

// ─── Internal helpers ────────────────────────────────────────────────────────

fn extract_vm_name<'a>(root: &'a roxmltree::Node<'a, 'a>) -> Option<String> {
    root.descendants()
        .find(|n| n.tag_name().name() == "Name")
        .and_then(|n| n.text())
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .map(sanitize_vm_name)
        .or_else(|| {
            root.descendants()
                .find(|n| n.tag_name().name() == "VirtualSystem")
                .and_then(|n| n.attributes().find(|a| a.name() == "id"))
                .map(|a| sanitize_vm_name(a.value()))
        })
}

/// Replace any character that is not alphanumeric, `-`, or `_` with `-`.
pub fn sanitize_vm_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Build `Vec<DiskRef>` from OVF References + DiskSection.
///
/// Resolution chain:
///   `File[@ovf:id, @ovf:href]` → `Disk[@ovf:fileRef, @ovf:diskId, @ovf:format]`
///
/// When no DiskSection exists, falls back to extension-based detection on File elements.
fn extract_disk_refs<'a>(root: &'a roxmltree::Node<'a, 'a>) -> Vec<DiskRef> {
    let disk_extensions = [".vmdk", ".vhd", ".vhdx", ".vdi", ".raw", ".img", ".qcow2"];

    // Step 1: Collect File elements → id → href
    let mut file_map: HashMap<String, String> = HashMap::new();
    // Preserve insertion order for deterministic output
    let mut file_order: Vec<String> = Vec::new();
    for node in root.descendants().filter(|n| n.tag_name().name() == "File") {
        if let (Some(id), Some(href)) = (
            node.attributes()
                .find(|a| a.name() == "id")
                .map(|a| a.value().to_string()),
            node.attributes()
                .find(|a| a.name() == "href")
                .map(|a| a.value().to_string()),
        ) {
            file_order.push(id.clone());
            file_map.insert(id, href);
        }
    }

    // Step 2: Collect Disk elements → fileRef + format URI
    let mut disk_entries: Vec<(String, Option<String>)> = Vec::new();
    let mut has_disk_section = false;
    for node in root.descendants().filter(|n| n.tag_name().name() == "Disk") {
        has_disk_section = true;
        let file_ref = node
            .attributes()
            .find(|a| a.name() == "fileRef")
            .map(|a| a.value().to_string());
        let format_uri = node
            .attributes()
            .find(|a| a.name() == "format")
            .map(|a| a.value().to_string());
        if let Some(fref) = file_ref {
            disk_entries.push((fref, format_uri));
        }
    }

    if has_disk_section && !disk_entries.is_empty() {
        // Resolve via DiskSection
        disk_entries
            .into_iter()
            .filter_map(|(file_ref, format_uri)| {
                let href = file_map.get(&file_ref)?;
                let format = format_uri
                    .as_deref()
                    .and_then(DiskFormat::from_ovf_uri)
                    .or_else(|| DiskFormat::from_extension(href))?;
                Some(DiskRef {
                    href: href.clone(),
                    input_format: format,
                    bus: DiskBus::Virtio,
                })
            })
            .collect()
    } else {
        // Fallback: grab disk files directly from References by extension
        file_order
            .iter()
            .filter_map(|id| {
                let href = file_map.get(id)?;
                let lower = href.to_lowercase();
                if !disk_extensions.iter().any(|ext| lower.ends_with(ext)) {
                    return None;
                }
                let format = DiskFormat::from_extension(href)?;
                Some(DiskRef {
                    href: href.clone(),
                    input_format: format,
                    bus: DiskBus::Virtio,
                })
            })
            .collect()
    }
}

/// Extract ISO file references from OVF References section.
fn extract_iso_files<'a>(root: &'a roxmltree::Node<'a, 'a>) -> Vec<String> {
    root.descendants()
        .filter(|n| n.tag_name().name() == "File")
        .filter_map(|n| {
            n.attributes()
                .find(|a| a.name() == "href")
                .filter(|a| a.value().to_lowercase().ends_with(".iso"))
                .map(|a| a.value().to_string())
        })
        .collect()
}

struct HardwareInfo {
    vcpu_count: u32,
    memory_mb: u64,
    nics: Vec<NicDef>,
    controller_map: HashMap<String, DiskBus>,
}

/// Extract hardware: CPU, memory, NICs (with MAC), and controller map.
fn extract_hardware<'a>(root: &'a roxmltree::Node<'a, 'a>) -> Result<HardwareInfo> {
    let mut vcpu_count: u32 = 1;
    let mut memory_mb: u64 = 1024;
    let mut nics: Vec<NicDef> = Vec::new();
    let mut controller_map: HashMap<String, DiskBus> = HashMap::new();

    let child_text = |item: &roxmltree::Node, tag: &str| -> Option<String> {
        item.descendants()
            .find(|n| n.tag_name().name() == tag)
            .and_then(|n| n.text())
            .map(|t| t.trim().to_string())
    };

    for item in root.descendants().filter(|n| n.tag_name().name() == "Item") {
        let resource_type: Option<u32> =
            child_text(&item, "ResourceType").and_then(|t| t.parse().ok());

        let virtual_quantity: Option<u64> =
            child_text(&item, "VirtualQuantity").and_then(|t| t.parse().ok());

        match resource_type {
            Some(3) => {
                // CPU
                if let Some(qty) = virtual_quantity {
                    vcpu_count = qty as u32;
                }
            }
            Some(4) => {
                // Memory
                if let Some(qty) = virtual_quantity {
                    let units = child_text(&item, "AllocationUnits")
                        .unwrap_or_else(|| "byte * 2^20".to_string());
                    memory_mb = normalize_memory_to_mib(qty, &units);
                }
            }
            Some(5) => {
                // IDE Controller
                if let Some(instance_id) = child_text(&item, "InstanceID") {
                    controller_map.insert(instance_id, DiskBus::Ide);
                }
            }
            Some(6) => {
                // SCSI Controller
                if let Some(instance_id) = child_text(&item, "InstanceID") {
                    controller_map.insert(instance_id, DiskBus::Scsi);
                }
            }
            Some(10) => {
                // Ethernet Adapter
                let mac_address = child_text(&item, "Address");
                nics.push(NicDef { mac_address });
            }
            Some(20) => {
                // SATA Controller (Other Storage Device)
                if let Some(instance_id) = child_text(&item, "InstanceID") {
                    controller_map.insert(instance_id, DiskBus::Sata);
                }
            }
            _ => {}
        }
    }

    Ok(HardwareInfo {
        vcpu_count,
        memory_mb,
        nics,
        controller_map,
    })
}

/// Apply bus mappings from controller items to disk refs.
///
/// For each disk drive item (ResourceType 17), looks up the Parent to find
/// the controller type, then matches the HostResource to associate with a DiskRef.
fn apply_bus_mappings(
    root: &roxmltree::Node,
    disks: &mut [DiskRef],
    controller_map: &HashMap<String, DiskBus>,
) {
    if controller_map.is_empty() {
        return;
    }

    let child_text = |item: &roxmltree::Node, tag: &str| -> Option<String> {
        item.descendants()
            .find(|n| n.tag_name().name() == tag)
            .and_then(|n| n.text())
            .map(|t| t.trim().to_string())
    };

    // Build diskId → bus mapping from disk drive items (RT 17)
    let mut disk_id_to_bus: HashMap<String, DiskBus> = HashMap::new();
    for item in root.descendants().filter(|n| n.tag_name().name() == "Item") {
        let resource_type: Option<u32> =
            child_text(&item, "ResourceType").and_then(|t| t.parse().ok());
        if resource_type == Some(17) {
            let parent = child_text(&item, "Parent");
            let host_resource = child_text(&item, "HostResource");
            if let (Some(parent_id), Some(hr)) = (parent, host_resource) {
                if let Some(&bus) = controller_map.get(&parent_id) {
                    // HostResource is typically "ovf:/disk/vmdisk1"
                    if let Some(disk_id) = hr.rsplit('/').next() {
                        disk_id_to_bus.insert(disk_id.to_string(), bus);
                    }
                }
            }
        }
    }

    if disk_id_to_bus.is_empty() {
        return;
    }

    // Map diskId → href via Disk and File elements
    let mut file_map: HashMap<String, String> = HashMap::new();
    for node in root.descendants().filter(|n| n.tag_name().name() == "File") {
        if let (Some(id), Some(href)) = (
            node.attributes()
                .find(|a| a.name() == "id")
                .map(|a| a.value().to_string()),
            node.attributes()
                .find(|a| a.name() == "href")
                .map(|a| a.value().to_string()),
        ) {
            file_map.insert(id, href);
        }
    }

    let mut disk_id_to_href: HashMap<String, String> = HashMap::new();
    for node in root.descendants().filter(|n| n.tag_name().name() == "Disk") {
        let disk_id = node
            .attributes()
            .find(|a| a.name() == "diskId")
            .map(|a| a.value().to_string());
        let file_ref = node
            .attributes()
            .find(|a| a.name() == "fileRef")
            .map(|a| a.value().to_string());
        if let (Some(did), Some(fref)) = (disk_id, file_ref) {
            if let Some(href) = file_map.get(&fref) {
                disk_id_to_href.insert(did, href.clone());
            }
        }
    }

    // Apply bus to matching disks
    for (disk_id, bus) in &disk_id_to_bus {
        if let Some(href) = disk_id_to_href.get(disk_id) {
            for disk in disks.iter_mut() {
                if &disk.href == href {
                    disk.bus = *bus;
                }
            }
        }
    }
}

/// Convert OVF AllocationUnits value to MiB.
pub fn normalize_memory_to_mib(qty: u64, units: &str) -> u64 {
    let u = units.trim().to_lowercase();
    if u.contains("2^30") || u.contains("giga") {
        qty * 1024
    } else if u.contains("2^10") || u.contains("kilo") {
        qty / 1024
    } else {
        qty
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal but complete OVF covering all resource types we parse.
    const SAMPLE_OVF: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Envelope
  xmlns="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:ovf="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:rasd="http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ResourceAllocationSettingData">
  <References>
    <File ovf:href="ubuntu-server.vmdk" ovf:id="file1"/>
  </References>
  <VirtualSystem ovf:id="ubuntu-server-fallback">
    <Name>Ubuntu Server Test</Name>
    <VirtualHardwareSection>
      <Item>
        <rasd:ResourceType>3</rasd:ResourceType>
        <rasd:VirtualQuantity>4</rasd:VirtualQuantity>
      </Item>
      <Item>
        <rasd:ResourceType>4</rasd:ResourceType>
        <rasd:VirtualQuantity>4096</rasd:VirtualQuantity>
        <rasd:AllocationUnits>byte * 2^20</rasd:AllocationUnits>
      </Item>
      <Item>
        <rasd:ResourceType>10</rasd:ResourceType>
        <rasd:VirtualQuantity>1</rasd:VirtualQuantity>
      </Item>
    </VirtualHardwareSection>
  </VirtualSystem>
</Envelope>"#;

    const OVF_GIB_MEMORY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Envelope xmlns="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:ovf="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:rasd="http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ResourceAllocationSettingData">
  <References>
    <File ovf:href="disk.vmdk" ovf:id="file1"/>
  </References>
  <VirtualSystem ovf:id="gib-vm">
    <Name>GiB VM</Name>
    <VirtualHardwareSection>
      <Item>
        <rasd:ResourceType>3</rasd:ResourceType>
        <rasd:VirtualQuantity>2</rasd:VirtualQuantity>
      </Item>
      <Item>
        <rasd:ResourceType>4</rasd:ResourceType>
        <rasd:VirtualQuantity>8</rasd:VirtualQuantity>
        <rasd:AllocationUnits>byte * 2^30</rasd:AllocationUnits>
      </Item>
    </VirtualHardwareSection>
  </VirtualSystem>
</Envelope>"#;

    const OVF_NO_DISK: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Envelope xmlns="http://schemas.dmtf.org/ovf/envelope/1">
  <VirtualSystem ovf:id="no-disk">
    <Name>No Disk</Name>
  </VirtualSystem>
</Envelope>"#;

    #[test]
    fn test_parse_basic_fields() {
        let cfg = parse_ovf_str(SAMPLE_OVF, false).unwrap();
        assert_eq!(cfg.name, "Ubuntu-Server-Test");
        assert_eq!(cfg.vcpu_count, 4);
        assert_eq!(cfg.memory_mb, 4096);
        assert_eq!(cfg.disks.len(), 1);
        assert_eq!(cfg.disks[0].href, "ubuntu-server.vmdk");
        assert_eq!(cfg.disks[0].input_format, DiskFormat::Vmdk);
        assert_eq!(cfg.disks[0].bus, DiskBus::Virtio);
        assert_eq!(cfg.nic_count(), 1);
        assert!(!cfg.uefi);
    }

    #[test]
    fn test_uefi_flag_propagated() {
        let cfg = parse_ovf_str(SAMPLE_OVF, true).unwrap();
        assert!(cfg.uefi);
    }

    #[test]
    fn test_gib_memory_converted_to_mib() {
        let cfg = parse_ovf_str(OVF_GIB_MEMORY, false).unwrap();
        assert_eq!(cfg.memory_mb, 8192);
    }

    const OVF_MULTI_DISK: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Envelope
  xmlns="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:ovf="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:rasd="http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ResourceAllocationSettingData">
  <References>
    <File ovf:href="os-disk.vmdk" ovf:id="file1"/>
    <File ovf:href="data-disk.vmdk" ovf:id="file2"/>
  </References>
  <VirtualSystem ovf:id="multi-disk-vm">
    <Name>Multi Disk VM</Name>
    <VirtualHardwareSection>
      <Item>
        <rasd:ResourceType>3</rasd:ResourceType>
        <rasd:VirtualQuantity>2</rasd:VirtualQuantity>
      </Item>
      <Item>
        <rasd:ResourceType>4</rasd:ResourceType>
        <rasd:VirtualQuantity>4096</rasd:VirtualQuantity>
        <rasd:AllocationUnits>byte * 2^20</rasd:AllocationUnits>
      </Item>
    </VirtualHardwareSection>
  </VirtualSystem>
</Envelope>"#;

    #[test]
    fn test_multiple_disk_files_parsed() {
        let cfg = parse_ovf_str(OVF_MULTI_DISK, false).unwrap();
        let hrefs: Vec<&str> = cfg.disks.iter().map(|d| d.href.as_str()).collect();
        assert_eq!(hrefs, vec!["os-disk.vmdk", "data-disk.vmdk"]);
    }

    #[test]
    fn test_missing_disk_file_is_error() {
        let result = parse_ovf_str(OVF_NO_DISK, false);
        assert!(
            result.is_err(),
            "Expected error when no disk reference exists"
        );
    }

    #[test]
    fn test_zero_nics_when_no_ethernet_items() {
        let cfg = parse_ovf_str(OVF_GIB_MEMORY, false).unwrap();
        assert_eq!(cfg.nic_count(), 0);
    }

    #[test]
    fn test_sanitize_removes_spaces() {
        assert_eq!(sanitize_vm_name("My VM Name"), "My-VM-Name");
    }

    #[test]
    fn test_sanitize_preserves_dashes_and_underscores() {
        assert_eq!(sanitize_vm_name("ubuntu-server_v2"), "ubuntu-server_v2");
    }

    #[test]
    fn test_sanitize_replaces_dots() {
        assert_eq!(sanitize_vm_name("vm.name.1"), "vm-name-1");
    }

    #[test]
    fn test_normalize_mib_default() {
        assert_eq!(normalize_memory_to_mib(4096, "byte * 2^20"), 4096);
    }

    #[test]
    fn test_normalize_gib() {
        assert_eq!(normalize_memory_to_mib(4, "byte * 2^30"), 4096);
    }

    #[test]
    fn test_normalize_gigabytes_string() {
        assert_eq!(normalize_memory_to_mib(2, "GigaBytes"), 2048);
    }

    #[test]
    fn test_normalize_kib() {
        assert_eq!(normalize_memory_to_mib(2_097_152, "byte * 2^10"), 2048);
    }

    #[test]
    fn test_fallback_name_from_virtual_system_id() {
        let ovf = r#"<?xml version="1.0" encoding="UTF-8"?>
<Envelope xmlns="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:ovf="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:rasd="http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ResourceAllocationSettingData">
  <References>
    <File ovf:href="disk.vmdk" ovf:id="file1"/>
  </References>
  <VirtualSystem ovf:id="fallback-name">
    <VirtualHardwareSection>
      <Item>
        <rasd:ResourceType>3</rasd:ResourceType>
        <rasd:VirtualQuantity>1</rasd:VirtualQuantity>
      </Item>
      <Item>
        <rasd:ResourceType>4</rasd:ResourceType>
        <rasd:VirtualQuantity>512</rasd:VirtualQuantity>
        <rasd:AllocationUnits>byte * 2^20</rasd:AllocationUnits>
      </Item>
    </VirtualHardwareSection>
  </VirtualSystem>
</Envelope>"#;
        let cfg = parse_ovf_str(ovf, false).unwrap();
        assert_eq!(cfg.name, "fallback-name");
    }

    #[test]
    fn test_invalid_xml_is_error() {
        let result = parse_ovf_str("<broken xml >>>", false);
        assert!(result.is_err());
    }

    // ── Feature 2: DiskFormat tests ─────────────────────────────────────────

    #[test]
    fn test_disk_format_from_ovf_uri_vmware() {
        assert_eq!(
            DiskFormat::from_ovf_uri(
                "http://www.vmware.com/interfaces/specifications/vmdk.html#streamOptimized"
            ),
            Some(DiskFormat::Vmdk)
        );
    }

    #[test]
    fn test_disk_format_from_ovf_uri_vhd() {
        assert_eq!(
            DiskFormat::from_ovf_uri("http://technet.microsoft.com/en-us/library/bb676673.aspx"),
            None // This URI doesn't contain "vhd"
        );
        assert_eq!(
            DiskFormat::from_ovf_uri("http://example.com/vhd-format"),
            Some(DiskFormat::Vhd)
        );
    }

    #[test]
    fn test_disk_format_from_extension() {
        assert_eq!(
            DiskFormat::from_extension("disk.vmdk"),
            Some(DiskFormat::Vmdk)
        );
        assert_eq!(
            DiskFormat::from_extension("disk.vhd"),
            Some(DiskFormat::Vhd)
        );
        assert_eq!(
            DiskFormat::from_extension("disk.vhdx"),
            Some(DiskFormat::Vhdx)
        );
        assert_eq!(
            DiskFormat::from_extension("disk.vdi"),
            Some(DiskFormat::Vdi)
        );
        assert_eq!(
            DiskFormat::from_extension("disk.raw"),
            Some(DiskFormat::Raw)
        );
        assert_eq!(
            DiskFormat::from_extension("disk.img"),
            Some(DiskFormat::Raw)
        );
        assert_eq!(
            DiskFormat::from_extension("disk.qcow2"),
            Some(DiskFormat::Qcow2)
        );
        assert_eq!(DiskFormat::from_extension("disk.txt"), None);
    }

    #[test]
    fn test_disk_format_qemu_str() {
        assert_eq!(DiskFormat::Vmdk.qemu_format_str(), "vmdk");
        assert_eq!(DiskFormat::Vhd.qemu_format_str(), "vpc");
        assert_eq!(DiskFormat::Vhdx.qemu_format_str(), "vpc");
        assert_eq!(DiskFormat::Vdi.qemu_format_str(), "vdi");
        assert_eq!(DiskFormat::Raw.qemu_format_str(), "raw");
        assert_eq!(DiskFormat::Qcow2.qemu_format_str(), "qcow2");
    }

    #[test]
    fn test_ovf_with_disk_section_and_format_uri() {
        let ovf = r#"<?xml version="1.0" encoding="UTF-8"?>
<Envelope
  xmlns="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:ovf="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:rasd="http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ResourceAllocationSettingData">
  <References>
    <File ovf:href="disk.vmdk" ovf:id="file1"/>
  </References>
  <DiskSection>
    <Disk ovf:diskId="vmdisk1" ovf:fileRef="file1"
          ovf:format="http://www.vmware.com/interfaces/specifications/vmdk.html#streamOptimized"
          ovf:capacity="10737418240"/>
  </DiskSection>
  <VirtualSystem ovf:id="test">
    <Name>Test</Name>
    <VirtualHardwareSection>
      <Item>
        <rasd:ResourceType>3</rasd:ResourceType>
        <rasd:VirtualQuantity>1</rasd:VirtualQuantity>
      </Item>
      <Item>
        <rasd:ResourceType>4</rasd:ResourceType>
        <rasd:VirtualQuantity>1024</rasd:VirtualQuantity>
      </Item>
    </VirtualHardwareSection>
  </VirtualSystem>
</Envelope>"#;
        let cfg = parse_ovf_str(ovf, false).unwrap();
        assert_eq!(cfg.disks.len(), 1);
        assert_eq!(cfg.disks[0].input_format, DiskFormat::Vmdk);
        assert_eq!(cfg.disks[0].href, "disk.vmdk");
    }

    #[test]
    fn test_ovf_with_vdi_disk_fallback_to_extension() {
        let ovf = r#"<?xml version="1.0" encoding="UTF-8"?>
<Envelope
  xmlns="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:ovf="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:rasd="http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ResourceAllocationSettingData">
  <References>
    <File ovf:href="disk.vdi" ovf:id="file1"/>
  </References>
  <DiskSection>
    <Disk ovf:diskId="vmdisk1" ovf:fileRef="file1"
          ovf:capacity="10737418240"/>
  </DiskSection>
  <VirtualSystem ovf:id="test">
    <Name>VDI Test</Name>
    <VirtualHardwareSection>
      <Item>
        <rasd:ResourceType>3</rasd:ResourceType>
        <rasd:VirtualQuantity>1</rasd:VirtualQuantity>
      </Item>
      <Item>
        <rasd:ResourceType>4</rasd:ResourceType>
        <rasd:VirtualQuantity>1024</rasd:VirtualQuantity>
      </Item>
    </VirtualHardwareSection>
  </VirtualSystem>
</Envelope>"#;
        let cfg = parse_ovf_str(ovf, false).unwrap();
        assert_eq!(cfg.disks[0].input_format, DiskFormat::Vdi);
    }

    // ── Feature 3: ISO extraction tests ─────────────────────────────────────

    #[test]
    fn test_iso_files_extracted() {
        let ovf = r#"<?xml version="1.0" encoding="UTF-8"?>
<Envelope
  xmlns="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:ovf="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:rasd="http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ResourceAllocationSettingData">
  <References>
    <File ovf:href="disk.vmdk" ovf:id="file1"/>
    <File ovf:href="tools.iso" ovf:id="file2"/>
  </References>
  <VirtualSystem ovf:id="test">
    <Name>ISO Test</Name>
    <VirtualHardwareSection>
      <Item>
        <rasd:ResourceType>3</rasd:ResourceType>
        <rasd:VirtualQuantity>1</rasd:VirtualQuantity>
      </Item>
      <Item>
        <rasd:ResourceType>4</rasd:ResourceType>
        <rasd:VirtualQuantity>1024</rasd:VirtualQuantity>
      </Item>
    </VirtualHardwareSection>
  </VirtualSystem>
</Envelope>"#;
        let cfg = parse_ovf_str(ovf, false).unwrap();
        assert_eq!(cfg.iso_files, vec!["tools.iso"]);
    }

    // ── Feature 6: Controller mapping tests ─────────────────────────────────

    #[test]
    fn test_scsi_controller_mapping() {
        let ovf = r#"<?xml version="1.0" encoding="UTF-8"?>
<Envelope
  xmlns="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:ovf="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:rasd="http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ResourceAllocationSettingData">
  <References>
    <File ovf:href="disk.vmdk" ovf:id="file1"/>
  </References>
  <DiskSection>
    <Disk ovf:diskId="vmdisk1" ovf:fileRef="file1"
          ovf:format="http://www.vmware.com/interfaces/specifications/vmdk.html#streamOptimized"/>
  </DiskSection>
  <VirtualSystem ovf:id="test">
    <Name>SCSI Test</Name>
    <VirtualHardwareSection>
      <Item>
        <rasd:ResourceType>3</rasd:ResourceType>
        <rasd:VirtualQuantity>1</rasd:VirtualQuantity>
      </Item>
      <Item>
        <rasd:ResourceType>4</rasd:ResourceType>
        <rasd:VirtualQuantity>1024</rasd:VirtualQuantity>
      </Item>
      <Item>
        <rasd:InstanceID>3</rasd:InstanceID>
        <rasd:ResourceType>6</rasd:ResourceType>
      </Item>
      <Item>
        <rasd:ResourceType>17</rasd:ResourceType>
        <rasd:Parent>3</rasd:Parent>
        <rasd:HostResource>ovf:/disk/vmdisk1</rasd:HostResource>
      </Item>
    </VirtualHardwareSection>
  </VirtualSystem>
</Envelope>"#;
        let cfg = parse_ovf_str(ovf, false).unwrap();
        assert_eq!(cfg.disks[0].bus, DiskBus::Scsi);
    }

    #[test]
    fn test_sata_controller_mapping() {
        let ovf = r#"<?xml version="1.0" encoding="UTF-8"?>
<Envelope
  xmlns="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:ovf="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:rasd="http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ResourceAllocationSettingData">
  <References>
    <File ovf:href="disk.vmdk" ovf:id="file1"/>
  </References>
  <DiskSection>
    <Disk ovf:diskId="vmdisk1" ovf:fileRef="file1"
          ovf:format="http://www.vmware.com/interfaces/specifications/vmdk.html#streamOptimized"/>
  </DiskSection>
  <VirtualSystem ovf:id="test">
    <Name>SATA Test</Name>
    <VirtualHardwareSection>
      <Item>
        <rasd:ResourceType>3</rasd:ResourceType>
        <rasd:VirtualQuantity>1</rasd:VirtualQuantity>
      </Item>
      <Item>
        <rasd:ResourceType>4</rasd:ResourceType>
        <rasd:VirtualQuantity>1024</rasd:VirtualQuantity>
      </Item>
      <Item>
        <rasd:InstanceID>4</rasd:InstanceID>
        <rasd:ResourceType>20</rasd:ResourceType>
      </Item>
      <Item>
        <rasd:ResourceType>17</rasd:ResourceType>
        <rasd:Parent>4</rasd:Parent>
        <rasd:HostResource>ovf:/disk/vmdisk1</rasd:HostResource>
      </Item>
    </VirtualHardwareSection>
  </VirtualSystem>
</Envelope>"#;
        let cfg = parse_ovf_str(ovf, false).unwrap();
        assert_eq!(cfg.disks[0].bus, DiskBus::Sata);
    }

    #[test]
    fn test_default_bus_is_virtio_when_no_controller() {
        let cfg = parse_ovf_str(SAMPLE_OVF, false).unwrap();
        assert_eq!(cfg.disks[0].bus, DiskBus::Virtio);
    }

    // ── Feature 7: MAC address tests ────────────────────────────────────────

    #[test]
    fn test_mac_address_extracted() {
        let ovf = r#"<?xml version="1.0" encoding="UTF-8"?>
<Envelope
  xmlns="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:ovf="http://schemas.dmtf.org/ovf/envelope/1"
  xmlns:rasd="http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_ResourceAllocationSettingData">
  <References>
    <File ovf:href="disk.vmdk" ovf:id="file1"/>
  </References>
  <VirtualSystem ovf:id="test">
    <Name>MAC Test</Name>
    <VirtualHardwareSection>
      <Item>
        <rasd:ResourceType>3</rasd:ResourceType>
        <rasd:VirtualQuantity>1</rasd:VirtualQuantity>
      </Item>
      <Item>
        <rasd:ResourceType>4</rasd:ResourceType>
        <rasd:VirtualQuantity>1024</rasd:VirtualQuantity>
      </Item>
      <Item>
        <rasd:ResourceType>10</rasd:ResourceType>
        <rasd:Address>00:50:56:C0:00:01</rasd:Address>
      </Item>
    </VirtualHardwareSection>
  </VirtualSystem>
</Envelope>"#;
        let cfg = parse_ovf_str(ovf, false).unwrap();
        assert_eq!(cfg.nics.len(), 1);
        assert_eq!(
            cfg.nics[0].mac_address.as_deref(),
            Some("00:50:56:C0:00:01")
        );
    }

    #[test]
    fn test_nic_without_mac_has_none() {
        let cfg = parse_ovf_str(SAMPLE_OVF, false).unwrap();
        assert_eq!(cfg.nics.len(), 1);
        assert!(cfg.nics[0].mac_address.is_none());
    }
}

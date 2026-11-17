use super::*;
use stela_driver_core::firmware::{Provenance, Resource, Target};

#[test]
fn the_complete_bundle_holds_every_recipe_once_by_content() {
    let objects = objects();
    for recipe in recipes() {
        assert!(
            objects
                .iter()
                .any(|object| object.sha256 == recipe.image.sha256),
            "{}",
            recipe.image.member
        );
    }
    let mut digests: Vec<_> = objects.iter().map(|object| object.sha256).collect();
    digests.sort_unstable();
    digests.dedup();
    assert_eq!(digests.len(), objects.len());
    let (count, bytes) = size();
    assert_eq!(count, objects.len());
    // The RTL8168H and RTL8168M share a file, as the RTL9151A and RTL9151AS
    // do, and, with the AMD port, amdgpu's `mec` and `mec2` of Raphael and
    // of Navi 21.
    if Ports::BUILT == Ports::ALL {
        assert_eq!(count + 4, recipes().count());
        // The recipes' recorded lengths: 149 files, 194.8 MiB.
        assert_eq!((count, bytes), (149, 204_290_704), "{count} {bytes}");
    }
    if Ports::BUILT == Ports::NONE {
        assert_eq!(count + 2, recipes().count());
        // Without the GPU ports: 88 files, 177.0 MiB.
        assert_eq!((count, bytes), (88, 185_584_904), "{count} {bytes}");
    }
    // One port alone, as a build with only one of the two carries it.
    if Ports::BUILT == (Ports {
        amd: true,
        intel: false,
    }) {
        assert_eq!(count + 4, recipes().count());
        // With the AMD port alone: 121 files, 184.9 MiB.
        assert_eq!((count, bytes), (121, 193_887_456), "{count} {bytes}");
    }
    if Ports::BUILT == (Ports {
        amd: false,
        intel: true,
    }) {
        assert_eq!(count + 2, recipes().count());
        // With the Intel port alone: 116 files, 186.9 MiB.
        assert_eq!((count, bytes), (116, 195_988_152), "{count} {bytes}");
    }
    // The GPU's large images come last, for a volume short of room.
    assert_eq!(objects.last().unwrap().length, 64_116_464);
    let manifest = manifest(&complete()).unwrap();
    let bundle = Bundle::parse(&manifest).unwrap();
    assert_eq!(bundle.objects.len(), count);
    assert!(bundle.missing().is_empty());
    assert_eq!(bundle.notices.len(), notices().len());
    for notice in &bundle.notices {
        assert!(
            notice.member.contains("LICEN") || notice.member == "COPYING",
            "{}",
            notice.member
        );
    }
}

#[test]
fn a_shared_file_is_stored_once_and_a_partial_bundle_names_what_it_lacks() {
    let first = recipes::GSP[0].image;
    let notice = *recipes::GSP[0].license;
    let alias = Artifact {
        member: "nvidia/other/gsp/alias.bin",
        ..first
    };
    // Two names for the same content: one file.
    let mut files: Vec<(&Artifact, &Artifact)> = Vec::new();
    for artifact in [&first, &alias] {
        if !files.iter().any(|(seen, _)| seen.sha256 == artifact.sha256) {
            files.push((artifact, &notice));
        }
    }
    let bytes = manifest(&files).unwrap();
    let bundle = Bundle::parse(&bytes).unwrap();
    assert_eq!(bundle.objects.len(), 1);
    assert_eq!(bundle.missing().len(), size().0 - 1);
    // A duplicate digest, a path off its digest, or a dangling notice is
    // refused.
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let object = value["objects"][0].clone();
    value["objects"].as_array_mut().unwrap().push(object);
    assert!(Bundle::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["objects"][0]["path"] = "other.bin".into();
    assert!(Bundle::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["objects"][0]["license"] = "licenses/none.txt".into();
    assert!(Bundle::parse(&serde_json::to_vec(&value).unwrap()).is_err());
}

fn members(selection: &Selection) -> Vec<&'static str> {
    let mut members: Vec<_> = selection
        .wanted
        .iter()
        .map(|(artifact, _)| artifact.member)
        .collect();
    members.sort_unstable();
    members
}

/// The USB Bluetooth and Ethernet files every boot reads, sorted.
const USB: [&str; 12] = [
    "intel/ibt-20-1-3.ddc",
    "intel/ibt-20-1-3.sfi",
    "mediatek/BT_RAM_CODE_MT7922_1_1_hdr.bin",
    "mediatek/BT_RAM_CODE_MT7961_1_2_hdr.bin",
    "mediatek/mt7925/BT_RAM_CODE_MT7925_1_1_hdr.bin",
    "rtl_nic/rtl8153a-2.fw",
    "rtl_nic/rtl8153a-3.fw",
    "rtl_nic/rtl8153a-4.fw",
    "rtl_nic/rtl8153b-2.fw",
    "rtl_nic/rtl8153c-1.fw",
    "rtl_nic/rtl8156a-2.fw",
    "rtl_nic/rtl8156b-2.fw",
];

fn pci(vendor: u16, device: u16, class: u8) -> Pci {
    Pci {
        vendor,
        device,
        class,
    }
}

#[test]
fn a_boot_selects_exactly_the_files_its_pci_functions_need() {
    // No device of a supported family: the USB Bluetooth and Ethernet files alone.
    let none = select(&[pci(0x8086, 0x125c, 0x02), pci(0x1b36, 0x000d, 0x0c)]);
    assert_eq!(members(&none), USB);
    assert!(none.unknown.is_empty());
    // An RTX 5080 (GB203) and its audio function, an MT7925 and an RTL8168.
    let pc = select(&[
        pci(0x10de, 0x2c02, 0x03),
        pci(0x10de, 0x22e9, 0x04),
        pci(0x14c3, 0x7925, 0x02),
        pci(0x10ec, 0x8168, 0x02),
    ]);
    let mut expected: Vec<&str> = USB.to_vec();
    expected.extend([
        "mediatek/mt7925/WIFI_MT7925_PATCH_MCU_1_1_hdr.bin",
        "mediatek/mt7925/WIFI_RAM_CODE_MT7925_1_1.bin",
        "nvidia/gb202/gsp/bootloader-570.211.01.bin",
        "nvidia/gb202/gsp/fmc-570.211.01.bin",
        "nvidia/gb202/gsp/gsp-570.211.01.bin",
        "regulatory.db",
        "regulatory.db.p7s",
    ]);
    expected.extend(recipes::REALTEK.iter().map(|recipe| recipe.image.member));
    expected.sort_unstable();
    expected.dedup();
    assert_eq!(members(&pc), expected);
    // A Realtek chip under an alias ID, such as a Killer E2600, reads them too.
    let killer = select(&[pci(0x10ec, 0x2600, 0x02)]);
    assert!(members(&killer).contains(&"rtl_nic/rtl8168h-2.fw"));
    assert_eq!(
        pc.wanted
            .iter()
            .find(|(artifact, _)| artifact.length == 64_116_464)
            .unwrap()
            .1,
        "pci 10de:2c02"
    );
    // About the GSP image and the small files, not the whole bundle.
    assert!(pc.bytes() < 70_000_000, "{}", pc.bytes());
    // An AX210: its image and PNVM and the database.
    let ax210 = select(&[pci(0x8086, 0x2725, 0x02)]);
    let mut expected: Vec<&str> = USB.to_vec();
    expected.extend([
        "intel/iwlwifi/iwlwifi-ty-a0-gf-a0-89.ucode",
        "intel/iwlwifi/iwlwifi-ty-a0-gf-a0.pnvm",
        "regulatory.db",
        "regulatory.db.p7s",
    ]);
    expected.sort_unstable();
    assert_eq!(members(&ax210), expected);
    // With the AMD port: Ryzen 9000 graphics, the files amdgpu asks for on it, its HDMI audio
    // function needs none, and an AMD display chip without pinned files
    // (Navi 10) is named once.
    let ryzen = select_for(
        &[
            pci(0x1002, 0x13c0, 0x03),
            pci(0x1002, 0x1640, 0x04),
            pci(0x1002, 0x731f, 0x03),
        ],
        Ports::ALL,
    );
    // The files amdgpu asks for on a GPU of the PCI device ID `device`.
    let family = |device: u16| {
        let mut inventory = Inventory::default();
        recipes::require_amdgpu(&mut inventory, device).unwrap();
        let mut names: Vec<&str> = inventory
            .requests()
            .iter()
            .map(|request| {
                recipes::AMDGPU
                    .iter()
                    .find(|recipe| recipe.image.member == request.name)
                    .unwrap()
                    .image
                    .member
            })
            .collect();
        names.sort_unstable();
        names
    };
    let raphael = family(0x13c0);
    let mut expected: Vec<&str> = USB.to_vec();
    expected.extend(raphael.iter().copied());
    expected.sort_unstable();
    // `mec2` is `mec`'s bytes, which the bundle holds once.
    let found = members(&ryzen);
    assert!(found.iter().all(|member| expected.contains(member)));
    assert_eq!(found.len(), USB.len() + raphael.len() - 1);
    assert_eq!(ryzen.unknown, ["pci 1002:731f"]);
    // An RX 9070 (Navi 48) and its audio function: amdgpu's files for Navi
    // 48 and nothing for the audio function.
    let navi48 = select_for(
        &[pci(0x1002, 0x7550, 0x03), pci(0x1002, 0xab40, 0x04)],
        Ports::ALL,
    );
    let mut expected: Vec<&str> = USB.to_vec();
    expected.extend(family(0x7550));
    expected.sort_unstable();
    assert_eq!(members(&navi48), expected);
    assert!(navi48.unknown.is_empty());
    // An RX 6800 XT (Navi 21): its files, `mec2` held once as `mec`'s bytes.
    let navi21 = select_for(
        &[pci(0x1002, 0x73bf, 0x03), pci(0x1002, 0xab28, 0x04)],
        Ports::ALL,
    );
    let found = members(&navi21);
    let files = family(0x73bf);
    assert!(
        found
            .iter()
            .all(|member| USB.contains(member) || files.contains(member))
    );
    assert_eq!(found.len(), USB.len() + files.len() - 1);
    assert!(navi21.unknown.is_empty());
    // An NVIDIA display chip no recipe covers is named once.
    let old = select(&[pci(0x10de, 0x1b80, 0x03), pci(0x10de, 0x1b80, 0x03)]);
    assert_eq!(old.unknown, ["pci 10de:1b80"]);
    assert_eq!(members(&old), USB);
}

#[test]
fn an_intel_igp_selects_the_files_its_platforms_xe_binds() {
    // The Alder Lake P Iris Xe of an i7-1265U, with a non-display Intel
    // function beside it.
    let igp = select_for(
        &[pci(0x8086, 0x46a6, 0x03), pci(0x8086, 0x51c8, 0x04)],
        Ports::ALL,
    );
    let mut inventory = Inventory::default();
    recipes::require_xe(&mut inventory, 0x46a6).unwrap();
    let mut expected: Vec<&str> = USB.to_vec();
    expected.extend(
        inventory
            .requests()
            .iter()
            .map(|request| request.name.as_str()),
    );
    expected.sort_unstable();
    assert_eq!(members(&igp), expected);
    assert!(igp.unknown.is_empty());
    // An Intel display chip no platform of xe's pins files for is named once.
    let old = select_for(&[pci(0x8086, 0x0042, 0x03)], Ports::ALL);
    assert_eq!(old.unknown, ["pci 8086:0042"]);
    assert_eq!(members(&old), USB);
}

#[test]
fn the_handoff_manifest_keeps_an_older_per_pc_manifest_beside_bundle_files() {
    let older = serde_json::to_vec(&serde_json::json!({"schema":1,"images":[{
        "name":"rtl_nic/rtl8168e-3.fw", "path":"0.bin",
        "target":{"driver":"realtek","hardware":"10ec:8168","revision":"xid-2c8","board":null},
        "provenance":{"source":"synthetic-fixture","revision":"v1","member":"one.bin","license":"CC0-1.0"},
        "length":3,"sha256":"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"}]}))
    .unwrap();
    let older = super::super::Manifest::parse(&older).unwrap();
    let bytes = manifest(&complete()).unwrap();
    let bundle = Bundle::parse(&bytes).unwrap();
    let selected: Vec<_> = bundle.objects.iter().take(2).collect();
    let handoff = handoff_manifest(Some(&older), &selected).unwrap();
    let parsed = super::super::Manifest::parse(&handoff).unwrap();
    assert_eq!(parsed.images.len(), 3);
    assert_eq!(parsed.images[0].name, "rtl_nic/rtl8168e-3.fw");
    assert_eq!(parsed.images[0].target.revision, "xid-2c8");
    assert_eq!(parsed.images[1].name, name(selected[0].sha256));
    assert_eq!(parsed.images[1].target.driver, TARGET.driver);
    // An older manifest alone still hands off as it was.
    let alone = handoff_manifest(Some(&older), &[]).unwrap();
    assert_eq!(
        super::super::Manifest::parse(&alone).unwrap().images.len(),
        1
    );
}

const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

static SYNTHETIC: Artifact = Artifact {
    source: "synthetic",
    revision: "v1",
    member: "regulatory.db",
    length: 3,
    sha256: ABC,
};

fn synthetic_pin(request: &Request) -> Option<&'static Artifact> {
    (request.name == "regulatory.db").then_some(&SYNTHETIC)
}

fn stored(name: &'static str, bytes: &[u8]) -> &'static Catalog {
    use stela_crypto::{Sha256, digest::Digest};
    Box::leak(Box::new(
        Catalog::from_handoff(alloc::vec![Resource {
            name,
            target: TARGET,
            provenance: Provenance {
                source: "synthetic",
                revision: "v1",
                member: "regulatory.db",
                license: "licenses/x.txt",
            },
            expected_length: bytes.len(),
            expected_sha256: Sha256::digest(bytes).into(),
            bytes: bytes.to_vec(),
        }])
        .unwrap(),
    ))
}

const REQUEST: Requirement<'static> = Requirement {
    name: "regulatory.db",
    target: Target {
        driver: "wifi-regulatory",
        hardware: "wireless-regdb",
        revision: "v20",
        board: None,
    },
    minimum_bytes: 3,
};

#[test]
fn a_drivers_exact_request_gets_the_bundle_file_its_recipe_pins() {
    let name: &'static str = Box::leak(name(ABC).into_boxed_str());
    let catalog = stored(name, b"abc");
    let provider = BundleProvider::with_pins(catalog, synthetic_pin);
    for _ in 0..2 {
        let image = provider.load(REQUEST).unwrap();
        assert_eq!(image.name(), "regulatory.db");
        assert_eq!(image.target(), REQUEST.target);
        assert_eq!(image.bytes(), b"abc");
        assert_eq!(image.provenance().member, "regulatory.db");
    }
    // Bound once, then found again.
    assert_eq!(provider.bound.borrow().len(), 1);
    // A request longer than the file is refused under its own name.
    let long = Requirement {
        minimum_bytes: 4,
        ..REQUEST
    };
    let error = BundleProvider::with_pins(catalog, synthetic_pin)
        .load(long)
        .unwrap_err();
    assert!(
        matches!(error.kind, ErrorKind::Truncated { .. }),
        "{error:?}"
    );
    assert_eq!(error.name.as_str(), Some("regulatory.db"));
    // A request no pin names stays missing.
    let other = Requirement {
        name: "vendor/unpinned.bin",
        ..REQUEST
    };
    assert_eq!(provider.load(other).unwrap_err().kind, ErrorKind::Missing);
}

#[test]
fn a_stored_file_whose_digest_is_not_the_pin_is_never_bound() {
    // The real pin for the database, and a file stored under its name whose
    // bytes are something else.
    let pin = recipes::REGULATORY[0].image;
    let name: &'static str = Box::leak(name(pin.sha256).into_boxed_str());
    let catalog = stored(name, b"not the database");
    let provider = BundleProvider::new(catalog);
    assert_eq!(
        provider
            .load(Requirement {
                minimum_bytes: 8,
                ..REQUEST
            })
            .unwrap_err()
            .kind,
        ErrorKind::Missing
    );
    assert!(provider.bound.borrow().is_empty());
}

#[test]
fn an_exact_binding_wins_over_the_bundle() {
    use stela_crypto::{Sha256, digest::Digest};
    let exact: &'static Catalog = Box::leak(Box::new(
        Catalog::from_handoff(alloc::vec![Resource {
            name: REQUEST.name,
            target: REQUEST.target,
            provenance: Provenance {
                source: "synthetic",
                revision: "v2",
                member: "regulatory.db",
                license: "CC0-1.0",
            },
            expected_length: 4,
            expected_sha256: Sha256::digest(b"abcd").into(),
            bytes: b"abcd".to_vec(),
        }])
        .unwrap(),
    ));
    let provider = BundleProvider::with_pins(exact, synthetic_pin);
    let image = provider.load(REQUEST).unwrap();
    assert_eq!(image.bytes(), b"abcd");
}

/// The GPU ports named in a Cargo.toml line's feature list.
fn ports_of(line: &str) -> Ports {
    Ports {
        amd: line.contains("amdgpu-port\""),
        intel: line.contains("xe-port\""),
    }
}

/// The line of `manifest` that starts with `start`.
fn line<'a>(manifest: &'a str, start: &str) -> &'a str {
    manifest
        .lines()
        .find(|line| line.starts_with(start))
        .unwrap_or_else(|| panic!("no line {start}"))
}

#[test]
fn the_boot_images_drivers_and_the_bundle_name_the_same_gpu_ports() {
    let boot = include_str!("../../../../stela-boot/Cargo.toml");
    let loader = include_str!("../../../Cargo.toml");
    let cli = include_str!("../../../../stela-cli/Cargo.toml");
    // The default image's drivers, the boot's bundle and a default stick's:
    // no AMD port in any.
    let drivers = ports_of(line(boot, "stela-drivers = "));
    assert_eq!(drivers, Ports::NONE);
    assert_eq!(ports_of(line(boot, "stela-loader = ")), drivers);
    assert_eq!(ports_of(line(loader, "default = ")), drivers);
    // An image with amdgpu reads its firmware, and `stela` built with the
    // port writes it.
    let amdgpu = line(boot, "amdgpu = ");
    assert!(amdgpu.contains("\"stela-drivers/amdgpu-port\""), "{amdgpu}");
    assert!(amdgpu.contains("\"stela-loader/amdgpu-port\""), "{amdgpu}");
    let port = line(cli, "amdgpu-port = ");
    assert!(port.contains("\"stela-loader/amdgpu-port\""), "{port}");
    // The same for xe, whose boot feature is named after the driver.
    let xe = line(boot, "xe = ");
    assert!(xe.contains("\"stela-drivers/xe-port\""), "{xe}");
    assert!(xe.contains("\"stela-loader/xe-port\""), "{xe}");
    let port = line(cli, "xe-port = ");
    assert!(port.contains("\"stela-loader/xe-port\""), "{port}");
}

#[test]
fn a_build_without_the_amd_port_bundles_none_of_its_firmware() {
    let theirs = |member: &str| member.starts_with("amdgpu/");
    let without_port = objects_for(Ports::NONE);
    assert!(!without_port.is_empty());
    assert!(without_port.iter().all(|artifact| !theirs(artifact.member)));
    assert!(
        families_for(Ports::NONE)
            .flat_map(|(_, recipes)| recipes.iter())
            .all(|recipe| !recipe.license.member.contains("amdgpu"))
    );
    // Its GPUs select nothing; the rest is as with the port.
    let functions = [pci(0x1002, 0x13c0, 3), pci(0x10de, 0x2c02, 3)];
    let without = select_for(&functions, Ports::NONE);
    assert!(
        without
            .wanted
            .iter()
            .all(|(artifact, _)| !theirs(artifact.member))
    );
    assert!(without.unknown.is_empty());
    assert!(
        without
            .wanted
            .iter()
            .any(|(artifact, _)| artifact.member.starts_with("nvidia/"))
    );
    let with = select_for(&functions, Ports::ALL);
    assert!(
        with.wanted
            .iter()
            .any(|(artifact, _)| theirs(artifact.member))
    );
    // This build's bundle holds them exactly when it has the port.
    assert_eq!(
        objects().iter().any(|artifact| theirs(artifact.member)),
        Ports::BUILT.amd
    );
}

#[test]
fn a_build_without_the_intel_port_bundles_none_of_its_firmware() {
    let theirs =
        |member: &str| member.starts_with("i915/") || member.starts_with("xe/");
    let without_port = objects_for(Ports::NONE);
    assert!(!without_port.is_empty());
    assert!(without_port.iter().all(|artifact| !theirs(artifact.member)));
    assert!(
        families_for(Ports::NONE)
            .flat_map(|(_, recipes)| recipes.iter())
            .all(|recipe| {
                let license = recipe.license.member;
                !(license.ends_with("LICENSE.i915") || license.ends_with("LICENSE.xe"))
            })
    );
    // Its GPUs select nothing; the rest is as with the port.
    let functions = [pci(0x8086, 0x46a6, 3), pci(0x10de, 0x2c02, 3)];
    let without = select_for(&functions, Ports::NONE);
    assert!(
        without
            .wanted
            .iter()
            .all(|(artifact, _)| !theirs(artifact.member))
    );
    assert!(without.unknown.is_empty());
    assert!(
        without
            .wanted
            .iter()
            .any(|(artifact, _)| artifact.member.starts_with("nvidia/"))
    );
    let with = select_for(&functions, Ports::ALL);
    assert!(
        with.wanted
            .iter()
            .any(|(artifact, _)| theirs(artifact.member))
    );
    // This build's bundle holds them exactly when it has the port.
    assert_eq!(
        objects().iter().any(|artifact| theirs(artifact.member)),
        Ports::BUILT.intel
    );
}

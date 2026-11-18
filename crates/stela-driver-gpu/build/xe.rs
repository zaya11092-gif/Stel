//! xe: the `xe-y` composite of its own Makefile with the display code it
//! shares with i915, compiled unchanged in Stela's Linux environment, and
//! the kernel services it imports that the environment does not yet supply.
use super::kbuild::{Config, Kbuild, Rule};
use super::native::{self, Linux};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
};
use stela_c_build::linux::Module;
use stela_c_build::linux_capture::{self as initcalls, Unit, UnitSpec};

const XE: &str = "../../third_party/linux/drivers/gpu/drm/xe";
const AUTOCONF: &str = "../stela-linux/src/compat/generated/autoconf.h";
const MODULE: Module<'static> = Module {
    name: "xe",
    file: "drivers/gpu/drm/xe/xe",
};
const COMPOSITE: &str = "xe";

/// The Makefile's rule that builds the display code shared with i915 from
/// i915's tree: an object below `i915-display/` is that tree's source.
const SHARED_DISPLAY: (&str, &str) = ("i915-display/", "drivers/gpu/drm/i915/display");
/// The host program the Makefile builds, and each rule file it turns into a
/// table of workarounds below `generated/`.
const GENERATOR: &str = "xe_gen_wa_oob";
const GENERATED: [&str; 2] = ["xe_wa_oob", "xe_device_wa_oob"];

/// The Makefile's rules, each carried out by [`port`] or by nothing: the
/// header self-test runs only with `DRM_XE_WERROR`, and a dependency of the
/// objects on the generated headers is this build's order.
const RULES: [(&str, &str, &[&str]); 5] = [
    (
        "$(obj)/generated/%_wa_oob.c $(obj)/generated/%_wa_oob.h",
        "$(obj)/xe_gen_wa_oob $(src)/xe_wa_oob.rules",
        &["$(call cmd,wa_oob)"],
    ),
    (
        "$(obj)/generated/%_device_wa_oob.c $(obj)/generated/%_device_wa_oob.h",
        "$(obj)/xe_gen_wa_oob $(src)/xe_device_wa_oob.rules",
        &["$(call cmd,device_wa_oob)"],
    ),
    (
        "$(obj)/i915-display/%.o",
        "$(srctree)/drivers/gpu/drm/i915/display/%.c FORCE",
        &[
            "$(call cmd,force_checksrc)",
            "$(call if_changed_rule,cc_o_c)",
        ],
    ),
    (
        "$(obj)/%.hdrtest",
        "$(src)/%.h FORCE",
        &["$(call if_changed_dep,hdrtest)"],
    ),
    (
        "$(uses_generated_oob)",
        "$(obj)/generated/xe_wa_oob.h $(obj)/generated/xe_device_wa_oob.h",
        &[],
    ),
];
/// The Makefile's variables without a selector that this build carries out,
/// as it writes them.
const DEFINITIONS: [(&str, &str); 5] = [
    (
        "cmd_device_wa_oob",
        "mkdir -p $(@D); $(obj)/xe_gen_wa_oob $(src)/xe_device_wa_oob.rules \
         $(generated_device_oob)",
    ),
    (
        "cmd_wa_oob",
        "mkdir -p $(@D); $(obj)/xe_gen_wa_oob $(src)/xe_wa_oob.rules $(generated_oob)",
    ),
    (
        "generated_device_oob",
        "$(obj)/generated/xe_device_wa_oob.c $(obj)/generated/xe_device_wa_oob.h",
    ),
    (
        "generated_oob",
        "$(obj)/generated/xe_wa_oob.c $(obj)/generated/xe_wa_oob.h",
    ),
    ("hostprogs", GENERATOR),
];
/// Its other variables, which name a command in Kbuild's output, belong to
/// the header self-test or list what depends on the generated headers.
const DESCRIPTIONS: [&str; 6] = [
    "cmd_hdrtest",
    "hdrtest_find_args",
    "quiet_cmd_device_wa_oob",
    "quiet_cmd_hdrtest",
    "quiet_cmd_wa_oob",
    "uses_generated_oob",
];

/// The kernel services the composite imports that Stela's Linux environment
/// lacks, or defines only as a trap, by service. Building fails when the
/// imports change, so each supplied service leaves this list with its change.
const OPEN: super::image::Open = &[
    // Nothing tears a pseudo filesystem's superblock down.
    (
        "reopening a file without a VFS path",
        &["stela_linux_reopen_unowned_path"],
    ),
    // As nouveau's and amdgpu's lists have them, through hwmon.c and
    // backlight.c: the display code's backlight devices and the hardware
    // monitor core ask the driver core's device lists, which the
    // environment traps.
    (
        "the driver core's device lists",
        &["class_find_device", "device_find_child"],
    ),
];

/// The unit of xe's own `module_init`.
const INITCALLS: UnitSpec = UnitSpec {
    scope: stela_c_build::linux_capture::Scope {
        slice: "gpu",
        group: "xe",
        group_ordinal: 33,
        unit_ordinal: 1,
        global_ordinal: 33,
    },
    source: "drivers/gpu/drm/xe/xe_module",
    module: MODULE.name,
    file: MODULE.file,
};
fn initcalls_unit() -> Unit {
    INITCALLS.into()
}

pub fn port(linux: &Linux) -> super::image::Driver {
    let config = Config::parse(&fs::read_to_string(AUTOCONF).expect("read Stela's Kconfig header"))
        .unwrap_or_else(|error| panic!("{AUTOCONF}: {error}"));
    let kbuild = Kbuild::read(Path::new(XE), &config).unwrap_or_else(|error| panic!("{error}"));
    for file in &kbuild.files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    check_makefile(&kbuild);
    let output = stela_c_build::linux_build::out_dir().join("xe");
    generate(&output);
    let sources = kbuild
        .objects(COMPOSITE)
        .unwrap_or_else(|error| panic!("{error}"))
        .iter()
        .map(|object| source(object))
        .filter(|source| *source != initcalls_unit().path())
        .collect::<Vec<_>>();
    assert!(
        kbuild.generated_include && !kbuild.werror,
        "xe's Makefile no longer searches its output directory, or selects -Werror"
    );
    let mut includes = vec![output.clone()];
    includes.extend(kbuild.includes.iter().cloned());
    includes.extend(
        kbuild
            .tree_includes
            .iter()
            .map(|directory| Path::new(native::LINUX).join(directory)),
    );
    let mut build = stela_c_build::native::configure();
    // The output directory is written by every build, so it is not watched.
    linux
        .environment
        .profile(&mut build, None, &includes, Some(&MODULE));
    for directory in [Path::new(XE).to_path_buf(), source("i915-display/x.o")] {
        let directory = directory.parent().unwrap_or(&directory);
        println!("cargo:rerun-if-changed={}", directory.display());
    }
    stela_c_build::linux_build::upstream_warnings(&mut build);
    for flag in &kbuild.optional_flags {
        build.flag_if_supported(flag);
    }
    // As Kbuild builds every profile: Linux relies on the optimizer to drop
    // the calls its configuration disables.
    build.opt_level(2).cargo_metadata(false);
    let jobs = env::var("NUM_JOBS")
        .ok()
        .and_then(|jobs| jobs.parse().ok())
        .unwrap_or(1)
        .max(1);
    let objects = thread::scope(|scope| {
        sources
            .chunks(sources.len().div_ceil(jobs))
            .map(|chunk| {
                let build = &build;
                scope.spawn(move || build.clone().files(chunk).compile_intermediates())
            })
            .collect::<Vec<_>>()
            .into_iter()
            .flat_map(|compiling| compiling.join().expect("a compiling thread"))
            .collect::<Vec<_>>()
    });
    let compiler = initcalls::compiler(&build);
    let placement = initcalls::Placement::new(
        &initcalls_unit().path(),
        &stela_c_build::linux_build::out_dir(),
        &[
            Path::new("../.."),
            compiler.parent().expect("compiler root"),
        ],
    )
    .expect("xe source placement");
    let captured = initcalls::compile(
        &build,
        Path::new("../stela-linux/src/compat/stela/registration.h"),
        INITCALLS,
        &placement,
    );
    build
        .objects(objects)
        .object(captured.object())
        .compile("stela-xe");
    // Hosted programs test the environment and link no driver.
    if env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "uefi") {
        println!("cargo:rustc-link-lib=static=stela-xe");
    }
    super::image::Driver {
        name: MODULE.name,
        spec: INITCALLS,
        captured,
        archive: stela_c_build::linux_build::out_dir().join("libstela-xe.a"),
        open: OPEN,
    }
}

/// An object's source: i915's tree for the display code xe shares with it,
/// xe's own otherwise.
fn source(object: &str) -> PathBuf {
    match object.strip_prefix(SHARED_DISPLAY.0) {
        Some(shared) => Path::new(native::LINUX).join(SHARED_DISPLAY.1).join(shared),
        None => Path::new(XE).join(object),
    }
    .with_extension("c")
}

/// Refuse a Makefile whose rules or variables are not the ones this build
/// carries out.
fn check_makefile(kbuild: &Kbuild) {
    let rules = RULES
        .iter()
        .map(|(targets, prerequisites, recipe)| Rule {
            targets: (*targets).to_owned(),
            prerequisites: (*prerequisites).to_owned(),
            recipe: recipe.iter().map(|line| (*line).to_owned()).collect(),
        })
        .collect::<Vec<_>>();
    assert!(
        kbuild.rules == rules,
        "xe's Makefile has other rules: {:?}",
        kbuild.rules
    );
    let mut names = DEFINITIONS
        .iter()
        .map(|(name, _)| *name)
        .chain(DESCRIPTIONS)
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert!(
        kbuild.definitions.keys().map(String::as_str).eq(names),
        "xe's Makefile has other variables: {:?}",
        kbuild.definitions.keys()
    );
    for (name, value) in DEFINITIONS {
        assert!(
            kbuild.definitions[name] == value,
            "xe's Makefile changed {name}: {}",
            kbuild.definitions[name]
        );
    }
}

/// Build the Makefile's host program for this machine and run it over each
/// rule file, as `cmd_wa_oob` and `cmd_device_wa_oob` do.
fn generate(output: &Path) {
    let generated = output.join("generated");
    fs::create_dir_all(&generated).expect("create xe's generated directory");
    let program = output.join(GENERATOR);
    let source = Path::new(XE).join(GENERATOR).with_extension("c");
    println!("cargo:rerun-if-changed={}", source.display());
    let host = env::var("HOST").expect("Cargo supplies HOST");
    let mut compiler = cc::Build::new();
    compiler
        .target(&host)
        .host(&host)
        .opt_level(1)
        .cargo_metadata(false);
    stela_c_build::linux_build::run(
        compiler
            .get_compiler()
            .to_command()
            .arg(&source)
            .arg("-o")
            .arg(&program),
    );
    for name in GENERATED {
        let rules = Path::new(XE).join(name).with_extension("rules");
        println!("cargo:rerun-if-changed={}", rules.display());
        stela_c_build::linux_build::run(
            Command::new(&program)
                .arg(&rules)
                .arg(generated.join(name).with_extension("c"))
                .arg(generated.join(name).with_extension("h")),
        );
    }
}

/// Linux's Intel PCI device ID lists, which xe's table is written with.
const PCI_IDS: &str = "../../third_party/linux/include/drm/intel/pciids.h";

/// Write xe's PCI ID table for the Rust side (`src/xe.rs`): each device ID
/// with the DMA mask bits of its platform's descriptor and whether that
/// descriptor binds only by force probe, read from the
/// driver's own `pciidlist` and Linux's ID lists. A clippy build reads it
/// too, so it is written before any C is compiled.
pub fn tables() {
    let table = Path::new(XE).join("xe_pci.c");
    for watched in [table.as_path(), Path::new(PCI_IDS)] {
        println!("cargo:rerun-if-changed={}", watched.display());
    }
    let devices = devices(
        &fs::read_to_string(&table).expect("read xe's PCI table"),
        &fs::read_to_string(PCI_IDS).expect("read Linux's Intel PCI IDs"),
    )
    .unwrap_or_else(|error| panic!("xe's PCI ID table: {error}"));
    let mut text = String::from(
        "/// xe's `pciidlist`: each PCI device ID with its platform's `dma_mask_size`\n\
         /// and `require_force_probe`.\n\
         pub(crate) const DEVICES: &[(u16, u8, bool)] = &[\n",
    );
    for (device, (bits, forced, _, _, _)) in &devices {
        text.push_str(&format!("    ({device:#06x}, {bits}, {forced}),\n"));
    }
    text.push_str(
        "];\n/// Each device ID's platform name (`PLATFORM()`), `has_display` and\n\
         /// `is_dgfx`.\n\
         pub(crate) const PLATFORMS: &[(u16, &str, bool, bool)] = &[\n",
    );
    for (device, (_, _, platform, display, discrete)) in &devices {
        text.push_str(&format!(
            "    ({device:#06x}, {platform:?}, {display}, {discrete}),\n"
        ));
    }
    text.push_str("];\n");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR"));
    fs::write(output.join("xe_devices.rs"), text).expect("write xe's device table");
}

/// Each device ID of `pciidlist` in `table` with the mask bits, the
/// force-probe requirement, the platform name, whether it has a display
/// engine and whether it is a discrete GPU (`is_dgfx`), of the descriptor
/// its entry names, the ID lists expanded from `ids`.
type Described = (u8, bool, String, bool, bool);
fn devices(table: &str, ids: &str) -> Result<BTreeMap<u16, Described>, String> {
    // Each list macro's own IDs and the list macros it includes.
    let mut lists = BTreeMap::new();
    for definition in ids.replace("\\\n", " ").lines() {
        let Some(rest) = definition.strip_prefix("#define ") else {
            continue;
        };
        let Some((name, body)) = rest.split_once("(MACRO__, ...)") else {
            continue;
        };
        let (mut own, mut included) = (Vec::new(), Vec::new());
        // Each call in the body: `MACRO__(0x1234, ...)` is an ID of this
        // list, `OTHER_IDS(MACRO__, ...)` includes another list.
        let pieces = body.split('(').collect::<Vec<_>>();
        for call in pieces.windows(2) {
            let callee = call[0]
                .rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .next()
                .unwrap_or("");
            let argument = call[1].split([',', ')']).next().unwrap_or("").trim();
            if callee == "MACRO__" {
                let id = argument
                    .strip_prefix("0x")
                    .or_else(|| argument.strip_prefix("0X"))
                    .and_then(|digits| u16::from_str_radix(digits, 16).ok())
                    .ok_or_else(|| format!("{name} lists {argument:?}"))?;
                own.push(id);
            } else if argument == "MACRO__" {
                included.push(callee.to_owned());
            }
        }
        lists.insert(name.trim().to_owned(), (own, included));
    }
    // The table's object-like macros, which descriptors include whole
    // (`DG2_FEATURES`).
    let joined = table.replace("\\\n", " ");
    let macros: BTreeMap<&str, &str> = joined
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("#define "))
        .filter_map(|rest| rest.split_once(|c: char| c.is_whitespace()))
        .filter(|(name, _)| name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        .collect();
    // Each descriptor's mask.
    let mut masks = BTreeMap::new();
    for descriptor in table.split("struct xe_device_desc ").skip(1) {
        let Some((name, body)) = descriptor.split_once(" = {") else {
            continue;
        };
        // A descriptor's definition, not a pointer to one in a signature.
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let mut body = body.split("};").next().unwrap_or("").to_owned();
        // Included macros may include others (`DG2_FEATURES` names
        // `DGFX_FEATURES`); each is expanded once.
        let mut expanded = BTreeSet::new();
        loop {
            let included: Vec<(&str, &str)> = body
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .filter_map(|word| macros.get_key_value(word).map(|(n, d)| (*n, *d)))
                .filter(|(name, _)| !expanded.contains(name))
                .collect();
            if included.is_empty() {
                break;
            }
            for (name, definition) in included {
                if expanded.insert(name) {
                    body.push_str(",\n");
                    body.push_str(definition);
                }
            }
        }
        if let Some(bits) = body.split(".dma_mask_size = ").nth(1) {
            let bits = bits.split(',').next().unwrap_or("").trim();
            let platform = body
                .split("PLATFORM(")
                .nth(1)
                .and_then(|rest| rest.split(')').next())
                .map(str::trim)
                .filter(|platform| !platform.is_empty())
                .ok_or_else(|| format!("{name} names no platform"))?;
            masks.insert(
                name.trim().to_owned(),
                (
                    bits.parse::<u8>()
                        .map_err(|_| format!("{name} has mask {bits:?}"))?,
                    body.contains(".require_force_probe = true"),
                    platform.to_owned(),
                    body.contains(".has_display = true"),
                    [".is_dgfx = 1", ".is_dgfx = true"]
                        .iter()
                        .any(|mark| body.contains(mark)),
                ),
            );
        }
    }
    let entries = table
        .split("pciidlist[] = {")
        .nth(1)
        .and_then(|rest| rest.split("{ }").next())
        .ok_or("no pciidlist")?;
    let mut devices = BTreeMap::new();
    for entry in entries
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let (list, descriptor) = entry
            .split_once('(')
            .and_then(|(list, rest)| Some((list, rest.split_once('&')?.1)))
            .ok_or_else(|| format!("entry {entry:?}"))?;
        let descriptor = descriptor.trim_end_matches("),");
        let described = masks
            .get(descriptor)
            .ok_or_else(|| format!("{descriptor} has no dma_mask_size"))?;
        let mut pending = vec![list.to_owned()];
        while let Some(list) = pending.pop() {
            let (own, included) = lists.get(&list).ok_or_else(|| format!("no list {list}"))?;
            pending.extend(included.iter().cloned());
            for id in own {
                if devices
                    .insert(*id, described.clone())
                    .is_some_and(|other| other != *described)
                {
                    return Err(format!("{id:#06x} has two descriptors"));
                }
            }
        }
    }
    if devices.is_empty() {
        return Err("pciidlist is empty".into());
    }
    Ok(devices)
}

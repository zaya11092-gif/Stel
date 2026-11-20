//! nouveau: the `nouveau-y` composite of its own Kbuild, compiled unchanged in
//! Stela's Linux environment, and the kernel services it imports that the
//! environment does not yet supply.
use super::kbuild::{Config, Kbuild};
use super::native::{self, Linux};
use std::{env, fs, path::Path, thread};
use stela_c_build::linux::Module;
use stela_c_build::linux_capture::{self as initcalls, Unit, UnitSpec};

const NOUVEAU: &str = "../../third_party/linux/drivers/gpu/drm/nouveau";
const AUTOCONF: &str = "../stela-linux/src/compat/generated/autoconf.h";
const MODULE: Module<'static> = Module {
    name: "nouveau",
    file: "drivers/gpu/drm/nouveau/nouveau",
};
/// Kbuild's composite: nouveau's DRM layer over NVIF and NVKM.
const COMPOSITES: [&str; 1] = ["nouveau"];

/// The kernel services the composites import that Stela's Linux environment
/// lacks, or defines only as a trap, by service. Building fails when the
/// imports change, so each supplied service leaves this list with its change.
const OPEN: super::image::Open = &[
    // Nothing tears a pseudo filesystem's superblock down.
    (
        "reopening a file without a VFS path",
        &["stela_linux_reopen_unowned_path"],
    ),
];

/// The unit of nouveau's own `module_init`.
const INITCALLS: UnitSpec = UnitSpec {
    scope: stela_c_build::linux_capture::Scope {
        slice: "gpu",
        group: "nouveau",
        group_ordinal: 32,
        unit_ordinal: 1,
        global_ordinal: 32,
    },
    source: "drivers/gpu/drm/nouveau/nouveau_drm",
    module: MODULE.name,
    file: MODULE.file,
};
fn initcalls_unit() -> Unit {
    INITCALLS.into()
}

pub fn port(linux: &Linux) -> super::image::Driver {
    let config = Config::parse(&fs::read_to_string(AUTOCONF).expect("read Stela's Kconfig header"))
        .unwrap_or_else(|error| panic!("{AUTOCONF}: {error}"));
    let kbuild =
        Kbuild::read(Path::new(NOUVEAU), &config).unwrap_or_else(|error| panic!("{error}"));
    for file in &kbuild.files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    let mut sources = Vec::new();
    for composite in COMPOSITES {
        let objects = kbuild
            .objects(composite)
            .unwrap_or_else(|error| panic!("{error}"));
        sources.extend(
            objects
                .iter()
                .map(|object| Path::new(NOUVEAU).join(object).with_extension("c")),
        );
    }
    let includes = kbuild
        .includes
        .iter()
        .map(|directory| {
            let relative = directory
                .strip_prefix(NOUVEAU)
                .expect("Kbuild includes lie in nouveau's tree");
            Path::new("nouveau").join(relative).display().to_string()
        })
        .collect::<Vec<_>>();
    let includes = includes.iter().map(String::as_str).collect::<Vec<_>>();
    // Compiled below, with its registrations captured.
    sources.retain(|source| *source != initcalls_unit().path());
    let mut build = stela_c_build::native::configure();
    linux.environment.apply(
        &mut build,
        None,
        &native::drm_includes(&includes),
        Some(&MODULE),
    );
    stela_c_build::linux_build::upstream_warnings(&mut build);
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
    .expect("Nouveau source placement");
    let captured = initcalls::compile(
        &build,
        Path::new("../stela-linux/src/compat/stela/registration.h"),
        INITCALLS,
        &placement,
    );
    build
        .objects(objects)
        .object(captured.object())
        .compile("stela-nouveau-nvkm");
    // Hosted programs test the environment and link no driver.
    if env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "uefi") {
        println!("cargo:rustc-link-lib=static=stela-nouveau-nvkm");
    }
    super::image::Driver {
        name: MODULE.name,
        spec: INITCALLS,
        captured,
        extra: Vec::new(),
        archive: stela_c_build::linux_build::out_dir().join("libstela-nouveau-nvkm.a"),
        open: OPEN,
    }
}

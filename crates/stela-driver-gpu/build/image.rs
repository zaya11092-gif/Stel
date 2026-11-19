//! The one registration image of a build: the environment's units (common,
//! then DRM's) and the unit of each kernel driver built, in ordinal order, as
//! one Linux kernel registers every driver built into it. With it, what the
//! image and its drivers import that the environment does not supply is
//! compared with the drivers' lists.
use super::native::{self, Linux};
use std::{collections::BTreeSet, env, iter::once, path::PathBuf};
use stela_c_build::linux_capture::{self as initcalls, Captured, Projection, UnitSpec};

/// Ordinals of the image's units after the common cohort: DRM's own level
/// (its core, helpers and buses) holds 5 to 31, and each driver its units
/// from 32 on: nouveau 32, xe 33 and 34 (its display parameters), amdgpu 35.
pub const FIRST_DRIVER_ORDINAL: u32 = 32;

/// The kernel services a driver's objects import that the environment lacks
/// or defines only as a trap, by service.
pub type Open = &'static [(&'static str, &'static [&'static str])];

/// One kernel driver compiled for the image.
pub struct Driver {
    /// Its module name, for the check's message.
    pub name: &'static str,
    /// The unit of its own `module_init`, compiled with its registrations
    /// captured.
    pub spec: UnitSpec,
    pub captured: Captured,
    /// The module's further captured units, each a reviewed source of its
    /// registrations, in `global_ordinal` order after `spec`'s: e.g. the
    /// display parameters the boot's kernel command line names. A unit's
    /// plain compilation would leave its registrations to the original
    /// section emission, which Stela's environment never reads.
    pub extra: Vec<(UnitSpec, Captured)>,
    /// Its objects, with the captured units'.
    pub archive: PathBuf,
    pub open: Open,
}

/// Project the image over the environment's units and `drivers`', and check
/// what it reaches.
pub fn link(linux: &Linux, mut drivers: Vec<Driver>) {
    drivers.sort_by_key(|driver| driver.spec.scope.global_ordinal);
    assert!(
        drivers
            .iter()
            .all(|driver| driver.spec.scope.global_ordinal >= FIRST_DRIVER_ORDINAL),
        "a driver's unit lies in DRM's ordinals"
    );
    let mut captured = linux.captured.clone();
    let environment_count = captured.len();
    captured.extend(drivers.iter().flat_map(|driver| {
        once(driver.captured.clone())
            .chain(driver.extra.iter().map(|(_, captured)| captured.clone()))
    }));
    let mut expected = native::environment_units();
    expected.extend(drivers.iter().flat_map(|driver| {
        once(driver.spec).chain(driver.extra.iter().map(|(spec, _)| *spec))
    }));
    let image = initcalls::source(
        &captured,
        Projection {
            symbol: "stela_linux_gpu_image",
            expected: &expected,
            imported_units: environment_count,
            descriptors_only: false,
        },
    );
    let mut build = stela_c_build::native::configure();
    linux
        .environment
        .apply(&mut build, None, &native::drm_includes(&[]), None);
    stela_c_build::linux_build::upstream_warnings(&mut build);
    build
        .opt_level(2)
        .cargo_metadata(false)
        .file(image)
        .compile("stela-linux-gpu-image");
    let output = stela_c_build::linux_build::out_dir();
    let archive = output.join("libstela-linux-gpu-image.a");
    let mut ports = vec![archive.as_path()];
    ports.extend(drivers.iter().map(|driver| driver.archive.as_path()));
    let mut open = super::imports::open_all(&ports, &[&linux.archive, &linux.common_archive]);
    let listed = drivers
        .iter()
        .flat_map(|driver| driver.open.iter())
        .flat_map(|(_, symbols)| symbols.iter().copied())
        .collect::<BTreeSet<_>>();
    let stale = listed
        .iter()
        .copied()
        .filter(|symbol| !open.remove(*symbol))
        .collect::<Vec<_>>();
    let names = drivers
        .iter()
        .map(|driver| driver.name)
        .collect::<Vec<_>>()
        .join(" and ");
    assert!(
        open.is_empty() && stale.is_empty(),
        "the imports of {names} changed: newly open {open:?}; now supplied {stale:?}"
    );
    let digest = initcalls::write(
        &output.join("captured-units.bin"),
        &captured[initcalls::COMMON.len()..],
    );
    println!("cargo:output={}", output.display());
    println!("cargo:cohort_sha256={digest}");
    // Hosted programs test the environment and link no driver.
    if env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "uefi") {
        println!("cargo:rustc-link-lib=static=stela-linux-gpu-image");
    }
}

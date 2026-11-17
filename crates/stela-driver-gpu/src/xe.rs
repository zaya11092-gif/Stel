//! xe as a stack's kernel driver: what the boot asks about an Intel GPU
//! before Linux sees it, answered from xe's own PCI ID table as the build
//! read it (`build/xe.rs`).
use crate::{Adapter, stack::Control};

include!(concat!(env!("OUT_DIR"), "/xe_devices.rs"));

/// The bits of the DMA mask xe sets for `device`, its platform's
/// `dma_mask_size`, if xe's table holds the device.
fn mask_bits(device: u16) -> Option<u8> {
    DEVICES
        .binary_search_by_key(&device, |(id, _, _)| *id)
        .ok()
        .map(|index| DEVICES[index].1)
}

/// Whether xe binds `device` only because the profile sets Linux's force
/// probe for every device (`CONFIG_DRM_XE_FORCE_PROBE`): its table marks the
/// platform `require_force_probe`, which upstream uses for the platforms
/// where xe is experimental and i915 is the supported driver.
pub fn force_probed(device: u16) -> bool {
    DEVICES
        .binary_search_by_key(&device, |(id, _, _)| *id)
        .is_ok_and(|index| DEVICES[index].2)
}

/// Whether xe's table holds the adapter. Intel graphics before Tiger Lake
/// are i915's, which Stela does not have.
pub fn admits(adapter: &Adapter) -> Result<(), &'static str> {
    match mask_bits(adapter.device) {
        Some(_) => Ok(()),
        None => Err(
            "xe's PCI ID table does not hold this Intel GPU (Ice Lake and older need i915, which is not ported to Stela)",
        ),
    }
}

/// The DMA mask xe sets on the adapter. One outside its table is never
/// presented to it ([`admits`]); it gets the narrowest mask of any platform.
pub fn dma_bits(adapter: &Adapter) -> u8 {
    mask_bits(adapter.device)
        .or_else(|| DEVICES.iter().map(|(_, bits, _)| *bits).min())
        .unwrap_or(32)
}

/// What xe's table says of a device's platform.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Platform {
    /// Its name (`PLATFORM()` in `xe_pci.c`).
    pub name: &'static str,
    /// Whether it has a display engine (`has_display`).
    pub display: bool,
    /// Whether it is a discrete GPU with memory of its own (`is_dgfx`),
    /// rather than one built into the processor.
    pub discrete: bool,
}

/// What xe's table says of `device`'s platform, if it holds the device.
pub fn platform(device: u16) -> Option<Platform> {
    PLATFORMS
        .binary_search_by_key(&device, |(id, ..)| *id)
        .ok()
        .map(|index| Platform {
            name: PLATFORMS[index].1,
            display: PLATFORMS[index].2,
            discrete: PLATFORMS[index].3,
        })
}

/// Whether xe's table describes the adapter as built into the processor.
/// One outside the table is never presented to xe ([`admits`]).
pub fn integrated(adapter: &Adapter) -> bool {
    platform(adapter.device).is_some_and(|platform| !platform.discrete)
}

/// xe's device query (`DRM_IOCTL_XE_DEVICE_QUERY`, `uapi/drm/xe_drm.h`).
pub mod query {
    use super::Control;
    use alloc::vec;
    use core::ffi::c_void;

    /// `DRM_IOWR(DRM_COMMAND_BASE + DRM_XE_DEVICE_QUERY, struct
    /// drm_xe_device_query)`.
    const DEVICE_QUERY: u32 = 0xc028_6440;
    const QUERY_MEM_REGIONS: u32 = 1;
    const QUERY_CONFIG: u32 = 2;
    /// `drm_xe_query_config.info[DRM_XE_QUERY_CONFIG_REV_AND_DEVICE_ID]`.
    const CONFIG_REV_AND_DEVICE_ID: usize = 0;
    /// `struct drm_xe_mem_region` in 64-bit words, and its class's value
    /// for device memory.
    const REGION_WORDS: usize = 11;
    const CLASS_VRAM: u16 = 1;

    /// `struct drm_xe_device_query`.
    #[repr(C)]
    #[derive(Default)]
    struct DeviceQuery {
        extensions: u64,
        query: u32,
        size: u32,
        data: u64,
        reserved: [u64; 2],
    }

    /// What `query` answers, in 64-bit words: asked for its size, then
    /// filled. None when the driver refuses either or answers nothing.
    fn ask(control: Control, query: u32) -> Option<alloc::vec::Vec<u64>> {
        let mut request = DeviceQuery {
            query,
            ..DeviceQuery::default()
        };
        if control(DEVICE_QUERY, (&raw mut request).cast::<c_void>()) != 0 || request.size == 0 {
            return None;
        }
        let mut words = vec![0u64; (request.size as usize).div_ceil(8)];
        request.data = words.as_mut_ptr() as u64;
        (control(DEVICE_QUERY, (&raw mut request).cast::<c_void>()) == 0).then_some(words)
    }

    /// The bound GPU's PCI device ID, from the configuration query.
    pub fn chip(control: Control) -> Option<u32> {
        // `num_params` and `pad`, then the parameters.
        let words = ask(control, QUERY_CONFIG)?;
        let device = words.get(1 + CONFIG_REV_AND_DEVICE_ID)? & 0xffff;
        (device != 0).then_some(device as u32)
    }

    /// The device memory xe manages: its VRAM regions' total, or on a GPU
    /// without VRAM the system memory region it allocates from.
    pub fn memory_bytes(control: Control) -> Option<u64> {
        let words = ask(control, QUERY_MEM_REGIONS)?;
        let count = (*words.first()? & 0xffff_ffff) as usize;
        let regions = words.get(1..1 + count.checked_mul(REGION_WORDS)?)?;
        let (mut vram, mut system) = (0u64, 0u64);
        for region in regions.as_chunks::<REGION_WORDS>().0 {
            // `mem_class`, `instance` and `min_page_size`, then `total_size`.
            let total = region[1];
            if region[0] as u16 == CLASS_VRAM {
                vram = vram.saturating_add(total);
            } else {
                system = system.max(total);
            }
        }
        let bytes = if vram != 0 { vram } else { system };
        (bytes != 0).then_some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn the_table_is_xes_own_sorted_with_each_platforms_mask() {
        assert!(DEVICES.windows(2).all(|pair| pair[0].0 < pair[1].0));
        // Tiger Lake, Alder Lake P (the i7-1265U's Iris Xe among them),
        // Raptor Lake P and DG1: 39 bits.
        for device in [0x9a49, 0x46a6, 0x46a8, 0xa7a0, 0x4905] {
            assert_eq!(mask_bits(device), Some(39), "{device:04x}");
        }
        // Arc A-series, Meteor Lake, Lunar Lake and Battlemage: 46 bits.
        for device in [0x56a0, 0x7d55, 0x64a0, 0xe20b] {
            assert_eq!(mask_bits(device), Some(46), "{device:04x}");
        }
        // Ice Lake and Ivy Bridge are not xe's.
        for device in [0x8a56, 0x0166] {
            assert_eq!(mask_bits(device), None, "{device:04x}");
        }
    }

    #[test]
    fn each_device_has_its_descriptors_platform_name() {
        assert_eq!(PLATFORMS.len(), DEVICES.len());
        let name = |device| platform(device).map(|platform| platform.name);
        assert_eq!(name(0x9a49), Some("TIGERLAKE"));
        // Alder Lake P is the platform of the i7-1265U's Iris Xe 0x46a8.
        assert_eq!(name(0x46a8), Some("ALDERLAKE_P"));
        // Raptor Lake P is a subplatform of Alder Lake P in xe's table.
        assert_eq!(name(0xa7a0), Some("ALDERLAKE_P"));
        assert_eq!(name(0x56a0), Some("DG2"));
        assert_eq!(name(0xe20b), Some("BATTLEMAGE"));
        assert_eq!(name(0x8a56), None);
        assert!(platform(0x9a49).unwrap().display);
        // The Data Center GPU Flex 170 (ATS-M) has no display engine.
        assert!(!platform(0x56c0).unwrap().display);
    }

    #[test]
    fn discrete_is_the_descriptors_is_dgfx_through_its_macros() {
        // DG1 sets it through `DGFX_FEATURES`, the Arc A-series and Flex
        // through `DG2_FEATURES`, which includes that, and Battlemage
        // directly; Tiger Lake, Alder Lake P, Meteor Lake and Lunar Lake are
        // built into the processor.
        for device in [0x4905, 0x56a0, 0x56c0, 0xe20b] {
            assert!(platform(device).unwrap().discrete, "{device:04x}");
        }
        for device in [0x9a49, 0x46a6, 0x46a8, 0x7d55, 0x64a0] {
            assert!(!platform(device).unwrap().discrete, "{device:04x}");
        }
        let adapter = |device| Adapter {
            address: crate::bus::pci::Address {
                segment: 0,
                bus: 0,
                device: 2,
                function: 0,
            },
            vendor: crate::Vendor::Intel,
            device,
            class: [3, 0, 0],
        };
        assert!(integrated(&adapter(0x9a49)));
        assert!(!integrated(&adapter(0x56a0)));
        // Outside the table: no claim that it is built in.
        assert!(!integrated(&adapter(0x8a56)));
    }

    /// A driver answering `DRM_IOCTL_XE_DEVICE_QUERY` with `words` for the
    /// query it holds, and refusing others.
    fn answering(held: u32, words: &[u64]) -> impl FnMut(u32, *mut core::ffi::c_void) -> i32 + '_ {
        move |request, argument| {
            assert_eq!(request, 0xc028_6440);
            // SAFETY: the query passes its `struct drm_xe_device_query`.
            let query = unsafe { &mut *argument.cast::<[u64; 5]>() };
            let (kind, size) = (query[1] as u32, (query[1] >> 32) as u32);
            if kind != held {
                return -22;
            }
            if size == 0 {
                query[1] |= ((words.len() * 8) as u64) << 32;
            } else {
                assert_eq!(size as usize, words.len() * 8);
                // SAFETY: the query's buffer holds `size` bytes.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        words.as_ptr(),
                        query[2] as *mut u64,
                        words.len(),
                    )
                };
            }
            0
        }
    }

    #[test]
    fn the_chip_is_the_configurations_device_id() {
        // Revision 4 of Tiger Lake 0x9a49, then the other parameters.
        let config = [5, 0x0004_9a49, 0, 1 << 16, 48, 2];
        assert_eq!(query::chip(&mut answering(2, &config)), Some(0x9a49));
        assert_eq!(query::chip(&mut answering(1, &config)), None);
        assert_eq!(query::chip(&mut answering(2, &[0])), None);
    }

    #[test]
    fn memory_is_vram_or_else_the_system_region() {
        let region = |class: u64, total: u64| {
            let mut words = [0u64; 11];
            words[0] = class | 4096 << 32;
            words[1] = total;
            words
        };
        let mut integrated = vec![1];
        integrated.extend(region(0, 16 << 30));
        assert_eq!(
            query::memory_bytes(&mut answering(1, &integrated)),
            Some(16 << 30)
        );
        let mut discrete = vec![3];
        discrete.extend(region(0, 32 << 30));
        discrete.extend(region(1, 8 << 30));
        discrete.extend(region(1 | 1 << 16, 4 << 30));
        assert_eq!(
            query::memory_bytes(&mut answering(1, &discrete)),
            Some(12 << 30)
        );
        // A count past the answer is no answer.
        assert_eq!(query::memory_bytes(&mut answering(1, &[2])), None);
        assert_eq!(query::memory_bytes(&mut answering(2, &integrated)), None);
    }

    #[test]
    fn force_probe_is_the_tables_mark_of_each_platform() {
        // Tiger Lake, Alder Lake P, Raptor Lake P, DG1, Arc A-series and
        // Meteor Lake bind only by force probe.
        for device in [0x9a49, 0x46a6, 0xa7a0, 0x4905, 0x56a0, 0x7d55] {
            assert!(force_probed(device), "{device:04x}");
        }
        // Lunar Lake and Battlemage bind without it; a device outside the
        // table is not xe's at all.
        for device in [0x64a0, 0xe20b, 0x8a56] {
            assert!(!force_probed(device), "{device:04x}");
        }
    }
}

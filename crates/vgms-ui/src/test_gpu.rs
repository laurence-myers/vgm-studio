//! Shared GPU harness support for the snapshot tests (`app_gui_tests` and
//! `theme_showcase`).
//!
//! The snapshot tests render egui through wgpu. egui_kittest's stock
//! `Harness::builder().wgpu()` builds a *fresh* wgpu instance, adapter and
//! device for **every** test, and its default setup enumerates the GL and
//! Vulkan backends alongside DX12 on the way. On a machine whose OpenGL and
//! Vulkan drivers are one and the same DLL (NVIDIA ships both as `nvoglv64.dll`
//! here), that pulls a heavy driver into the test process and costs a few
//! hundred MB of committed memory per concurrent test. When the box is already
//! near its commit limit (several test binaries plus parallel `rustc` jobs), the
//! next allocation loses the race for pagefile growth and a process dies -- the
//! "flaky wgpu test" crashes recorded in the Windows Application event log
//! (0xC0000005 inside the driver, 0xC0000409 "memory allocation of N bytes
//! failed", 0x0000087d raised from `KERNELBASE`).
//!
//! This module removes the cause rather than papering over it:
//!
//! 1. **DX12 only.** The shared instance enables just the DX12 backend, so the
//!    GL and Vulkan ICDs never load. The baselines already render through the
//!    DX12 WARP (CPU) adapter, so the pixels are unchanged.
//! 2. **One shared device.** Every snapshot harness reuses a single
//!    instance/adapter/device/queue via [`WgpuSetup::Existing`]. Each harness
//!    still gets its own egui `Renderer` (and thus its own font atlas), so
//!    nothing that would make renders interfere is shared -- only the expensive
//!    device is. Per-test cost drops from a whole device to a few MB.
//! 3. **Bounded render concurrency.** A small permit pool caps how many tests
//!    render at once, keeping the peak (a render target plus a readback buffer
//!    per render) well clear of the commit limit even when several test binaries
//!    run side by side.

use std::sync::{Condvar, Mutex, OnceLock};

use eframe::egui_wgpu::{WgpuSetup, WgpuSetupExisting};
use eframe::wgpu;

/// The one DX12/WARP instance+adapter+device+queue every snapshot test shares,
/// built on first use.
fn shared_existing() -> &'static WgpuSetupExisting {
    static SHARED: OnceLock<WgpuSetupExisting> = OnceLock::new();
    SHARED.get_or_init(|| {
        // DX12 only: the GL and Vulkan ICDs (one and the same `nvoglv64.dll` on
        // an NVIDIA box) never load into the test process. Built without reading
        // `WGPU_BACKEND`, so the backend set stays deterministic.
        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
        instance_desc.backends = wgpu::Backends::DX12;
        let instance = wgpu::Instance::new(instance_desc);

        // `force_fallback_adapter` selects WARP -- the CPU rasterizer the
        // baselines (and CI) already render through -- so the shared device is
        // pixel-identical to kittest's CPU-preferring selector, just DX12-only.
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::None,
            force_fallback_adapter: true,
            compatible_surface: None,
        }))
        .expect("no DX12 WARP adapter available for the snapshot tests");

        // Mirror egui_kittest's default device limits so anything capability
        // dependent renders the same as the baselines.
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("vgms snapshot shared device"),
            required_limits: wgpu::Limits {
                max_texture_dimension_2d: 8192,
                ..wgpu::Limits::default()
            },
            ..Default::default()
        }))
        .expect("failed to create the shared snapshot device");

        WgpuSetupExisting {
            instance,
            adapter,
            device,
            queue,
        }
    })
}

/// The wgpu setup every snapshot harness should build with, in place of
/// `Harness::builder().wgpu()`. Reuses the shared DX12/WARP device; each call
/// still yields a fresh renderer.
pub(crate) fn shared_wgpu_setup() -> WgpuSetup {
    WgpuSetup::Existing(shared_existing().clone())
}

/// How many snapshot tests may render on the shared device at once. The shared
/// device already removes the per-test memory blow-up; this bounds the residual
/// peak (a render target plus a readback buffer per render) and the contention
/// on the device's wait-for-idle poll. Roughly the physical core count.
const MAX_CONCURRENT_RENDERS: usize = 4;

#[derive(Debug)]
struct RenderGate {
    free: Mutex<usize>,
    released: Condvar,
}

static RENDER_GATE: RenderGate = RenderGate {
    free: Mutex::new(MAX_CONCURRENT_RENDERS),
    released: Condvar::new(),
};

/// Held for the duration of one snapshot render; hands its permit back on drop,
/// including when an unwinding snapshot-mismatch panic tears the test down.
#[derive(Debug)]
pub(crate) struct RenderPermit;

impl Drop for RenderPermit {
    fn drop(&mut self) {
        *RENDER_GATE.free.lock().expect("render gate poisoned") += 1;
        RENDER_GATE.released.notify_one();
    }
}

/// Acquire a render permit, blocking while all [`MAX_CONCURRENT_RENDERS`] are in
/// use. Hold the returned guard across the `harness.snapshot(...)` call.
pub(crate) fn render_permit() -> RenderPermit {
    let mut free = RENDER_GATE.free.lock().expect("render gate poisoned");
    while *free == 0 {
        free = RENDER_GATE
            .released
            .wait(free)
            .expect("render gate poisoned");
    }
    *free -= 1;
    RenderPermit
}

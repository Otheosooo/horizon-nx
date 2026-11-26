//! Nintendo Switch (Horizon OS) runtime for Horizon: the libnx lifecycle hooks, boot/exit
//! logging, crash handler, heap reporting and C shims the `aarch64-nintendo-horizon` target
//! needs, ported from bevy-horizon's `bevy_horizon` crate with the Bevy parts removed.
//!
//! Linking this crate is what makes an NRO work: its `#[no_mangle]` `userAppInit`/`userAppExit`,
//! `__wrap_threadCreate`/`__wrap_fchmod`/`__wrap_rename`/`__wrap_rmdir` (the target spec's
//! `--wrap=` pre-link args), `getrandom`/`_getentropy_r`/`sysconf`/`_init`/`_fini` and exception
//! handlers come from `compat`, and its `build.rs` adds the devkitA64 link-search paths and
//! `-lnx -lsysbase -lc -lm -lgcc` that nothing else in the Horizon graph emits.
//!
//! `init` runs the service setup (romfs, BSD sockets, applet focus mode) and starts the log
//! writer; call it first thing in `main`. `flush_logs` and `process_applet_messages` are for the
//! game loop (Phase 2); `software_keyboard` shows the system keyboard for text entry.

pub mod compat;
pub mod lifecycle;
pub mod logging;
/// Symbolic links on the SD card (FAT has none), for the pack and mod overlays.
#[cfg(any(target_os = "horizon", test))]
pub mod vlinks;

#[cfg(target_os = "horizon")]
mod memory;
#[cfg(all(target_os = "horizon", feature = "top_down_alloc"))]
mod allocator;
#[cfg(all(target_os = "horizon", feature = "vulkan"))]
pub mod nvk_tune;
#[cfg(target_os = "horizon")]
pub mod profiler;
#[cfg(target_os = "horizon")]
mod boost;
#[cfg(target_os = "horizon")]
mod clocks;
#[cfg(target_os = "horizon")]
mod keyboard;
#[cfg(target_os = "horizon")]
mod motion;
/// HD rumble on the Joy-Cons, step by step.
#[cfg(target_os = "horizon")]
pub mod rumble;
/// The system's controller screen, for splitscreen's players.
#[cfg(target_os = "horizon")]
pub mod controllers;
/// The console's DNS servers, for SRV records.
#[cfg(target_os = "horizon")]
pub mod dns;

pub use compat::{
    ThreadCorePolicy, set_thread_core_policy, set_thread_logging, thread_core_policy,
    thread_logging,
};
#[cfg(target_os = "horizon")]
pub use boost::loading_world;
#[cfg(target_os = "horizon")]
pub use clocks::splitscreen_clocks;
#[cfg(target_os = "horizon")]
pub use keyboard::software_keyboard;
#[cfg(target_os = "horizon")]
pub use motion::{Motion, motion, stop_motion};
#[cfg(target_os = "horizon")]
pub use lifecycle::relaunch_on_exit;
pub use lifecycle::{
    flush_logs, init_horizon_services, process_applet_messages, register_release_hook,
    suspend_until_out_of_background,
};
#[cfg(target_os = "horizon")]
pub use logging::nro_path;
pub use logging::{flush_switch_logs, init_switch_logging, nro_dir};

#[cfg(target_os = "horizon")]
static MOUSE_CURSOR_ACTIVE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

#[cfg(target_os = "horizon")]
#[unsafe(no_mangle)]
pub extern "C" fn horizon_nx_set_mouse_cursor_active(active: bool) {
    MOUSE_CURSOR_ACTIVE.store(active, core::sync::atomic::Ordering::Relaxed);
}

/// Whether a USB mouse moved or clicked more recently than the touch screen (used to draw a
/// software menu pointer on Switch, which has no hardware OS cursor).
#[cfg(target_os = "horizon")]
pub fn mouse_cursor_active() -> bool {
    MOUSE_CURSOR_ACTIVE.load(core::sync::atomic::Ordering::Relaxed)
}

/// With `on`, runs the calling thread just ahead of the game's own threads (which start at
/// priority 0x3b), else back among them: for the time a thread only waits on the network (a
/// download), so it isn't left waiting for a core while world generation and meshing keep every
/// core busy (equal priorities on Horizon don't share a core by turns). Never for work that
/// keeps a core busy: that would hold frames back. Whether the kernel took it.
#[cfg(target_os = "horizon")]
pub fn favour_waiting_thread(on: bool) -> bool {
    unsafe extern "C" {
        fn svcSetThreadPriority(handle: u32, priority: u32) -> u32;
    }
    let priority = if on { 0x3a } else { 0x3b };
    // SAFETY: the calling thread's own pseudo-handle; no pointers.
    unsafe { svcSetThreadPriority(0xFFFF_8000, priority) == 0 }
}

/// One line on the malloc heap: what malloc has in use and has taken from the heap, and how
/// much of the heap nvmap objects hold. On the Switch that one heap holds everything: the
/// game's allocations and, with the `vulkan` feature, every GPU buffer and image. Walks the
/// heap with `svcQueryMemory`, so call it at load stages, not every frame.
#[cfg(target_os = "horizon")]
pub fn memory_report() -> String {
    match memory::nvmap_heap() {
        Some((blocks, bytes)) => format!(
            "{}; {:.1} MiB of the heap is mapped for the GPU (nvmap), in {blocks} blocks",
            memory::malloc_report(),
            bytes as f64 / (1024.0 * 1024.0),
        ),
        None => memory::malloc_report(),
    }
}

/// Sets up the Horizon services and logging. Call at the very start of `main`, before anything
/// allocates heavily or opens sockets. Does nothing on other targets.
pub fn init() {
    #[cfg(target_os = "horizon")]
    {
        lifecycle::init_horizon_services();
        lifecycle::configure_memory_line_from_env();
        init_switch_logging("horizon");
        // A homebrew loader leaves the working directory undefined, and the game resolves its
        // game home, options.txt, saves/ and world/ from it. Move into the NRO's own folder so
        // every file the user puts next to the NRO is found. No-op when the loader already
        // starts there (Ryujinx passes the NRO at the SD root, cwd = sdmc:/).
        if let Some(dir) = nro_dir() {
            match std::env::set_current_dir(&dir) {
                Ok(()) => crate::logging::write_log_line(format_args!(
                    "[Boot] working directory -> {} (the NRO's folder)",
                    dir.display()
                )),
                Err(e) => crate::logging::write_log_line(format_args!(
                    "[Boot] could not enter {}: {e}; files must go where the loader's cwd points",
                    dir.display()
                )),
            }
        }
        check_std_stat();
        // env.txt next to the NRO sets environment variables before anything reads them, so
        // perf knobs (BH_ROBUST_BUFFER_ACCESS, BEVY_HORIZON_CPU_PROFILE,
        // HORIZON_NX_SWAPCHAIN_IMAGES, BH_ALLOC_CACHE, BEVY_HORIZON_ZCULL_DIR, HORIZON_NX_CPU_BOOST,
        // HORIZON_NX_THREAD_CORES, HORIZON_NX_OPEN_STAMPS, HORIZON_NX_GPU_CLOCKS) are testable without
        // reflashing.
        // `KEY=VALUE` per line, `#` starts a comment, blank lines ignored. A variable the loader
        // already set wins; each applied line is logged.
        apply_env_file();
        // The game's home (options.txt, saves/, logs/) is the NRO's folder: the game reads
        // HORIZON_HOME, else a desktop data folder the Switch doesn't have.
        if std::env::var_os("HORIZON_HOME").is_none() {
            // SAFETY: edition 2024 makes set_var unsafe; no other thread is running yet.
            unsafe { std::env::set_var("HORIZON_HOME", ".") };
        }
        // std has no core count for Horizon; an application gets cores 0-2 (core 3 is the
        // system's). The game sizes its worker pools from HORIZON_WORKER_THREADS.
        if std::env::var_os("HORIZON_WORKER_THREADS").is_none() {
            // SAFETY: as above, no other thread is running yet.
            unsafe { std::env::set_var("HORIZON_WORKER_THREADS", "3") };
        }
        // NAK's plat and ofold passes off, unless env.txt says otherwise
        // (nak_safe_default).
        nak_safe_default();
        // Links on the SD card for the resource-pack and mod overlays (vlinks), unless
        // HORIZON_NX_LINKS=0. After env.txt, while no other thread runs and no folder is open.
        if std::env::var_os("HORIZON_NX_LINKS").is_none_or(|v| v != "0") {
            match vlinks::install() {
                Ok(()) => crate::logging::write_log_line(format_args!(
                    "[Boot] SD card links on (resource packs and mods)"
                )),
                Err(e) => crate::logging::write_log_line(format_args!(
                    "[Boot] SD card links NOT on: {e}; resource packs and mods won't load"
                )),
            }
        }
        // Cores 0-2 for new threads unless HORIZON_NX_THREAD_CORES says otherwise (compat.rs);
        // before the profiler, so its thread follows the choice too.
        compat::thread_cores_from_env();
        // Each new thread's [Thread] lines only with HORIZON_NX_THREAD_LOG=1 (compat.rs).
        compat::thread_logging_from_env();
        // Read-only opens skip libnx's timestamp query unless HORIZON_NX_OPEN_STAMPS=1 (compat.rs).
        compat::horizon_stubs::open_stamps_from_env();
        // Only with HORIZON_CPU_PROFILE set (profiler.rs).
        profiler::start_from_env();
        // BH_ALLOC_CACHE=0 turns the allocator's small-block cache off.
        #[cfg(feature = "top_down_alloc")]
        allocator::configure_from_env();
        // ZCULL direction for reversed depth, unless BEVY_HORIZON_ZCULL_DIR=less (nvk_tune.rs).
        #[cfg(feature = "vulkan")]
        nvk_tune::configure_from_env();
        // NVK's Switch WSI reads `bh_wsi_switch_image_count` (compat.rs) when it makes a
        // swapchain; set it from the env before any surface exists.
        #[cfg(feature = "vulkan")]
        compat::horizon_stubs::set_swapchain_images();
        // The clocks into the log, and HORIZON_NX_GPU_CLOCKS's faster ones for a diagnostic run
        // (clocks.rs), before the boost stands in for them.
        clocks::start_from_env();
        // The CPU boost for loading, ended once the player is in a world (boost.rs).
        boost::start_from_env();
    }
}

/// std reads `struct stat` through its own libc, which the build must point at the patched one
/// (devkitA64's newlib layout; `scripts/hx-build-client.sh` moves std's lock onto it). Built
/// against crates.io's libc (the 3DS's layout) instead, std misreads every `stat`: folders are
/// not directories and sizes are wrong, so the game can't open its logs, fonts or data (log15).
/// The working directory is a folder, so std must see one.
#[cfg(target_os = "horizon")]
fn check_std_stat() {
    if !std::fs::metadata(".").is_ok_and(|m| m.is_dir()) {
        crate::logging::write_log_line(format_args!(
            "[Boot] ERROR: std does not see the working directory as a folder: its libc doesn't match newlib's `struct stat` (std built against crates.io's libc?). File access will fail"
        ));
    }
}

/// Two NAK passes the public NVK build (mesa-switch with bevy-horizon's patches) turns on by
/// default in Switch builds, turned off, unless `NAK_DEBUG` is already set (env.txt).
///
/// With all seven default passes on, the GPU hung (`fifo idle timeout`) about ten seconds after a
/// world's terrain started drawing, on a console, every time. Bisected on a console
/// (2026-11-17, one pass at a time back on, shader cache off): xbar, sink, remat, xbrk and lsink
/// play; turning `plat` on hangs, and so does turning `ofold` on with `plat` off: each pass hangs
/// the GPU on its own. Release 0.1.0 was built with a private NVK whose NAK differs.
#[cfg(target_os = "horizon")]
pub const NAK_SAFE_DEBUG: &str = "-plat,-ofold";

#[cfg(target_os = "horizon")]
fn nak_safe_default() {
    // A build linked with JohnUzoka's mesa-switch (the NVK of release 0.1.0, whose NAK passes
    // played without hanging) keeps its driver's defaults: `HORIZON_NX_NAK_SAFE=0` at build time.
    if option_env!("HORIZON_NX_NAK_SAFE") == Some("0") {
        crate::logging::write_log_line(format_args!(
            "[Boot] NAK_DEBUG left to the driver (built with HORIZON_NX_NAK_SAFE=0)"
        ));
        return;
    }
    if std::env::var_os("NAK_DEBUG").is_some() {
        return;
    }
    // Edition 2024 makes set_var unsafe: no other thread is running yet (init is the first
    // thing main does), so nothing reads the environment meanwhile.
    unsafe { std::env::set_var("NAK_DEBUG", NAK_SAFE_DEBUG) };
    crate::logging::write_log_line(format_args!(
        "[Boot] NAK_DEBUG = {NAK_SAFE_DEBUG} (default: NAK's plat and ofold passes hung the GPU)"
    ));
}

/// Reads `env.txt` next to the NRO and applies each `KEY=VALUE` line to the environment,
/// unless the variable is already set (a loader's own value wins). Runs before the profiler,
/// allocator, NVK tuning and swapchain-image setup read the environment, so a perf knob can
/// be changed by editing a text file on the SD card instead of reflashing the NRO.
#[cfg(target_os = "horizon")]
fn apply_env_file() {
    let Some(dir) = nro_dir() else { return };
    let path = dir.join("env.txt");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let mut applied = 0;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() {
            continue;
        }
        if std::env::var_os(key).is_some() {
            crate::logging::write_log_line(format_args!(
                "[Boot] env.txt: {key} was already set by the loader; keeping the loader's value"
            ));
            continue;
        }
        // Edition 2024 makes set_var unsafe: no other thread is running yet (init is the
        // first thing main does), so the environment is not being read concurrently.
        unsafe { std::env::set_var(key, value) };
        applied += 1;
        crate::logging::write_log_line(format_args!("[Boot] env.txt: {key} = {value}"));
    }
    if applied > 0 {
        crate::logging::write_log_line(format_args!(
            "[Boot] env.txt next to the NRO supplied {applied} variable(s)"
        ));
    }
}

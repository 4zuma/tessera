//! Opt-in memory log for measuring: with `TESSERA_MEMLOG=<file>` set, a
//! thread appends one line every 2 seconds with this process's memory and
//! where it sits (heaps, other private memory, the file tree). When the
//! private regions of 1 MiB or more change by 4 MiB or more, it also lists
//! the regions that came and went (address, size, protection, allocation
//! base), to tell who allocated them. Without the variable nothing runs.
//! `local-bench\mem.ps1` sets it.

use std::io::Write;
use std::time::{Duration, Instant};

use tauri::Manager;

use crate::backend;

const EVERY: Duration = Duration::from_secs(2);

pub fn start(app: &tauri::AppHandle) {
    let Some(path) = std::env::var_os("TESSERA_MEMLOG") else {
        return;
    };
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("tessera-memlog".into())
        .spawn(move || {
            let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            else {
                return;
            };
            let _ = writeln!(
                f,
                "# MB: private_ws = Task Manager's Memory column; private = commit; \
                 heap_* = all heaps (HeapSummary); big/small = committed private regions \
                 of at least / under 1 MiB (VirtualQuery); tree_* = Tree::memory()"
            );
            let _ = writeln!(
                f,
                "secs private_ws private ws heaps heap_alloc heap_commit big small \
                 tree_nodes tree_names names_raw"
            );
            let t0 = Instant::now();
            let mut seen: Vec<Region> = Vec::new();
            loop {
                let p = sys::sample();
                let tree = app.state::<backend::Core>().tree_memory();
                let mb = |b: usize| b as f64 / 1048576.0;
                let (nodes, names, raw) =
                    tree.map_or((0, 0, 0), |m| (m.nodes, m.names, m.names_raw));
                let _ = writeln!(
                    f,
                    "{:.0} {:.1} {:.1} {:.1} {} {:.1} {:.1} {:.1} {:.1} {:.1} {:.1} {:.1}",
                    t0.elapsed().as_secs_f64(),
                    mb(p.private_ws),
                    mb(p.private),
                    mb(p.ws),
                    p.heaps,
                    mb(p.heap_alloc),
                    mb(p.heap_commit),
                    mb(p.big),
                    mb(p.small),
                    mb(nodes),
                    mb(names),
                    mb(raw),
                );
                let total = |v: &[Region]| v.iter().map(|r| r.size).sum::<usize>();
                if total(&p.regions).abs_diff(total(&seen)) >= 4 << 20 {
                    for r in p.regions.iter().filter(|r| !seen.contains(r)) {
                        let _ = writeln!(f, "#   + {r}");
                    }
                    for r in seen.iter().filter(|r| !p.regions.contains(r)) {
                        let _ = writeln!(f, "#   - {r}");
                    }
                    seen = p.regions;
                }
                let _ = f.flush();
                std::thread::sleep(EVERY);
            }
        });
}

/// A committed private region of at least 1 MiB.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Region {
    base: usize,
    size: usize,
    protect: u32,
    alloc_base: usize,
}

impl std::fmt::Display for Region {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:#014x} {:8.2} MB protect {:#x} allocation {:#014x}",
            self.base,
            self.size as f64 / 1048576.0,
            self.protect,
            self.alloc_base
        )
    }
}

#[derive(Default)]
struct Sample {
    private_ws: usize,
    private: usize,
    ws: usize,
    heaps: usize,
    heap_alloc: usize,
    heap_commit: usize,
    big: usize,
    small: usize,
    regions: Vec<Region>,
}

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;
    use std::mem::size_of;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Memory::{
        GetProcessHeaps, HEAP_SUMMARY, HeapSummary, MEM_COMMIT, MEM_PRIVATE,
        MEMORY_BASIC_INFORMATION, VirtualQuery,
    };
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX2,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;

    use super::{Region, Sample};

    pub(super) fn sample() -> Sample {
        let mut s = Sample::default();
        // SAFETY: plain queries about this process into locals sized for them.
        unsafe {
            let mut pmc = PROCESS_MEMORY_COUNTERS_EX2 {
                cb: size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32,
                ..Default::default()
            };
            if GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut pmc as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
                pmc.cb,
            )
            .is_ok()
            {
                s.private_ws = pmc.PrivateWorkingSetSize;
                s.private = pmc.PrivateUsage;
                s.ws = pmc.WorkingSetSize;
            }
            let mut heaps = vec![HANDLE::default(); 64];
            let n = GetProcessHeaps(&mut heaps) as usize;
            if n > heaps.len() {
                heaps.resize(n, HANDLE::default());
                GetProcessHeaps(&mut heaps);
            }
            for &h in heaps.iter().take(n) {
                let mut hs = HEAP_SUMMARY {
                    cb: size_of::<HEAP_SUMMARY>() as u32,
                    ..Default::default()
                };
                if HeapSummary(h, 0, &mut hs).as_bool() {
                    s.heaps += 1;
                    s.heap_alloc += hs.cbAllocated;
                    s.heap_commit += hs.cbCommitted;
                }
            }
            let mut addr = 0usize;
            loop {
                let mut mbi = MEMORY_BASIC_INFORMATION::default();
                let got = VirtualQuery(
                    Some(addr as *const c_void),
                    &mut mbi,
                    size_of::<MEMORY_BASIC_INFORMATION>(),
                );
                if got == 0 {
                    break;
                }
                if mbi.State == MEM_COMMIT && mbi.Type == MEM_PRIVATE {
                    if mbi.RegionSize >= 1 << 20 {
                        s.big += mbi.RegionSize;
                        s.regions.push(Region {
                            base: mbi.BaseAddress as usize,
                            size: mbi.RegionSize,
                            protect: mbi.Protect.0,
                            alloc_base: mbi.AllocationBase as usize,
                        });
                    } else {
                        s.small += mbi.RegionSize;
                    }
                }
                let next = (mbi.BaseAddress as usize).wrapping_add(mbi.RegionSize);
                if next <= addr {
                    break;
                }
                addr = next;
            }
        }
        s
    }
}

#[cfg(not(windows))]
mod sys {
    pub(super) fn sample() -> super::Sample {
        super::Sample::default()
    }
}

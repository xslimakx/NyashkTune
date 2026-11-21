//! Counts heap allocations while the engine processes a real voice (must be zero).
use nyashktune::engine::Engine;
use nyashktune::NyashkTuneParams;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;
static N: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        N.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        N.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static A: Counting = Counting;

fn main() {
    let raw = std::fs::read(std::env::args().nth(1).unwrap()).unwrap();
    let x: Vec<f32> = raw.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let pp = NyashkTuneParams::default();
    let mut e = Engine::new(48000.0, pp.engine_params().min_hz);
    let mut sink = 0.0;
    let before = N.load(Ordering::Relaxed);
    for ch in x.chunks(64) {
        let p = pp.engine_params();
        e.set_params(&p);
        for &s in ch {
            sink += e.process_frame([s, s], 2)[0];
        }
    }
    println!("allocations while processing: {}  [{sink:.1}]", N.load(Ordering::Relaxed) - before);
}

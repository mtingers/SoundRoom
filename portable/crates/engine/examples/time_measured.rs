//! Times modeled-hit synthesis: direct sound vs the full stereo hit.
use soundroom_engine::{drums::{Articulation, Kit, Voice}, measured_drums::{self, Shape}};
use std::{sync::atomic::AtomicBool, time::Instant};
fn main() {
    let c = AtomicBool::new(false);
    for voice in [Voice::Kick, Voice::Snare, Voice::Crash] {
        let t = Instant::now(); let d = measured_drums::direct_hit(voice, "center", 0.7, 1).unwrap(); let direct = t.elapsed();
        let t = Instant::now(); let _ = measured_drums::render(Kit::StudioModeled, voice, 0.7, 1, Articulation::Center, 0.5, Some(Shape { body: 0.5, ..Shape::default() }), &c).unwrap(); let first = t.elapsed();
        let t = Instant::now(); let _ = measured_drums::render(Kit::StudioModeled, voice, 0.6, 2, Articulation::Center, 0.5, Some(Shape { body: 0.5, ..Shape::default() }), &c).unwrap(); let warm = t.elapsed();
        println!("{voice:?}: direct {direct:?} ({:.1}s)  full first {first:?}  full warm-room {warm:?}", d.len() as f64 / 48000.);
    }
}

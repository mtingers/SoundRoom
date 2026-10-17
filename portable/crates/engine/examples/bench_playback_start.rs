//! Times what the server does to open a project and start playback at a
//! position: open, prepare the paged graph, seek (DSP replay from the
//! nearest checkpoint), store a checkpoint, page in the sources.
//!
//! bench_playback_start <project.soundroom> [seconds...]
use soundroom_engine::{
    project::Project,
    render::{CheckpointCache, PreparedGraph},
};
use std::{sync::atomic::AtomicBool, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let cancel = AtomicBool::new(false);
    let t = Instant::now();
    let project = Project::open(std::path::Path::new(&args[1]))?;
    println!(
        "open project            {:7.0} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    let t = Instant::now();
    let (graph, mut loader) =
        PreparedGraph::prepare_paged(&project, 32 * 1024 * 1024, &cancel, None)?;
    println!(
        "prepare graph           {:7.0} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    println!(
        "effect tail             {:7.1} s",
        graph.tail_frames() as f64 / 48000.
    );
    println!(
        "checkpoint size needed  {:7.1} MB (cache 32 MB)",
        graph.checkpoint_storage_budget() as f64 / 1e6
    );
    let mut cache = CheckpointCache::new(32 * 1024 * 1024);
    let positions: Vec<f64> = args[2..].iter().map(|s| s.parse().unwrap()).collect();
    for seconds in positions {
        let t = Instant::now();
        let mut fork = graph.fork();
        fork.seek((seconds * 48000.) as u64, &cache, &cancel)?;
        let seek = t.elapsed().as_secs_f64() * 1e3;
        let stored = cache.store(&fork).is_ok();
        let t2 = Instant::now();
        loader.prepare_graph(&fork, 32768, &cancel)?;
        println!("play at {seconds:6.1} s: seek {seek:6.0} ms, page-in {:5.0} ms, checkpoint stored {stored}", t2.elapsed().as_secs_f64() * 1e3);
    }
    Ok(())
}

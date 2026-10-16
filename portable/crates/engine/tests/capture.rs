//! All sources/timestamps are deterministic simulated data. These tests do not
//! enumerate, open or call an audio device, and do not qualify capture hardware.
use soundroom_engine::{
    capture::{CaptureTiming, CapturedBlock},
    capture_archive::{ArchiveManifest, ArchiveRead, CaptureArchive, CaptureArchiveReader},
    capture_clock::{
        CaptureAligner, ClockConfig, ClockObservation, OutputClockAnchor, PacketAlignment,
        SampleClock,
    },
    capture_resample::{CaptureResampler, ResampleConfig},
    capture_worker::{route_track, CaptureIngest, RecordingSourceWorker},
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
const TICKS: f64 = 1e9;
const HOST: u64 = 1_000_000_000_000_000_000;
fn clock(rate: u32) -> ClockConfig {
    ClockConfig {
        sample_rate: rate,
        host_ticks_per_second: TICKS,
        maximum_drift_ppm: 2000.,
    }
}
fn time(sample: f64, seconds: f64) -> CaptureTiming {
    CaptureTiming {
        sample_time: sample,
        host_time: HOST + (seconds * TICKS).round() as u64,
        flags: 3,
    }
}
fn anchor(frame: u64, seconds: f64) -> OutputClockAnchor {
    OutputClockAnchor {
        epoch: 1,
        playback_clock: 40000 + frame,
        frames: 512,
        timing: time(100000. + frame as f64, seconds),
    }
}
static ID: AtomicU64 = AtomicU64::new(0);
struct Temporary(PathBuf);
impl Temporary {
    fn new() -> Self {
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "soundroom-capture-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        soundroom_platform::PrivateDirectory::create_root(&root).unwrap();
        Self(root.join("data"))
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.0.parent().unwrap());
    }
}
fn manifest() -> ArchiveManifest {
    ArchiveManifest {
        version: 1,
        take_id: "11223344-5566-7788-9900-aabbccddeeff".into(),
        device_uid: "simulated-input-only".into(),
        channels: 2,
        sample_rate: 48000,
        maximum_frames: 512,
        host_ticks_per_second: TICKS,
        calibration_offset_frames: 0,
    }
}

#[test]
fn independent_input_and_output_clocks_align_drift_without_large_host_tick_roundoff() {
    let input_rate = 44100. * 1.0006;
    let output_rate = 48000. * 0.9996;
    let mut map = CaptureAligner::new(clock(44100), clock(48000), -23).unwrap();
    let mut last_output = None;
    let mut maximum_error = 0f64;
    for packet in 0..6000u64 {
        let frame = packet * 441;
        let seconds = frame as f64 / input_rate;
        let output_frame = ((seconds * output_rate / 512.).floor() as u64) * 512;
        if last_output != Some(output_frame) {
            map.observe_output(anchor(output_frame, output_frame as f64 / output_rate))
                .unwrap();
            last_output = Some(output_frame);
        }
        let jitter: f64 = if packet % 3 == 0 {
            0.000003
        } else if packet % 3 == 1 {
            -0.000003
        } else {
            0.
        };
        let block = CapturedBlock {
            source_frame: frame,
            frames: 441,
            timing: time(500000. + frame as f64, seconds + jitter.max(-seconds)),
        };
        let alignment = map.align_packet(block).unwrap();
        assert_eq!(alignment.queue_gap, 0);
        assert_eq!(alignment.device_gap, 0);
        if packet > 1000 {
            let expected = 40000. + seconds * output_rate - 23.;
            maximum_error = maximum_error.max((alignment.output_start - expected).abs());
            assert!(
                (alignment.input_frames_per_output_frame - input_rate / output_rate).abs() < 1e-6
            );
        }
    }
    assert!(
        maximum_error < 0.3,
        "fractional alignment error {maximum_error}"
    );
    let (input, output) = map.estimates();
    assert!((input.drift_ppm - 600.).abs() < 0.5);
    assert!((output.drift_ppm + 400.).abs() < 0.01);
    println!("60-second independent clocks: max alignment error {maximum_error:.8} output frames; input {:.6} ppm output {:.6} ppm",input.drift_ppm,output.drift_ppm);
}

#[test]
fn clocks_report_jitter_outliers_queue_gaps_hardware_gaps_and_reset_epochs() {
    let mut map = CaptureAligner::new(clock(48000), clock(48000), 0).unwrap();
    map.observe_output(anchor(0, 0.)).unwrap();
    let first = CapturedBlock {
        source_frame: 0,
        frames: 512,
        timing: time(0., 0.),
    };
    map.align_packet(first).unwrap();
    let missing = CapturedBlock {
        source_frame: 1024,
        frames: 512,
        timing: time(1152., 1152. / 48000.),
    };
    let result = map.align_packet(missing).unwrap();
    assert_eq!(result.queue_gap, 512);
    assert_eq!(result.device_gap, 128);
    let outlier = CapturedBlock {
        source_frame: 1536,
        frames: 512,
        timing: time(1664., 1664. / 48000. + 0.01),
    };
    assert_eq!(
        map.align_packet(outlier).unwrap().clock_observation,
        ClockObservation::TimestampOutlier
    );
    assert!(map
        .align_packet(CapturedBlock {
            source_frame: 2048,
            frames: 512,
            timing: time(0., 0.06)
        })
        .is_err());
    let mut changed = anchor(512, 512. / 48000.);
    changed.epoch = 2;
    assert!(map.observe_output(changed).is_err());
    let mut missing = SampleClock::new(clock(48000)).unwrap();
    assert!(missing.observe(CaptureTiming::default()).is_err());
    assert!(CaptureAligner::new(clock(48000), clock(48000), i64::MIN).is_err());
    let mut wrong_rate = SampleClock::new(clock(48000)).unwrap();
    assert!((0..100).any(|i| wrong_rate
        .observe(time(i as f64 * 512., i as f64 * 512. / 48000. / 1.01))
        .is_err()));
    let mut stale = CaptureAligner::new(clock(48000), clock(48000), 0).unwrap();
    stale.observe_output(anchor(0, 0.)).unwrap();
    assert!(stale
        .align_packet(CapturedBlock {
            source_frame: 0,
            frames: 512,
            timing: time(0., 1.)
        })
        .is_err());
}
fn resample(
    rate: u32,
    length: usize,
    frequency: f64,
    input_block: usize,
    output_block: usize,
    fraction: f64,
) -> Vec<f32> {
    let mut converter = CaptureResampler::new(
        ResampleConfig {
            channels: 1,
            input_rate: rate,
            output_rate: 48000,
            maximum_drift_ppm: 2000.,
        },
        0,
        fraction,
    )
    .unwrap();
    let mut output = Vec::new();
    let mut scratch = vec![0.; output_block];
    for start in (0..length).step_by(input_block) {
        let end = (start + input_block).min(length);
        let samples: Vec<f32> = (start..end)
            .map(|i| (std::f64::consts::TAU * frequency * i as f64 / rate as f64).sin() as f32)
            .collect();
        converter.push(start as u64, &[&samples]).unwrap();
        loop {
            let block = converter.drain(&mut [&mut scratch]).unwrap();
            if block.frames == 0 {
                break;
            }
            output.extend_from_slice(&scratch[..block.frames]);
        }
    }
    converter.finish();
    loop {
        let block = converter.drain(&mut [&mut scratch]).unwrap();
        if block.frames == 0 {
            break;
        }
        output.extend_from_slice(&scratch[..block.frames]);
    }
    output
}
#[test]
fn sinc_conversion_has_fractional_alignment_passband_accuracy_alias_rejection_and_packet_invariance(
) {
    let mut max_error = 0f64;
    for rate in [8000, 44100, 48000, 96000, 192000, 384000] {
        let a = resample(rate, rate as usize / 4, 997., 257, 113, 0.375);
        let b = resample(rate, rate as usize / 4, 997., 4096, 8192, 0.375);
        assert_eq!(a, b, "packet partition changed output at {rate}");
        assert!((a.len() as isize - 12000).abs() <= 1);
        for (i, &sample) in a.iter().enumerate().skip(700).take(a.len() - 1400) {
            let expected = (std::f64::consts::TAU * 997. * (i as f64 + 1. - 0.375) / 48000.).sin();
            max_error = max_error.max((sample as f64 - expected).abs());
        }
    }
    assert!(max_error < 3e-5, "analytic passband error {max_error}");
    let alias = resample(96000, 48000, 30000., 512, 8192, 0.);
    let peak = alias[300..alias.len() - 300]
        .iter()
        .fold(0f64, |v, &x| v.max((x as f64).abs()));
    let rejection = 20. * peak.log10();
    assert!(rejection < -85., "alias rejection {rejection} dB");
    println!(
        "sinc analytic max passband error {max_error:.9}; 96k→48k 30kHz stopband {rejection:.3} dB"
    );
}
#[test]
fn sinc_stream_bounds_gaps_nonfinite_and_filter_tail_do_not_invent_raw_audio() {
    let mut convert = CaptureResampler::new(
        ResampleConfig {
            channels: 2,
            input_rate: 48000,
            output_rate: 48000,
            maximum_drift_ppm: 2000.,
        },
        0,
        0.,
    )
    .unwrap();
    let mut input = [0.2; 512];
    input[100] = f32::from_bits(0x7fc01234);
    convert.push(0, &[&input, &input]).unwrap();
    assert_eq!(convert.stats().invalid_samples, 2);
    assert!(convert.push(1024, &[&input, &input]).is_err());
    let mut l = [0.; 8192];
    let mut r = l;
    let first = convert.drain(&mut [&mut l, &mut r]).unwrap();
    assert_eq!(first.frames, 416);
    assert!(l[..first.frames].iter().all(|s| s.is_finite()));
    convert.finish();
    let end = convert.drain(&mut [&mut l, &mut r]).unwrap();
    assert_eq!(end.frames, 96);
    assert_eq!(convert.drain(&mut [&mut l, &mut r]).unwrap().frames, 0);
    assert_eq!(convert.stats().output_frames, 512);
    assert!(convert.push(512, &[&input, &input]).is_err());
    assert!(convert.set_ratio(f64::NAN).is_err());
    assert!(convert
        .follow_packet(
            512,
            PacketAlignment {
                output_start: 512.,
                input_frames_per_output_frame: 1.,
                queue_gap: 1,
                device_gap: 0,
                clock_observation: ClockObservation::Accepted
            }
        )
        .is_err());
    assert!(CaptureResampler::new(
        ResampleConfig {
            channels: 65,
            input_rate: 48000,
            output_rate: 48000,
            maximum_drift_ppm: 2000.
        },
        0,
        0.
    )
    .is_err());
}
#[test]
fn raw_archive_roundtrips_exact_float_bits_timestamps_and_explicit_gaps() {
    let temp = Temporary::new();
    let mut raw = CaptureArchive::create(&temp.0, manifest()).unwrap();
    assert!(CaptureArchive::create(&temp.0, manifest()).is_err());
    let l = [f32::from_bits(0x7fc12345), -0., f32::INFINITY, 0.25];
    let r = [-0.4; 4];
    for source_frame in [0, 4, 12] {
        let block = CapturedBlock {
            source_frame,
            frames: 4,
            timing: time(300. + source_frame as f64, source_frame as f64 / 48000.),
        };
        raw.append(block, &[&l, &r]).unwrap();
    }
    let stats = raw.finish().unwrap();
    assert_eq!(stats.retained_frames, 12);
    assert_eq!(stats.queue_gap_frames, 4);
    assert_eq!(stats.invalid_samples, 6);
    let mut scan = CaptureArchiveReader::open(&temp.0).unwrap();
    let mut a = [0.; 512];
    let mut b = a;
    for source_frame in [0, 4, 12] {
        let ArchiveRead::Packet(packet) = scan.read_planar(&mut [&mut a, &mut b]).unwrap() else {
            panic!()
        };
        assert_eq!(packet.source_frame, source_frame);
        assert_eq!(
            packet.timing,
            time(300. + source_frame as f64, source_frame as f64 / 48000.)
        );
        assert_eq!(
            a[..4].iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            l.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
        assert_eq!(&b[..4], &r);
    }
    assert_eq!(
        scan.read_planar(&mut [&mut a, &mut b]).unwrap(),
        ArchiveRead::End
    );
    assert!(fs::read_to_string(temp.0.join("events.jsonl"))
        .unwrap()
        .contains("queueGap"));
}
#[test]
fn raw_archive_recovers_complete_prefix_of_every_partial_header_or_pcm_write_and_detects_corruption(
) {
    let source = Temporary::new();
    let mut raw = CaptureArchive::create(&source.0, manifest()).unwrap();
    let block = CapturedBlock {
        source_frame: 0,
        frames: 4,
        timing: time(0., 0.),
    };
    raw.append(block, &[&[0.25; 4], &[-0.25; 4]]).unwrap();
    raw.append(
        CapturedBlock {
            source_frame: 4,
            ..block
        },
        &[&[0.5; 4], &[-0.5; 4]],
    )
    .unwrap();
    raw.finish().unwrap();
    let bytes = fs::read(source.0.join("native.packets")).unwrap();
    let copy = Temporary::new();
    fs::create_dir(&copy.0).unwrap();
    fs::copy(source.0.join("manifest.json"), copy.0.join("manifest.json")).unwrap();
    let first = 32 + 48 + 32;
    let mut a = [0.; 512];
    let mut b = a;
    for cut in first..bytes.len() {
        fs::write(copy.0.join("native.packets"), &bytes[..cut]).unwrap();
        let mut scan = CaptureArchiveReader::open(&copy.0).unwrap();
        assert!(matches!(
            scan.read_planar(&mut [&mut a, &mut b]).unwrap(),
            ArchiveRead::Packet(_)
        ));
        let tail = scan.read_planar(&mut [&mut a, &mut b]).unwrap();
        if cut == first {
            assert_eq!(tail, ArchiveRead::End);
        } else {
            assert_eq!(
                tail,
                ArchiveRead::TruncatedTail {
                    bytes: (cut - first) as u64
                }
            );
        }
        assert_eq!(
            scan.read_planar(&mut [&mut a, &mut b]).unwrap(),
            ArchiveRead::End
        );
    }
    let mut corrupt = bytes.clone();
    corrupt[80] ^= 1;
    fs::write(copy.0.join("native.packets"), corrupt).unwrap();
    let mut scan = CaptureArchiveReader::open(&copy.0).unwrap();
    assert!(scan.read_planar(&mut [&mut a, &mut b]).is_err());
    assert_eq!(
        scan.read_planar(&mut [&mut a, &mut b]).unwrap(),
        ArchiveRead::End
    );
}
#[test]
fn source_worker_preserves_packets_after_alignment_stops_and_routes_independent_tracks() {
    let temp = Temporary::new();
    let raw = CaptureArchive::create(&temp.0, manifest()).unwrap();
    let mut worker = RecordingSourceWorker::new(
        raw,
        CaptureAligner::new(clock(48000), clock(48000), 0).unwrap(),
        2000.,
    )
    .unwrap();
    worker.observe_output(anchor(0, 0.)).unwrap();
    let l = [0.2; 512];
    let r = [-0.3; 512];
    assert!(matches!(
        worker
            .ingest(
                CapturedBlock {
                    source_frame: 0,
                    frames: 512,
                    timing: time(0., 0.)
                },
                &[&l, &r]
            )
            .unwrap(),
        CaptureIngest::Aligned(_)
    ));
    let mut outl = [0.; 512];
    let mut outr = outl;
    assert_eq!(
        worker
            .drain(&mut [&mut outl, &mut outr])
            .unwrap()
            .unwrap()
            .frames,
        416
    );
    assert_eq!(
        worker
            .ingest(
                CapturedBlock {
                    source_frame: 1024,
                    frames: 512,
                    timing: time(1024., 1024. / 48000.)
                },
                &[&l, &r]
            )
            .unwrap(),
        CaptureIngest::RetainedOnly
    );
    assert!(worker.suspended());
    assert_eq!(
        worker
            .ingest(
                CapturedBlock {
                    source_frame: 1536,
                    frames: 512,
                    timing: time(1536., 1536. / 48000.)
                },
                &[&l, &r]
            )
            .unwrap(),
        CaptureIngest::RetainedOnly
    );
    worker.finish_conversion();
    assert_eq!(
        worker
            .drain(&mut [&mut outl, &mut outr])
            .unwrap()
            .unwrap()
            .frames,
        96
    );
    let stats = worker.finish_archive().unwrap();
    assert_eq!(stats.retained_frames, 1536);
    assert_eq!(stats.queue_gap_frames, 512);
    route_track(&[&l, &r], &[1], &mut outl, &mut outr).unwrap();
    assert_eq!(outl, r);
    assert_eq!(outr, r);
    route_track(&[&l, &r], &[1, 0], &mut outl, &mut outr).unwrap();
    assert_eq!(outl, r);
    assert_eq!(outr, l);
    let before = outl;
    assert!(route_track(&[&l, &r], &[2], &mut outl, &mut outr).is_err());
    assert_eq!(outl, before);
}

#[test]
fn adaptive_conversion_follows_independent_clock_drift_without_discontinuous_phase_steps() {
    let input_rate = 44100. * 1.0006;
    let output_rate = 48000. * 0.9996;
    let mut map = CaptureAligner::new(clock(44100), clock(48000), 0).unwrap();
    let mut convert = CaptureResampler::new(
        ResampleConfig {
            channels: 1,
            input_rate: 44100,
            output_rate: 48000,
            maximum_drift_ppm: 2000.,
        },
        0,
        40000.,
    )
    .unwrap();
    let mut last_output = None;
    let mut scratch = [0.; 8192];
    let mut max_error = 0f64;
    for packet in 0..2400u64 {
        let frame = packet * 441;
        let seconds = frame as f64 / input_rate;
        let out = ((seconds * output_rate / 512.).floor() as u64) * 512;
        if last_output != Some(out) {
            map.observe_output(anchor(out, out as f64 / output_rate))
                .unwrap();
            last_output = Some(out);
        }
        let packet = CapturedBlock {
            source_frame: frame,
            frames: 441,
            timing: time(500000. + frame as f64, seconds),
        };
        let alignment = map.align_packet(packet).unwrap();
        convert.follow_packet(frame, alignment).unwrap();
        let samples: Vec<f32> = (frame..frame + 441)
            .map(|i| (std::f64::consts::TAU * 997. * i as f64 / input_rate).sin() as f32)
            .collect();
        convert.push(frame, &[&samples]).unwrap();
        loop {
            let block = convert.drain(&mut [&mut scratch]).unwrap();
            if block.frames == 0 {
                break;
            }
            if seconds > 15. {
                for (i, &value) in scratch[..block.frames].iter().enumerate() {
                    let output = (block.output_frame - 40000) as f64 + i as f64;
                    let expected = (std::f64::consts::TAU * 997. * output / output_rate).sin();
                    max_error = max_error.max((value as f64 - expected).abs());
                }
            }
        }
    }
    assert!(
        max_error < 0.00005,
        "adaptive bandlimited reference error {max_error}"
    );
    println!(
        "24-second adaptive independent-clock analytic reference: max settled error {max_error:.9}"
    );
}

#[test]
fn sinc_upper_passband_and_high_rate_stopbands_hold_across_all_admitted_ratios() {
    let mut max_error = 0f64;
    let mut worst_rejection = f64::NEG_INFINITY;
    for rate in [8000, 44100, 48000, 96000, 192000, 384000] {
        let frequency = 20000f64.min(rate as f64 * 0.4);
        let output = resample(rate, rate as usize / 8, frequency, 8192, 8192, 0.625);
        for (i, &sample) in output
            .iter()
            .enumerate()
            .skip(800)
            .take(output.len() - 1600)
        {
            let expected =
                (std::f64::consts::TAU * frequency * (i as f64 + 1. - 0.625) / 48000.).sin();
            max_error = max_error.max((sample as f64 - expected).abs());
        }
        if rate >= 96000 {
            let rejected = resample(rate, rate as usize / 8, 30000., 8192, 8192, 0.625);
            let peak = rejected[800..rejected.len() - 800]
                .iter()
                .fold(0f64, |v, &x| v.max((x as f64).abs()));
            worst_rejection = worst_rejection.max(20. * peak.log10());
        }
    }
    assert!(max_error < 0.00001, "upper passband error {max_error}");
    assert!(
        worst_rejection < -100.,
        "worst 30kHz stopband {worst_rejection}"
    );
    println!("all-rate upper passband max analytic error {max_error:.9}; worst 96/192/384k 30kHz stopband {worst_rejection:.3} dB");
}

#[test]
fn event_recovery_preserves_clock_anchors_and_reports_final_capture_overrun_and_partial_journal() {
    use soundroom_engine::{
        capture::CaptureStats,
        capture_archive::{ArchiveEventRead, CaptureEvent, CaptureEventReader},
    };
    let temp = Temporary::new();
    let raw = CaptureArchive::create(&temp.0, manifest()).unwrap();
    let mut worker = RecordingSourceWorker::new(
        raw,
        CaptureAligner::new(clock(48000), clock(48000), 0).unwrap(),
        2000.,
    )
    .unwrap();
    worker.observe_output(anchor(0, 0.)).unwrap();
    worker.observe_output(anchor(0, 0.)).unwrap();
    worker
        .ingest(
            CapturedBlock {
                source_frame: 0,
                frames: 512,
                timing: time(0., 0.),
            },
            &[&[0.2; 512], &[-0.2; 512]],
        )
        .unwrap();
    let mut health = CaptureStats {
        pcm_bytes: 1024,
        received_frames: 1024,
        dropped_frames: 512,
        invalid_samples: 0,
        device_faults: 0,
        last_device_error: 0,
    };
    worker.observe_capture_health(health).unwrap();
    worker.observe_capture_health(health).unwrap();
    assert!(worker.suspended());
    health.device_faults = 1;
    health.last_device_error = -50;
    worker.observe_capture_health(health).unwrap();
    let stats = worker.finish_archive().unwrap();
    assert_eq!(stats.retained_frames, 512);
    assert_eq!(stats.events, 3);
    let mut reader = CaptureEventReader::open(&temp.0).unwrap();
    let ArchiveEventRead::Event(CaptureEvent::OutputAnchor {
        sample_time_bits,
        host_time,
        ..
    }) = reader.read_event().unwrap()
    else {
        panic!()
    };
    assert_eq!(f64::from_bits(sample_time_bits), 100000.);
    assert_eq!(host_time, HOST);
    assert_eq!(
        reader.read_event().unwrap(),
        ArchiveEventRead::Event(CaptureEvent::QueueOverrun {
            received_frames: 1024,
            dropped_frames: 512
        })
    );
    assert_eq!(
        reader.read_event().unwrap(),
        ArchiveEventRead::Event(CaptureEvent::DeviceFault {
            source_frame: 1024,
            status: -50
        })
    );
    assert_eq!(reader.read_event().unwrap(), ArchiveEventRead::End);
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(temp.0.join("events.jsonl"))
        .unwrap()
        .write_all(b"{\"type\":")
        .unwrap();
    let mut reader = CaptureEventReader::open(&temp.0).unwrap();
    for _ in 0..3 {
        reader.read_event().unwrap();
    }
    assert_eq!(
        reader.read_event().unwrap(),
        ArchiveEventRead::TruncatedTail { bytes: 8 }
    );
    fs::write(temp.0.join("events.jsonl"), vec![b'x'; 16385]).unwrap();
    assert!(CaptureEventReader::open(&temp.0)
        .unwrap()
        .read_event()
        .is_err());
}

fn recovery_plan(incomplete: bool) -> soundroom_engine::capture_recovery::RecoveryPlan {
    use soundroom_engine::capture_recovery::{RecoveryPlan, RecoveryRoute};
    RecoveryPlan {
        version: 1,
        name: "Simulated take".into(),
        routes: vec![
            RecoveryRoute {
                track_id: "00112233-4455-6677-8899-AABBCCDDEEFF".into(),
                channels: vec![0, 1],
            },
            RecoveryRoute {
                track_id: "FFEEDDCC-BBAA-9988-7766-554433221100".into(),
                channels: vec![1],
            },
        ],
        range: None,
        loop_range: None,
        maximum_drift_ppm: 2000.,
        allow_incomplete: incomplete,
    }
}
fn recover(
    raw: &Temporary,
    target: &Temporary,
    plan: &soundroom_engine::capture_recovery::RecoveryPlan,
) -> soundroom_engine::capture_recovery::RecoveryReport {
    use soundroom_engine::capture_recovery::{recover_take, RecoveryOutcome};
    match recover_take(
        &raw.0,
        &target.0,
        plan,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap()
    {
        RecoveryOutcome::Committed(r) => r,
        RecoveryOutcome::AlreadyPresent(_) => panic!("new target unexpectedly exists"),
    }
}
fn archive_fixture(gap: bool, reset: bool) -> Temporary {
    use soundroom_engine::capture_archive::CaptureEvent;
    let raw = Temporary::new();
    let mut archive = CaptureArchive::create(&raw.0, manifest()).unwrap();
    for packet in 0..8u64 {
        let frame = packet * 512 + if gap && packet >= 4 { 512 } else { 0 };
        archive
            .event(CaptureEvent::OutputAnchor {
                source_frame: Some(if packet == 0 {
                    0
                } else {
                    (packet - 1) * 512 + 512 + if gap && packet >= 5 { 512 } else { 0 }
                }),
                epoch: 1,
                playback_clock: 40000 + frame,
                frames: 512,
                sample_time_bits: (100000. + frame as f64).to_bits(),
                host_time: time(0., frame as f64 / 48000.).host_time,
                flags: 3,
            })
            .unwrap();
        let audio0: Vec<f32> = (frame..frame + 512)
            .map(|i| (std::f64::consts::TAU * 997. * i as f64 / 48000.).sin() as f32 * 0.3)
            .collect();
        let audio1: Vec<f32> = audio0.iter().map(|&v| v * -0.7).collect();
        let block = CapturedBlock {
            source_frame: frame,
            frames: 512,
            timing: time(
                if reset && packet >= 4 {
                    (packet - 4) * 512
                } else {
                    frame
                } as f64,
                frame as f64 / 48000.,
            ),
        };
        archive.append(block, &[&audio0, &audio1]).unwrap();
    }
    archive.finish().unwrap();
    raw
}
#[test]
fn recovery_streams_stereo_and_mono_take_assets_with_stable_receipts_and_rejects_changed_retries() {
    use soundroom_engine::{
        capture_recovery::{recover_take, RecoveryOutcome},
        wave::WaveReader,
    };
    let raw = archive_fixture(false, false);
    let target = Temporary::new();
    let plan = recovery_plan(false);
    let report = recover(&raw, &target, &plan);
    assert!(!report.requires_review);
    assert_eq!(report.retained_native_frames, 4096);
    assert_eq!(report.assets.len(), 2);
    assert_eq!(report.takes.len(), 2);
    for asset in &report.assets {
        assert_eq!(asset.frames, 4096);
        assert_eq!(asset.timeline_start, 40000);
        let mut reader = WaveReader::open(&target.0.join(&asset.relative_path)).unwrap();
        let mut left = [0.; 4096];
        let mut right = left;
        reader.read_stereo(0, &mut left, &mut right).unwrap();
        for i in 200..3896 {
            let original = (std::f64::consts::TAU * 997. * i as f64 / 48000.).sin() as f32 * 0.3;
            if asset.track_id == plan.routes[0].track_id {
                assert!((left[i] - original).abs() < 2e-6);
                assert!((right[i] + original * 0.7).abs() < 2e-6);
            } else {
                assert_eq!(left[i], right[i]);
                assert!((left[i] + original * 0.7).abs() < 2e-6);
            }
        }
    }
    let result = recover_take(
        &raw.0,
        &target.0,
        &plan,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(result, RecoveryOutcome::AlreadyPresent(report.clone()));
    let target2 = Temporary::new();
    let second = recover(&raw, &target2, &plan);
    assert_eq!(second, report);
    for (a, b) in report.assets.iter().zip(&second.assets) {
        assert_eq!(
            fs::read(target.0.join(&a.relative_path)).unwrap(),
            fs::read(target2.0.join(&b.relative_path)).unwrap()
        );
    }
    let mut changed = plan.clone();
    changed.routes[0].channels.reverse();
    assert!(recover_take(
        &raw.0,
        &target.0,
        &changed,
        &std::sync::atomic::AtomicBool::new(false)
    )
    .is_err());
    let asset = &report.assets[0];
    let path = target.0.join(&asset.relative_path);
    let mut bytes = fs::read(&path).unwrap();
    bytes[100] ^= 1;
    fs::write(path, bytes).unwrap();
    assert!(recover_take(
        &raw.0,
        &target.0,
        &plan,
        &std::sync::atomic::AtomicBool::new(false)
    )
    .is_err());
}
#[test]
fn recovery_keeps_gap_clips_and_clock_reset_segments_explicit_and_incomplete_policy_transactional()
{
    use soundroom_engine::capture_recovery::recover_take;
    for reset in [false, true] {
        let raw = archive_fixture(!reset, reset);
        let target = Temporary::new();
        let plan = recovery_plan(true);
        let report = recover(&raw, &target, &plan);
        assert!(report.requires_review);
        assert_eq!(report.assets.len(), 4);
        assert_eq!(report.takes.len(), 2);
        for take in &report.takes {
            assert_eq!(take.take.clips.len(), 2);
            assert_eq!(take.take.clips[0].length, 2048);
            assert_eq!(take.take.clips[1].length, 2048);
            assert_eq!(
                take.take.clips[1].start,
                40000 + 2048 + if reset { 0 } else { 512 }
            );
        }
        assert!(report.issues.iter().any(|v| v.kind
            == if reset {
                "inputClockSegment"
            } else {
                "queueGap"
            }));
        let denied = Temporary::new();
        let plan = recovery_plan(false);
        assert!(recover_take(
            &raw.0,
            &denied.0,
            &plan,
            &std::sync::atomic::AtomicBool::new(false)
        )
        .is_err());
        assert!(!denied.0.exists());
    }
}
#[test]
fn recovery_splits_loop_passes_and_punch_trims_streamed_assets_without_losing_raw_preroll() {
    use soundroom_engine::capture_recovery::RecoveryRange;
    let raw = archive_fixture(false, false);
    let target = Temporary::new();
    let mut plan = recovery_plan(false);
    plan.loop_range = Some(RecoveryRange {
        start: 40000,
        end: 41024,
    });
    plan.range = Some(RecoveryRange {
        start: 40256,
        end: 40768,
    });
    let report = recover(&raw, &target, &plan);
    assert!(!report.requires_review);
    assert_eq!(report.assets.len(), 8);
    assert_eq!(report.takes.len(), 8);
    assert_eq!(report.trimmed_output_frames, 2048);
    for asset in &report.assets {
        assert_eq!(asset.timeline_start, 40256);
        assert_eq!(asset.frames, 512);
        assert!((1..=4).contains(&asset.loop_pass));
    }
    let ids: std::collections::BTreeSet<_> = report.assets.iter().map(|a| &a.id).collect();
    assert_eq!(ids.len(), 8);
    assert_eq!(report.retained_native_frames, 4096);
}
#[test]
fn interrupted_recovery_preserves_complete_native_prefix_and_cancellation_leaves_no_target() {
    use soundroom_engine::capture_recovery::recover_take;
    let raw = archive_fixture(false, false);
    let packets = raw.0.join("native.packets");
    let size = fs::metadata(&packets).unwrap().len();
    fs::OpenOptions::new()
        .write(true)
        .open(&packets)
        .unwrap()
        .set_len(size - 200)
        .unwrap();
    let target = Temporary::new();
    let report = recover(&raw, &target, &recovery_plan(true));
    assert!(report.requires_review);
    assert_eq!(report.retained_native_frames, 3584);
    assert!(report.issues.iter().any(|v| v.kind == "packetTail"));
    assert_eq!(report.assets[0].frames, 3584);
    let cancelled = Temporary::new();
    assert!(recover_take(
        &raw.0,
        &cancelled.0,
        &recovery_plan(true),
        &std::sync::atomic::AtomicBool::new(true)
    )
    .is_err());
    assert!(!cancelled.0.exists());
    assert!(raw.0.exists());
    let changed = Temporary::new();
    let mut plan = recovery_plan(true);
    plan.routes[1].track_id = plan.routes[0].track_id.to_lowercase();
    assert!(recover_take(
        &raw.0,
        &changed.0,
        &plan,
        &std::sync::atomic::AtomicBool::new(false)
    )
    .is_err());
    assert!(!changed.0.exists());
}

#[test]
fn recovery_replays_recording_workers_actual_anchor_sequence_and_resampling_sample_exactly() {
    use soundroom_engine::wave::WaveReader;
    let raw = Temporary::new();
    let mut metadata = manifest();
    metadata.sample_rate = 44100;
    metadata.calibration_offset_frames = -23;
    let archive = CaptureArchive::create(&raw.0, metadata).unwrap();
    let mut worker = RecordingSourceWorker::new(
        archive,
        CaptureAligner::new(clock(44100), clock(48000), -23).unwrap(),
        2000.,
    )
    .unwrap();
    let actual_input = 44100. * 1.0006;
    let actual_output = 48000. * 0.9996;
    let mut live_l = Vec::new();
    let mut live_r = Vec::new();
    let mut l = [0.; 512];
    let mut r = l;
    for packet in 0..100u64 {
        let frame = packet * 441;
        let seconds = frame as f64 / actual_input;
        let out = ((seconds * actual_output / 512.).floor() as u64) * 512;
        worker
            .observe_output(anchor(out, out as f64 / actual_output))
            .unwrap();
        let left: Vec<f32> = (frame..frame + 441)
            .map(|i| (std::f64::consts::TAU * 997. * i as f64 / actual_input).sin() as f32 * 0.3)
            .collect();
        let right: Vec<f32> = left.iter().map(|&v| v * 0.6).collect();
        worker
            .ingest(
                CapturedBlock {
                    source_frame: frame,
                    frames: 441,
                    timing: time(500000. + frame as f64, seconds),
                },
                &[&left, &right],
            )
            .unwrap();
        loop {
            let block = worker.drain(&mut [&mut l, &mut r]).unwrap().unwrap();
            if block.frames == 0 {
                break;
            }
            live_l.extend_from_slice(&l[..block.frames]);
            live_r.extend_from_slice(&r[..block.frames]);
        }
    }
    worker.finish_conversion();
    loop {
        let block = worker.drain(&mut [&mut l, &mut r]).unwrap().unwrap();
        if block.frames == 0 {
            break;
        }
        live_l.extend_from_slice(&l[..block.frames]);
        live_r.extend_from_slice(&r[..block.frames]);
    }
    worker.finish_archive().unwrap();
    let target = Temporary::new();
    let report = recover(&raw, &target, &recovery_plan(false));
    assert!(!report.requires_review);
    let asset = report
        .assets
        .iter()
        .find(|v| v.track_id == recovery_plan(false).routes[0].track_id)
        .unwrap();
    assert_eq!(asset.frames, live_l.len() as u64);
    assert_eq!(asset.timeline_start, 39977);
    let mut wave = WaveReader::open(&target.0.join(&asset.relative_path)).unwrap();
    for frame in (0..live_l.len()).step_by(512) {
        let count = (live_l.len() - frame).min(512);
        wave.read_stereo(frame as u64, &mut l[..count], &mut r[..count])
            .unwrap();
        assert_eq!(&l[..count], &live_l[frame..frame + count]);
        assert_eq!(&r[..count], &live_r[frame..frame + count]);
    }
}

#[test]
fn recovery_corrupt_receipts_changed_raw_and_concurrent_retry_identity_are_rejected_or_deduplicated(
) {
    use soundroom_engine::capture_recovery::{recover_take, RecoveryOutcome};
    let raw = archive_fixture(false, false);
    let target = Temporary::new();
    let plan = recovery_plan(false);
    let left_raw = raw.0.clone();
    let left_target = target.0.clone();
    let left_plan = plan.clone();
    let first = std::thread::spawn(move || {
        recover_take(
            &left_raw,
            &left_target,
            &left_plan,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap()
    });
    let second = recover_take(
        &raw.0,
        &target.0,
        &plan,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    let first = first.join().unwrap();
    let reports = [&first, &second];
    assert_eq!(
        reports
            .iter()
            .filter(|v| matches!(v, RecoveryOutcome::Committed(_)))
            .count(),
        1
    );
    let unwrap = |result| match result {
        RecoveryOutcome::Committed(r) | RecoveryOutcome::AlreadyPresent(r) => r,
    };
    assert_eq!(unwrap(first), unwrap(second));
    let mut receipt = fs::read(target.0.join("recovery.json")).unwrap();
    receipt[30] ^= 1;
    fs::write(target.0.join("recovery.json"), receipt).unwrap();
    assert!(recover_take(
        &raw.0,
        &target.0,
        &plan,
        &std::sync::atomic::AtomicBool::new(false)
    )
    .is_err());
    let other = Temporary::new();
    recover(&raw, &other, &plan);
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(raw.0.join("events.jsonl"))
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    assert!(recover_take(
        &raw.0,
        &other.0,
        &plan,
        &std::sync::atomic::AtomicBool::new(false)
    )
    .is_err());
}

#[test]
fn fixed_recovery_fixtures_keep_asset_take_and_receipt_fingerprints_stable() {
    use soundroom_engine::capture_recovery::{
        recover_take, RecoveryOutcome, RecoveryPlan, RecoveryReport,
    };
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/capture");
    for name in ["continuous", "gap", "loop-punch"] {
        let fixture = root.join(name);
        let plan: RecoveryPlan =
            serde_json::from_slice(&fs::read(fixture.join("plan.json")).unwrap()).unwrap();
        let expected: RecoveryReport =
            serde_json::from_slice(&fs::read(fixture.join("expected.json")).unwrap()).unwrap();
        let target = Temporary::new();
        let raw_copy = Temporary::new();
        let private = soundroom_platform::PrivateDirectory::create_root(&raw_copy.0).unwrap();
        for file in [
            "manifest.json",
            "native.packets",
            "events.jsonl",
            "completion.json",
        ] {
            use std::io::Write;
            let mut output = private.create_file(file).unwrap();
            output
                .write_all(&fs::read(fixture.join("raw").join(file)).unwrap())
                .unwrap();
            output.sync_all().unwrap();
        }
        let RecoveryOutcome::Committed(actual) = recover_take(
            &raw_copy.0,
            &target.0,
            &plan,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(actual, expected, "{name} artifact metadata/digests changed");
        assert_eq!(
            recover_take(
                &raw_copy.0,
                &target.0,
                &plan,
                &std::sync::atomic::AtomicBool::new(false)
            )
            .unwrap(),
            RecoveryOutcome::AlreadyPresent(expected)
        );
    }
}

#[test]
fn take_import_preparation_is_scoped_preserves_active_content_and_deduplicates_or_conflicts() {
    use soundroom_engine::{
        capture_recovery::{prepare_take_import, RecoveryAcceptance},
        model::{Session, Track},
    };
    let raw = archive_fixture(false, false);
    let target = Temporary::new();
    let plan = recovery_plan(false);
    let report = recover(&raw, &target, &plan);
    let mut session = Session {
        tracks: plan
            .routes
            .iter()
            .map(|r| Track {
                id: r.track_id.clone(),
                name: "Original track".into(),
                gain: 0.7,
                pan: -0.3,
                armed: true,
                ..Track::default()
            })
            .collect(),
        ..Session::default()
    };
    let unrelated = Track {
        id: "FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF".into(),
        name: "Untouched".into(),
        ..Track::default()
    };
    session.tracks.push(unrelated.clone());
    let prepared =
        prepare_take_import(&report, &session, RecoveryAcceptance::CompleteOnly).unwrap();
    assert_eq!(prepared.tracks.len(), 2);
    assert_eq!(prepared.asset_ids.len(), 2);
    assert!(prepared.already_present.is_empty());
    assert!(session.tracks.iter().all(|t| t.recording_takes.is_empty()));
    for changed in prepared.tracks {
        let original = session
            .tracks
            .iter_mut()
            .find(|t| t.id == changed.id)
            .unwrap();
        assert_eq!(changed.gain, original.gain);
        assert_eq!(changed.pan, original.pan);
        assert_eq!(changed.clips, original.clips);
        assert_eq!(changed.name, original.name);
        *original = changed;
    }
    assert_eq!(session.tracks[2], unrelated);
    let retry = prepare_take_import(&report, &session, RecoveryAcceptance::CompleteOnly).unwrap();
    assert!(retry.tracks.is_empty());
    assert!(retry.asset_ids.is_empty());
    assert_eq!(retry.already_present.len(), 2);
    session.tracks[0].recording_takes[0].clips[0].length -= 1;
    assert!(prepare_take_import(&report, &session, RecoveryAcceptance::CompleteOnly).is_err());
    session.tracks.clear();
    assert!(prepare_take_import(&report, &session, RecoveryAcceptance::CompleteOnly).is_err());
    let raw = archive_fixture(true, false);
    let target = Temporary::new();
    let report = recover(&raw, &target, &recovery_plan(true));
    let session = Session {
        tracks: plan
            .routes
            .iter()
            .map(|r| Track {
                id: r.track_id.clone(),
                ..Track::default()
            })
            .collect(),
        ..Session::default()
    };
    assert!(prepare_take_import(&report, &session, RecoveryAcceptance::CompleteOnly).is_err());
    assert!(prepare_take_import(&report, &session, RecoveryAcceptance::ReviewedIncomplete).is_ok());
}

#[test]
fn recovery_crc_failure_and_legacy_unsequenced_timestamps_keep_only_verified_audio() {
    let raw = archive_fixture(false, false);
    let file = raw.0.join("native.packets");
    let mut bytes = fs::read(&file).unwrap();
    bytes[32 + 48 + 4096 + 48 + 100] ^= 1;
    fs::write(file, bytes).unwrap();
    let target = Temporary::new();
    let report = recover(&raw, &target, &recovery_plan(true));
    assert!(report.requires_review);
    assert_eq!(report.retained_native_frames, 512);
    assert_eq!(report.assets[0].frames, 512);
    assert!(report.issues.iter().any(|v| v.kind == "corruptPacket"));
    let raw = archive_fixture(false, false);
    let eventfile = raw.0.join("events.jsonl");
    let text = fs::read_to_string(&eventfile).unwrap();
    let rewritten = text
        .lines()
        .map(|line| {
            let mut value: serde_json::Value = serde_json::from_str(line).unwrap();
            if value["type"] == "outputAnchor" {
                value["source_frame"] = serde_json::Value::Null;
            }
            serde_json::to_string(&value).unwrap()
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(eventfile, rewritten).unwrap();
    let target = Temporary::new();
    let report = recover(&raw, &target, &recovery_plan(true));
    assert!(report.requires_review);
    assert_eq!(report.unaligned_native_frames, 4096);
    assert!(report.assets.is_empty());
    assert!(report
        .issues
        .iter()
        .any(|v| v.kind == "legacyUnsequencedClock"));
}

#[test]
#[cfg(unix)]
fn recovery_admission_rejects_raw_aliases_hardlinks_and_broad_files_without_creating_takes() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let raw = archive_fixture(false, false);
    let target = Temporary::new();
    let file = raw.0.join("manifest.json");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(soundroom_engine::capture_recovery::recover_take(
        &raw.0,
        &target.0,
        &recovery_plan(false),
        &AtomicBool::new(false)
    )
    .is_err());
    assert!(!target.0.exists());
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    let alias = raw.0.join("alias");
    fs::hard_link(&file, &alias).unwrap();
    assert!(soundroom_engine::capture_recovery::recover_take(
        &raw.0,
        &target.0,
        &recovery_plan(false),
        &AtomicBool::new(false)
    )
    .is_err());
    fs::remove_file(alias).unwrap();
    fs::rename(&file, raw.0.join("original-manifest")).unwrap();
    symlink("original-manifest", &file).unwrap();
    assert!(soundroom_engine::capture_recovery::recover_take(
        &raw.0,
        &target.0,
        &recovery_plan(false),
        &AtomicBool::new(false)
    )
    .is_err());
    assert!(!target.0.exists());
    assert_eq!(fs::read_dir(target.0.parent().unwrap()).unwrap().count(), 0);
}

#[test]
#[cfg(unix)]
fn recovery_capability_keeps_original_raw_after_path_replacement_and_validates_retry_custody() {
    use soundroom_engine::capture_recovery::{recover_take_in, RecoveryOutcome};
    use std::os::unix::fs::{symlink, PermissionsExt};
    let raw = archive_fixture(false, false);
    let target = Temporary::new();
    let source = soundroom_platform::PrivateDirectory::open_root(&raw.0).unwrap();
    let parent =
        soundroom_platform::PrivateDirectory::open_root(target.0.parent().unwrap()).unwrap();
    let moved = raw.0.with_file_name("original-raw");
    fs::rename(&raw.0, &moved).unwrap();
    let foreign = Temporary::new();
    soundroom_platform::PrivateDirectory::create_root(&foreign.0).unwrap();
    fs::write(foreign.0.join("sentinel"), b"foreign").unwrap();
    symlink(&foreign.0, &raw.0).unwrap();
    let RecoveryOutcome::Committed(report) = recover_take_in(
        &source,
        &parent,
        "data",
        &recovery_plan(false),
        &AtomicBool::new(false),
    )
    .unwrap() else {
        panic!()
    };
    assert_eq!(report.retained_native_frames, 4096);
    assert_eq!(fs::read(foreign.0.join("sentinel")).unwrap(), b"foreign");
    assert!(!foreign.0.join("native.packets").exists());
    let receipt = target.0.join("recovery.json");
    fs::set_permissions(&receipt, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(recover_take_in(
        &source,
        &parent,
        "data",
        &recovery_plan(false),
        &AtomicBool::new(false)
    )
    .is_err());
    fs::set_permissions(receipt, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        recover_take_in(
            &source,
            &parent,
            "data",
            &recovery_plan(false),
            &AtomicBool::new(false)
        )
        .unwrap(),
        RecoveryOutcome::AlreadyPresent(_)
    ));
}

#[test]
fn live_sealed_archive_matches_bound_source_and_preserves_final_loss_fault_and_nan_evidence() {
    use soundroom_engine::{
        capture::{capture_channel, CaptureStatus},
        capture_archive::{ArchiveEventRead, CaptureEvent, CaptureEventReader},
    };
    let temp = Temporary::new();
    let metadata = manifest();
    let (mut writer, mut reader) = capture_channel(metadata.capture_config(2)).unwrap();
    let archive = CaptureArchive::create(&temp.0, metadata).unwrap();
    let mut worker = RecordingSourceWorker::with_capture(
        archive,
        CaptureAligner::new(clock(48000), clock(48000), 0).unwrap(),
        2000.,
        &reader.control(),
    )
    .unwrap();
    let mut native_left = [0.2; 64];
    native_left[0] = f32::from_bits(0x7fc01234);
    let native_right = [0.3; 64];
    assert_eq!(
        writer.push(&[&native_left, &native_right], CaptureTiming::default()),
        CaptureStatus::Captured
    );
    assert_eq!(
        writer.push(&[&[0.4; 64], &[0.5; 64]], CaptureTiming::default()),
        CaptureStatus::Captured
    );
    assert_eq!(
        writer.push(&[&[0.6; 64], &[0.7; 64]], CaptureTiming::default()),
        CaptureStatus::Overflow
    );
    writer.device_fault(-10851);
    reader.control().seal();
    assert!(reader.seal_receipt().is_none());
    let mut left = [0.; 512];
    let mut right = left;
    for packet in 0..2 {
        let block = reader
            .read_planar(&mut [&mut left, &mut right])
            .unwrap()
            .unwrap();
        let retained = worker.ingest(block, &[&left[..block.frames], &right[..block.frames]]);
        if packet == 0 {
            assert!(retained.is_err());
        } else {
            assert_eq!(retained.unwrap(), CaptureIngest::RetainedOnly);
        }
    }
    let receipt = reader.seal_receipt().unwrap();
    let stats = worker.finish_archive_sealed(receipt).unwrap();
    assert_eq!(stats.retained_frames, 128);
    assert_eq!(stats.invalid_samples, 1);
    let mut native = CaptureArchiveReader::open(&temp.0).unwrap();
    let ArchiveRead::Packet(block) = native.read_planar(&mut [&mut left, &mut right]).unwrap()
    else {
        panic!("missing immutable raw packet")
    };
    assert_eq!(block.frames, 64);
    assert_eq!(left[0].to_bits(), 0x7fc01234);
    let mut journal = CaptureEventReader::open(&temp.0).unwrap();
    let mut events = vec![];
    loop {
        match journal.read_event().unwrap() {
            ArchiveEventRead::Event(event) => events.push(event),
            ArchiveEventRead::End => break,
            other => panic!("{other:?}"),
        }
    }
    assert!(events.contains(&CaptureEvent::QueueOverrun {
        received_frames: 192,
        dropped_frames: 64
    }));
    assert!(events.contains(&CaptureEvent::DeviceFault {
        source_frame: 192,
        status: -10851
    }));
    assert!(temp.0.join("completion.json").metadata().unwrap().len() > 0);
}

#[test]
fn live_archive_refuses_foreign_seal_receipts_and_unarchived_accepted_pcm() {
    use soundroom_engine::capture::capture_channel;
    for foreign in [false, true] {
        let temp = Temporary::new();
        let metadata = manifest();
        let (mut writer, mut reader) = capture_channel(metadata.capture_config(2)).unwrap();
        let archive = CaptureArchive::create(&temp.0, metadata.clone()).unwrap();
        let mut worker = RecordingSourceWorker::with_capture(
            archive,
            CaptureAligner::new(clock(48000), clock(48000), 0).unwrap(),
            2000.,
            &reader.control(),
        )
        .unwrap();
        writer.push(&[&[0.2; 64], &[0.3; 64]], CaptureTiming::default());
        reader.control().seal();
        let mut left = [0.; 512];
        let mut right = left;
        let block = reader
            .read_planar(&mut [&mut left, &mut right])
            .unwrap()
            .unwrap();
        let receipt = if foreign {
            let _ = worker.ingest(block, &[&left[..block.frames], &right[..block.frames]]);
            let (mut other_writer, mut other_reader) =
                capture_channel(metadata.capture_config(2)).unwrap();
            other_writer.push(&[&[0.2; 64], &[0.3; 64]], CaptureTiming::default());
            other_reader.control().seal();
            other_reader
                .read_planar(&mut [&mut left, &mut right])
                .unwrap();
            other_reader.seal_receipt().unwrap()
        } else {
            reader.seal_receipt().unwrap()
        };
        assert!(worker.finish_archive_sealed(receipt).is_err());
        assert_eq!(
            temp.0.join("completion.json").metadata().unwrap().len(),
            0,
            "mismatched source/prefix cannot become a completed raw archive"
        );
    }
}

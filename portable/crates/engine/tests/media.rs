//! Native Apple AudioFile and explicit specification fixtures. No devices.
use soundroom_engine::{
    aiff::{AiffExport, AiffReader, AiffWriter},
    media::{import_audio, AudioExport, AudioReader, AudioWriter},
    wave::WaveReader,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
static ID: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "soundroom-media-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/aiff")
}
#[test]
fn native_aiff_aifc_pcm_float_endian_and_fractional_rate_references_decode_all_channels() {
    let refs: serde_json::Value =
        serde_json::from_slice(&fs::read(root().join("native-reference.json")).unwrap()).unwrap();
    let mut native_created = 0;
    let mut native_read = 0;
    for reference in refs.as_array().unwrap() {
        let name = reference["name"].as_str().unwrap();
        let path = root().join(format!("{name}.aiff"));
        let mut audio = AudioReader::open(&path).unwrap();
        assert!((audio.info.sample_rate - reference["sampleRate"].as_f64().unwrap()).abs() < 1e-8);
        assert_eq!(
            audio.info.channels,
            reference["channels"].as_u64().unwrap() as u16
        );
        assert_eq!(audio.info.frames, 8);
        assert_eq!(audio.info.bits, reference["bits"].as_u64().unwrap() as u16);
        let expected = reference["samples"].as_array().unwrap();
        let mut channels = vec![vec![0.; 8]; expected.len()];
        let mut buffers: Vec<&mut [f32]> = channels.iter_mut().map(|v| v.as_mut_slice()).collect();
        audio.read_planar(0, &mut buffers).unwrap();
        for (actual, expected) in channels.iter().zip(expected) {
            let expected: Vec<f32> = expected
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap() as f32)
                .collect();
            assert_eq!(actual, &expected, "native/spec {name}");
        }
        let mut left = [0.; 4];
        let mut right = left;
        audio.read_stereo(2, &mut left, &mut right).unwrap();
        assert_eq!(&left, &channels[0][2..6]);
        assert_eq!(
            &right,
            &channels[if channels.len() > 1 { 1 } else { 0 }][2..6]
        );
        assert!(audio.read_stereo(8, &mut left, &mut right).is_err());
        assert!(audio.scratch_bytes() <= 4 * 1024 * 1024);
        native_created += usize::from(reference["nativeCreated"] == true);
        native_read += usize::from(reference["nativeReadVerified"] == true);
    }
    assert_eq!(native_created, 5);
    assert_eq!(native_read, 7);
}
#[test]
fn streamed_aiff_exports_preserve_float_headroom_and_correct_signed_integer_limits() {
    let tmp = Temp::new();
    let left = [-2., -1., -0.5, 0., 0.125, 1., 2., f32::NAN];
    let right = [-0.; 8];
    for format in [AiffExport::Pcm16, AiffExport::Pcm24, AiffExport::Float32] {
        let path = tmp.0.join(format!("{format:?}.aiff"));
        let mut writer = AiffWriter::create(&path, 8, 48000, format).unwrap();
        writer.write(&left[..3], &right[..3]).unwrap();
        writer.write(&left[3..], &right[3..]).unwrap();
        writer.finish().unwrap();
        let mut reader = AiffReader::open(&path).unwrap();
        assert_eq!(reader.info.frames, 8);
        assert_eq!(reader.info.sample_rate, 48000.);
        let mut a = [0.; 8];
        let mut b = a;
        reader.read_stereo(0, &mut a, &mut b).unwrap();
        match format {
            AiffExport::Float32 => {
                assert_eq!(&a[..7], &left[..7]);
                assert_eq!(a[7], 0.);
                assert!(b.iter().all(|v| v.to_bits() == (-0f32).to_bits()));
            }
            _ => {
                let scale = if format == AiffExport::Pcm16 {
                    32768.
                } else {
                    8388608.
                };
                for i in 0..7 {
                    let expected = (left[i] as f64 * scale).round().clamp(-scale, scale - 1.)
                        as f32
                        / scale as f32;
                    assert_eq!(a[i], expected);
                }
                assert_eq!(a[7], 0.);
            }
        }
        assert!(AiffWriter::create(&path, 8, 48000, format).is_err());
    }
    assert!(AiffWriter::create(
        &tmp.0.join("large"),
        u32::MAX as u64,
        48000,
        AiffExport::Float32
    )
    .is_err());
    let incomplete =
        AiffWriter::create(&tmp.0.join("incomplete"), 5, 48000, AiffExport::Pcm24).unwrap();
    assert!(incomplete.finish().is_err());
}
fn chunk(bytes: &mut Vec<u8>, name: &[u8; 4], body: &[u8]) {
    bytes.extend_from_slice(name);
    bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
    bytes.extend_from_slice(body);
    if body.len() % 2 != 0 {
        bytes.push(0);
    }
}
fn rate(value: u32) -> [u8; 10] {
    let exponent = 31 - value.leading_zeros();
    let mut b = [0; 10];
    b[..2].copy_from_slice(&(16383 + exponent as u16).to_be_bytes());
    b[2..].copy_from_slice(&(u64::from(value) << (63 - exponent)).to_be_bytes());
    b
}
fn fixture(
    bits: u16,
    channels: u16,
    frames: u32,
    data: &[u8],
    offset: u32,
    sound_first: bool,
) -> Vec<u8> {
    let mut common = Vec::new();
    common.extend_from_slice(&channels.to_be_bytes());
    common.extend_from_slice(&frames.to_be_bytes());
    common.extend_from_slice(&bits.to_be_bytes());
    common.extend_from_slice(&rate(48000));
    let mut sound = Vec::new();
    sound.extend_from_slice(&offset.to_be_bytes());
    sound.extend_from_slice(&16u32.to_be_bytes());
    sound.resize(8 + offset as usize, 0);
    sound.extend_from_slice(data);
    let mut bytes = b"FORM\0\0\0\0AIFF".to_vec();
    chunk(&mut bytes, b"NAME", b"odd");
    if sound_first {
        chunk(&mut bytes, b"SSND", &sound);
        chunk(&mut bytes, b"COMM", &common);
    } else {
        chunk(&mut bytes, b"COMM", &common);
        chunk(&mut bytes, b"SSND", &sound);
    }
    let size = bytes.len() as u32 - 8;
    bytes[4..8].copy_from_slice(&size.to_be_bytes());
    bytes
}
#[test]
fn arbitrary_chunk_order_offsets_odd_padding_and_left_aligned_signed_depths_are_bounded() {
    let tmp = Temp::new();
    for bits in 1..=32u16 {
        let size = bits.div_ceil(8) as usize;
        let mut data = Vec::new();
        for word in [i32::MIN, 0, i32::MAX] {
            let packed = word.to_be_bytes();
            data.extend_from_slice(&packed[..size]);
        }
        let path = tmp.0.join(format!("{bits}.aiff"));
        fs::write(&path, fixture(bits, 1, 3, &data, 7, bits % 2 == 0)).unwrap();
        let mut reader = AiffReader::open(&path).unwrap();
        let mut a = [0.; 3];
        let mut b = a;
        reader.read_stereo(0, &mut a, &mut b).unwrap();
        assert_eq!(a, b);
        assert_eq!(a[0], -1.);
        assert_eq!(a[1], 0.);
        let maximum = 1. - 2f64.powi(1 - i32::from(bits));
        assert_eq!(a[2], maximum as f32);
    }
    let empty = tmp.0.join("empty");
    let mut bytes = b"FORM\0\0\0\0AIFF".to_vec();
    let mut common = Vec::new();
    common.extend_from_slice(&1u16.to_be_bytes());
    common.extend_from_slice(&0u32.to_be_bytes());
    common.extend_from_slice(&16u16.to_be_bytes());
    common.extend_from_slice(&rate(48000));
    chunk(&mut bytes, b"COMM", &common);
    let size = bytes.len() as u32 - 8;
    bytes[4..8].copy_from_slice(&size.to_be_bytes());
    fs::write(&empty, bytes).unwrap();
    assert_eq!(AiffReader::open(&empty).unwrap().info.frames, 0);
}
#[test]
fn malformed_headers_truncation_duplicate_chunks_and_compression_fail_before_decoding() {
    let tmp = Temp::new();
    let good = fixture(16, 2, 2, &[0; 8], 0, false);
    let path = tmp.0.join("bad");
    for cut in 0..good.len() {
        fs::write(&path, &good[..cut]).unwrap();
        assert!(AiffReader::open(&path).is_err(), "truncation {cut}");
    }
    let common = good.windows(4).position(|v| v == b"COMM").unwrap();
    let sound = good.windows(4).position(|v| v == b"SSND").unwrap();
    let mut bads = Vec::new();
    for (at, data) in [
        (common + 8, 0u16.to_be_bytes().to_vec()),
        (common + 8, 65u16.to_be_bytes().to_vec()),
        (common + 14, 0u16.to_be_bytes().to_vec()),
        (common + 16, [0xff; 10].to_vec()),
        (common + 10, 3u32.to_be_bytes().to_vec()),
        (sound + 8, 99u32.to_be_bytes().to_vec()),
    ] {
        let mut b = good.clone();
        b[at..at + data.len()].copy_from_slice(&data);
        bads.push(b);
    }
    let mut duplicate = good.clone();
    duplicate.extend_from_slice(&good[common..sound]);
    let size = duplicate.len() as u32 - 8;
    duplicate[4..8].copy_from_slice(&size.to_be_bytes());
    bads.push(duplicate);
    let float = fs::read(root().join("float32.aiff")).unwrap();
    let code = float.windows(4).position(|v| v == b"fl32").unwrap();
    let mut unsupported = float.clone();
    unsupported[code..code + 4].copy_from_slice(b"ima4");
    bads.push(unsupported);
    let mut version = float;
    version[20..24].copy_from_slice(&0u32.to_be_bytes());
    bads.push(version);
    for bytes in bads {
        fs::write(&path, bytes).unwrap();
        assert!(AiffReader::open(&path).is_err());
    }
}
#[test]
fn import_routes_normalize_native_and_fractional_rates_without_overwriting_or_changing_source() {
    let tmp = Temp::new();
    let source = root().join("pcm32.aiff");
    let original = fs::read(&source).unwrap();
    let target = tmp.0.join("normalized.wav");
    let result = import_audio(&source, &target, &[3], &AtomicBool::new(false)).unwrap();
    assert_eq!(result.source.channels, 4);
    assert_eq!(result.frames, 4);
    let mut wave = WaveReader::open(&target).unwrap();
    let mut l = [0.; 4];
    let mut r = l;
    wave.read_stereo(0, &mut l, &mut r).unwrap();
    assert_eq!(l, r);
    assert_eq!(fs::read(&source).unwrap(), original);
    assert!(import_audio(&source, &target, &[0, 1], &AtomicBool::new(false)).is_err());
    assert!(target.exists());
    let legacy = root().join("fractional.aiff");
    let target = tmp.0.join("fractional.wav");
    let imported = import_audio(&legacy, &target, &[0], &AtomicBool::new(false)).unwrap();
    assert!((imported.source.sample_rate - 22254.54).abs() < 1e-8);
    assert_eq!(imported.frames, 18);
    let cancelled = tmp.0.join("cancelled.wav");
    assert!(import_audio(&source, &cancelled, &[0], &AtomicBool::new(true)).is_err());
    assert!(!cancelled.exists());
    let bad = tmp.0.join("badroute.wav");
    assert!(import_audio(&source, &bad, &[4], &AtomicBool::new(false)).is_err());
    assert!(!bad.exists());
    let export = tmp.0.join("generic.aiff");
    let mut output = AudioWriter::create(&export, 4, 48000, AudioExport::AiffFloat32).unwrap();
    output.write(&l, &r).unwrap();
    output.finish().unwrap();
    assert_eq!(AudioReader::open(&export).unwrap().info.frames, 4);
}

fn exact_rate(value: f64) -> [u8; 10] {
    let word = value.to_bits();
    let exponent = ((word >> 52) & 0x7ff) as i32 - 1023;
    let mantissa = ((word & 0x000f_ffff_ffff_ffff) | (1 << 52)) << 11;
    let mut bytes = [0; 10];
    bytes[..2].copy_from_slice(&((exponent + 16383) as u16).to_be_bytes());
    bytes[2..].copy_from_slice(&mantissa.to_be_bytes());
    bytes
}
#[test]
fn fractional_legacy_rate_import_has_analytic_accuracy_and_carried_phase_over_many_packets() {
    let tmp = Temp::new();
    let source = tmp.0.join("fractional-float.aiff");
    let rate = 22254.54f64;
    let frames = 22000u32;
    let mut bytes = b"FORM\0\0\0\0AIFC".to_vec();
    chunk(&mut bytes, b"FVER", &0xa2805140u32.to_be_bytes());
    let mut common = Vec::new();
    common.extend_from_slice(&1u16.to_be_bytes());
    common.extend_from_slice(&frames.to_be_bytes());
    common.extend_from_slice(&32u16.to_be_bytes());
    common.extend_from_slice(&exact_rate(rate));
    common.extend_from_slice(b"fl32\0\0");
    chunk(&mut bytes, b"COMM", &common);
    let mut data = vec![0; 8];
    for frame in 0..frames {
        let sample = (std::f64::consts::TAU * 997. * frame as f64 / rate).sin() as f32;
        data.extend_from_slice(&sample.to_be_bytes());
    }
    chunk(&mut bytes, b"SSND", &data);
    let size = bytes.len() as u32 - 8;
    bytes[4..8].copy_from_slice(&size.to_be_bytes());
    fs::write(&source, bytes).unwrap();
    let target = tmp.0.join("normalized.wav");
    let imported = import_audio(&source, &target, &[0], &AtomicBool::new(false)).unwrap();
    assert_eq!(
        imported.frames,
        (frames as f64 / rate * 48000.).ceil() as u64
    );
    let mut audio = WaveReader::open(&target).unwrap();
    let mut l = [0.; 8192];
    let mut r = l;
    let mut max_error = 0f64;
    for frame in (700..imported.frames - 700).step_by(8192) {
        let count = (imported.frames - 700 - frame).min(8192) as usize;
        audio
            .read_stereo(frame, &mut l[..count], &mut r[..count])
            .unwrap();
        assert_eq!(&l[..count], &r[..count]);
        for (i, &sample) in l[..count].iter().enumerate() {
            let expected =
                (std::f64::consts::TAU * 997. * (frame + i as u64) as f64 / 48000.).sin();
            max_error = max_error.max((sample as f64 - expected).abs());
        }
    }
    assert!(
        max_error < 0.000005,
        "fractional rate analytic error {max_error}"
    );
    println!(
        "fractional22254.54Hz→48kHz max analytic error {max_error:.9}, {} frames",
        imported.frames
    );
}
#[test]
fn sparse_large_aiff_reads_stay_bounded_and_float64_wave_is_finite() {
    use std::io::{Seek, SeekFrom, Write};
    let tmp = Temp::new();
    let source = tmp.0.join("sparse.aiff");
    let frames = 200_000_000u32;
    let mut header = fixture(32, 2, frames, &[], 0, false);
    let sound = header.windows(4).position(|v| v == b"SSND").unwrap();
    let payload = u64::from(frames) * 8;
    let data_offset = sound as u64 + 16;
    let length = data_offset + payload;
    header[4..8].copy_from_slice(&((length - 8) as u32).to_be_bytes());
    header[sound + 4..sound + 8].copy_from_slice(&((payload + 8) as u32).to_be_bytes());
    fs::write(&source, &header).unwrap();
    let mut file = fs::OpenOptions::new().write(true).open(&source).unwrap();
    file.set_len(length).unwrap();
    file.seek(SeekFrom::Start(data_offset + u64::from(frames - 1) * 8))
        .unwrap();
    file.write_all(&0x40000000i32.to_be_bytes()).unwrap();
    file.write_all(&(-0x40000000i32).to_be_bytes()).unwrap();
    drop(file);
    let mut audio = AudioReader::open(&source).unwrap();
    assert_eq!(audio.scratch_bytes(), 65536);
    let mut l = [0.];
    let mut r = l;
    audio
        .read_stereo(u64::from(frames - 1), &mut l, &mut r)
        .unwrap();
    assert_eq!(l, [0.5]);
    assert_eq!(r, [-0.5]);
    assert_eq!(fs::metadata(&source).unwrap().len(), length);
    let wave = tmp.0.join("float64.wav");
    let values = [0.25f64, -0.75, 3.5, f64::NAN, f64::INFINITY, 1e200];
    let mut bytes = b"RIFF".to_vec();
    bytes.extend_from_slice(&(36u32 + values.len() as u32 * 8).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&3u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&48000u32.to_le_bytes());
    bytes.extend_from_slice(&384000u32.to_le_bytes());
    bytes.extend_from_slice(&8u16.to_le_bytes());
    bytes.extend_from_slice(&64u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(values.len() as u32 * 8).to_le_bytes());
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    fs::write(&wave, bytes).unwrap();
    let mut audio = AudioReader::open(&wave).unwrap();
    let mut l = [0.; 6];
    let mut r = l;
    audio.read_stereo(0, &mut l, &mut r).unwrap();
    assert_eq!(l, [0.25, -0.75, 3.5, 0., 0., 0.]);
    assert_eq!(r, l);
}

#[test]
fn little_endian_signed_layouts_and_float64_aifc_sanitation_remain_explicit() {
    let tmp = Temp::new();
    let path = tmp.0.join("aifc");
    for bits in 1..=32u16 {
        let size = bits.div_ceil(8) as usize;
        let mut bytes = b"FORM\0\0\0\0AIFC".to_vec();
        chunk(&mut bytes, b"FVER", &0xa2805140u32.to_be_bytes());
        let mut common = Vec::new();
        common.extend_from_slice(&1u16.to_be_bytes());
        common.extend_from_slice(&3u32.to_be_bytes());
        common.extend_from_slice(&bits.to_be_bytes());
        common.extend_from_slice(&rate(48000));
        common.extend_from_slice(b"sowt\0\0");
        chunk(&mut bytes, b"COMM", &common);
        let mut sound = vec![0; 8];
        for word in [i32::MIN, 0, i32::MAX] {
            sound.extend(word.to_be_bytes()[..size].iter().rev());
        }
        chunk(&mut bytes, b"SSND", &sound);
        let length = bytes.len() as u32 - 8;
        bytes[4..8].copy_from_slice(&length.to_be_bytes());
        fs::write(&path, bytes).unwrap();
        let mut reader = AudioReader::open(&path).unwrap();
        let mut l = [0.; 3];
        let mut r = l;
        reader.read_stereo(0, &mut l, &mut r).unwrap();
        assert_eq!(l, [-1., 0., (1. - 2f64.powi(1 - i32::from(bits))) as f32]);
        assert_eq!(l, r);
    }
    let mut bytes = fs::read(root().join("float64.aiff")).unwrap();
    let sound = bytes.windows(4).position(|v| v == b"SSND").unwrap();
    let offset = u32::from_be_bytes(bytes[sound + 8..sound + 12].try_into().unwrap()) as usize;
    let start = sound + 16 + offset;
    for (frame, value) in [3.5f64, f64::NAN, f64::INFINITY, 1e200]
        .into_iter()
        .enumerate()
    {
        bytes[start + frame * 16..start + frame * 16 + 8].copy_from_slice(&value.to_be_bytes());
    }
    fs::write(&path, bytes).unwrap();
    let mut reader = AudioReader::open(&path).unwrap();
    let mut l = [0.; 4];
    let mut r = l;
    reader.read_stereo(0, &mut l, &mut r).unwrap();
    assert_eq!(l, [3.5, 0., 0., 0.]);
    let mut oversized = [0.; 8193];
    let mut right = oversized;
    assert!(reader.read_stereo(0, &mut oversized, &mut right).is_err());
    assert!(reader.read_planar(0, &mut [&mut l]).is_err());
    assert!(reader.read_stereo(u64::MAX, &mut l, &mut r).is_err());
}

#[test]
fn owned_media_handles_import_and_export_without_reopening_paths_or_truncating_artifacts() {
    use soundroom_engine::media::import_audio_from_files;
    use std::io::{Seek, SeekFrom, Write};
    let temp = Temp::new();
    let private_path = temp.0.canonicalize().unwrap().join("private");
    let private = soundroom_platform::PrivateDirectory::create_root(&private_path).unwrap();
    let mut native = private.create_file("source.aiff").unwrap();
    native
        .write_all(&fs::read(root().join("float32.aiff")).unwrap())
        .unwrap();
    native.sync_all().unwrap();
    drop(native);
    let mut source = private.read_file("source.aiff").unwrap();
    source.seek(SeekFrom::Start(100)).unwrap(); // Constructor must rewind the handle.
    let target = private.create_file("import.wav").unwrap();
    let imported =
        import_audio_from_files(source, target, &[1, 0], &AtomicBool::new(false)).unwrap();
    assert_eq!(imported.frames, 8);
    let mut audio = AudioReader::from_file(private.read_file("import.wav").unwrap()).unwrap();
    let mut left = [0.; 8];
    let mut right = left;
    audio.read_stereo(0, &mut left, &mut right).unwrap();
    assert_eq!(left[0], 0.5);
    assert_eq!(right[0], -1.);
    let artifact = private.create_file("export.aiff").unwrap();
    let mut writer = AudioWriter::from_file(artifact, 8, 48000, AudioExport::AiffPcm24).unwrap();
    writer.write(&left, &right).unwrap();
    writer.finish().unwrap();
    assert_eq!(
        AudioReader::from_file(private.read_file("export.aiff").unwrap())
            .unwrap()
            .info
            .frames,
        8
    );
    let existing = fs::read(private_path.join("export.aiff")).unwrap();
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(private_path.join("export.aiff"))
        .unwrap();
    assert!(AudioWriter::from_file(file, 8, 48000, AudioExport::AiffPcm24).is_err());
    assert_eq!(
        fs::read(private_path.join("export.aiff")).unwrap(),
        existing
    );
}

#[test]
#[cfg(unix)]
fn owned_media_import_ignores_source_name_replaced_by_symlink() {
    use soundroom_engine::media::import_audio_from_files;
    use std::{io::Write, os::unix::fs::symlink};
    let temp = Temp::new();
    let path = temp.0.canonicalize().unwrap().join("private");
    let private = soundroom_platform::PrivateDirectory::create_root(&path).unwrap();
    let mut file = private.create_file("original").unwrap();
    file.write_all(&fs::read(root().join("pcm16.aiff")).unwrap())
        .unwrap();
    drop(file);
    let source = private.read_file("original").unwrap();
    fs::rename(path.join("original"), path.join("retained")).unwrap();
    fs::write(path.join("foreign"), b"must not read or change").unwrap();
    symlink("foreign", path.join("original")).unwrap();
    let result = import_audio_from_files(
        source,
        private.create_file("new.wav").unwrap(),
        &[0],
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(result.frames, 8);
    assert_eq!(
        fs::read(path.join("foreign")).unwrap(),
        b"must not read or change"
    );
    let mut reader = AudioReader::from_file(private.read_file("new.wav").unwrap()).unwrap();
    let mut left = [0.; 8];
    let mut right = left;
    reader.read_stereo(0, &mut left, &mut right).unwrap();
    assert_eq!(left[0], -1.);
    assert_eq!(left, right);
}

#[test]
#[cfg(unix)]
fn owned_media_constructors_reject_nonregular_handles_before_codec_writes() {
    let null = || {
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")
            .unwrap()
    };
    assert!(AudioReader::from_file(null()).is_err());
    assert!(soundroom_engine::wave::WaveWriter::from_file(null(), 0).is_err());
    assert!(AudioWriter::from_file(null(), 0, 48000, AudioExport::AiffFloat32).is_err());
}

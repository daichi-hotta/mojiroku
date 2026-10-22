//! 音声デコード: 任意フォーマット → **16kHz / mono / f32[-1,1]**（whisper 入力仕様）。
//!
//! symphonia でデコード（planar/interleaved・i16/i32/f32 を `SampleBuffer<f32>` で正規化）→
//! mono ダウンミックス → rubato で 16kHz にリサンプル。

use std::fs::File;
use std::path::Path;

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::error::{CoreError, Result};

/// whisper が要求するサンプルレート。
pub const WHISPER_SAMPLE_RATE: u32 = 16_000;

/// 音声ファイルをデコードし、16kHz mono f32 PCM を返す。
pub fn decode_to_pcm16k_mono<P: AsRef<Path>>(path: P) -> Result<Vec<f32>> {
    let (mono, src_rate) = decode_to_mono_f32(path)?;
    Ok(resample_to_16k(&mono, src_rate))
}

/// デコード → mono f32（元のサンプルレートのまま）。
fn decode_to_mono_f32<P: AsRef<Path>>(path: P) -> Result<(Vec<f32>, u32)> {
    let file = File::open(path.as_ref()).map_err(|e| CoreError::Io(e.to_string()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.as_ref().extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| CoreError::Audio(format!("probe: {e}")))?;
    let mut format = probed.format;

    let track = format
        .default_track()
        .ok_or_else(|| CoreError::Audio("no default track".into()))?;
    let track_id = track.id;
    let src_rate = track
        .codec_params
        .sample_rate
        .ok_or_else(|| CoreError::Audio("unknown sample rate".into()))?;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| CoreError::Audio(format!("make decoder: {e}")))?;

    let mut mono: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            // ストリーム終端
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(e) => return Err(CoreError::Audio(format!("next_packet: {e}"))),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let channels = spec.channels.count().max(1);
                // 任意のサンプル形式を f32 interleaved [-1,1] に変換
                let mut sbuf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                sbuf.copy_interleaved_ref(decoded);
                for frame in sbuf.samples().chunks(channels) {
                    let sum: f32 = frame.iter().copied().sum();
                    mono.push(sum / channels as f32);
                }
            }
            // 破損パケットはスキップ
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(CoreError::Audio(format!("decode: {e}"))),
        }
    }

    Ok((mono, src_rate))
}

/// mono f32 を 16kHz にリサンプル（rubato, 高品質 sinc）。すでに 16k ならそのまま返す。
fn resample_to_16k(input: &[f32], src_rate: u32) -> Vec<f32> {
    if src_rate == WHISPER_SAMPLE_RATE || input.is_empty() {
        return input.to_vec();
    }

    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    let chunk = 1024usize;
    let mut resampler = match SincFixedIn::<f32>::new(
        WHISPER_SAMPLE_RATE as f64 / src_rate as f64,
        2.0,
        params,
        chunk,
        1,
    ) {
        Ok(r) => r,
        Err(_) => return input.to_vec(),
    };

    let mut out: Vec<f32> =
        Vec::with_capacity(input.len() * WHISPER_SAMPLE_RATE as usize / src_rate as usize + chunk);
    let mut pos = 0;
    while pos + chunk <= input.len() {
        let wave_in = [input[pos..pos + chunk].to_vec()];
        if let Ok(wave_out) = resampler.process(&wave_in, None) {
            out.extend_from_slice(&wave_out[0]);
        }
        pos += chunk;
    }
    // 端数はゼロ詰めして最後に流す（末尾に僅かな無音が付くが MVP では許容）
    if pos < input.len() {
        let mut last = input[pos..].to_vec();
        last.resize(chunk, 0.0);
        if let Ok(wave_out) = resampler.process(&[last], None) {
            out.extend_from_slice(&wave_out[0]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, secs: f32, freq: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        (0..n)
            .map(|i| (i as f32 * freq * 2.0 * std::f32::consts::PI / rate as f32).sin() * 0.5)
            .collect()
    }

    #[test]
    fn passthrough_when_already_16k() {
        let input = sine(16_000, 1.0, 440.0);
        let out = resample_to_16k(&input, 16_000);
        assert_eq!(out.len(), input.len());
    }

    #[test]
    fn resample_48k_to_16k_length_and_range() {
        let input = sine(48_000, 1.0, 440.0); // 1 秒
        let out = resample_to_16k(&input, 48_000);
        // 期待長 ≈ 16000（チャンク境界の誤差を許容）
        assert!(
            (out.len() as i32 - 16_000).abs() < 2048,
            "resampled len = {}",
            out.len()
        );
        // 値域 [-1,1]・有限
        assert!(out.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
    }

    #[test]
    fn resample_44100_to_16k_is_finite() {
        let input = sine(44_100, 0.5, 1000.0);
        let out = resample_to_16k(&input, 44_100);
        assert!(!out.is_empty());
        assert!(out.iter().all(|x| x.is_finite()));
    }

    // ---- 取り込みで選べる形式のデコード（AUDIO_EXT / IMPORT_EXTENSIONS の aiff・flac・ogg） ----

    /// 振幅 0.5 の 440 Hz を 1 秒ぶん、16 kHz mono に直せているか。長さで 16 kHz と mono
    /// （ステレオを並べたままなら 2 倍になる）を、ゼロ交差の数で元のサンプルレートの解釈を、
    /// RMS で中身が無音や雑音でないことを確かめる。ゼロ交差と RMS は、リサンプラの立ち上がりと
    /// 末尾のゼロ詰めを避けて 0.1〜0.9 秒の区間で数える（440 Hz なら 0.8 秒で 704 回）。
    fn assert_one_second_440hz_at_16k(pcm: &[f32]) {
        assert!(
            (pcm.len() as i32 - 16_000).abs() < 1024,
            "decoded len = {}",
            pcm.len()
        );
        assert!(pcm.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
        let middle = &pcm[1_600..14_400];
        let crossings = middle
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        assert!(
            (694..=714).contains(&crossings),
            "zero crossings = {crossings}"
        );
        let rms = (middle.iter().map(|s| s * s).sum::<f32>() / middle.len() as f32).sqrt();
        assert!((0.3..0.4).contains(&rms), "rms = {rms}");
    }

    fn sine_i16(rate: u32) -> Vec<i16> {
        sine(rate, 1.0, 440.0)
            .into_iter()
            .map(|s| (s * i16::MAX as f32) as i16)
            .collect()
    }

    fn decode_bytes(name: &str, bytes: &[u8]) -> Vec<f32> {
        let path =
            std::env::temp_dir().join(format!("mojiroku_audio_test_{}_{name}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let pcm = decode_to_pcm16k_mono(&path);
        let _ = std::fs::remove_file(&path);
        pcm.unwrap()
    }

    /// AIFF の COMM が持つ 80 ビット拡張精度浮動小数のサンプルレート。
    fn ieee_extended(rate: u32) -> [u8; 10] {
        let msb = 31 - rate.leading_zeros();
        let mut out = [0u8; 10];
        out[..2].copy_from_slice(&(16_383 + msb as u16).to_be_bytes());
        out[2..].copy_from_slice(&((rate as u64) << (63 - msb)).to_be_bytes());
        out
    }

    /// 非圧縮 AIFF（16 bit big-endian・ステレオ）を組み立てる。
    fn aiff_stereo(rate: u32, samples: &[i16]) -> Vec<u8> {
        let mut ssnd = vec![0u8; 8]; // offset, block size
        for s in samples {
            ssnd.extend_from_slice(&s.to_be_bytes()); // L
            ssnd.extend_from_slice(&s.to_be_bytes()); // R
        }
        let mut comm = Vec::new();
        comm.extend_from_slice(&2u16.to_be_bytes()); // channels
        comm.extend_from_slice(&(samples.len() as u32).to_be_bytes()); // frames
        comm.extend_from_slice(&16u16.to_be_bytes()); // bits per sample
        comm.extend_from_slice(&ieee_extended(rate));

        let mut body = b"AIFF".to_vec();
        for (id, chunk) in [(b"COMM", &comm), (b"SSND", &ssnd)] {
            body.extend_from_slice(id);
            body.extend_from_slice(&(chunk.len() as u32).to_be_bytes());
            body.extend_from_slice(chunk);
        }
        let mut out = b"FORM".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out
    }

    /// FLAC のフレームヘッダの CRC-8（多項式 0x07）。
    fn flac_crc8(bytes: &[u8]) -> u8 {
        let mut crc = 0u8;
        for &b in bytes {
            crc ^= b;
            for _ in 0..8 {
                crc = if crc & 0x80 != 0 {
                    (crc << 1) ^ 0x07
                } else {
                    crc << 1
                };
            }
        }
        crc
    }

    /// FLAC のフレーム全体の CRC-16（多項式 0x8005）。
    fn flac_crc16(bytes: &[u8]) -> u16 {
        let mut crc = 0u16;
        for &b in bytes {
            crc ^= (b as u16) << 8;
            for _ in 0..8 {
                crc = if crc & 0x8000 != 0 {
                    (crc << 1) ^ 0x8005
                } else {
                    crc << 1
                };
            }
        }
        crc
    }

    /// 無圧縮（VERBATIM サブフレーム）の FLAC（16 bit・mono・48 kHz）を組み立てる。
    fn flac_mono_48k(samples: &[i16]) -> Vec<u8> {
        const BLOCK: usize = 4096;
        let mut out = b"fLaC".to_vec();
        out.extend_from_slice(&[0x80, 0, 0, 34]); // 最後のメタデータブロック・STREAMINFO・34 バイト
        out.extend_from_slice(&(BLOCK as u16).to_be_bytes()); // 最小ブロック長
        out.extend_from_slice(&(BLOCK as u16).to_be_bytes()); // 最大ブロック長
        out.extend_from_slice(&[0; 6]); // 最小・最大フレーム長（不明）
                                        // サンプルレート 20 bit・チャンネル数-1 3 bit・ビット深度-1 5 bit・総サンプル数 36 bit
        let packed = (48_000u64 << 44) | (15 << 36) | samples.len() as u64;
        out.extend_from_slice(&packed.to_be_bytes());
        out.extend_from_slice(&[0; 16]); // MD5（不明）

        for (n, block) in samples.chunks(BLOCK).enumerate() {
            // 同期符号・固定ブロック長／ブロック長は末尾に 16 bit・48 kHz／mono・16 bit／フレーム番号
            let mut frame = vec![0xFF, 0xF8, 0x7A, 0x08, n as u8];
            frame.extend_from_slice(&(block.len() as u16 - 1).to_be_bytes());
            frame.push(flac_crc8(&frame));
            frame.push(0x02); // VERBATIM サブフレーム
            for s in block {
                frame.extend_from_slice(&s.to_be_bytes());
            }
            let crc16 = flac_crc16(&frame);
            frame.extend_from_slice(&crc16.to_be_bytes());
            out.extend_from_slice(&frame);
        }
        out
    }

    #[test]
    fn decodes_aiff_to_16k_mono() {
        let aiff = aiff_stereo(44_100, &sine_i16(44_100));
        assert_one_second_440hz_at_16k(&decode_bytes("sine.aiff", &aiff));
    }

    #[test]
    fn decodes_flac_to_16k_mono() {
        let flac = flac_mono_48k(&sine_i16(48_000));
        assert_one_second_440hz_at_16k(&decode_bytes("sine.flac", &flac));
    }

    /// Vorbis は手で組み立てられないので小さな実ファイルを使う。作り方（FFmpeg 9.0.2）:
    /// `ffmpeg -f lavfi -i "aevalsrc=0.5*sin(2*PI*440*t):s=44100:d=1:c=stereo" -c:a vorbis
    /// -strict -2 -map_metadata -1 -fflags +bitexact -flags:a +bitexact sine_440hz_1s.ogg`
    #[test]
    fn decodes_ogg_vorbis_to_16k_mono() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sine_440hz_1s.ogg");
        assert_one_second_440hz_at_16k(&decode_to_pcm16k_mono(path).unwrap());
    }
}

//! Meeting crosstalk: drop microphone segments that only contain the other participants
//! leaking into the microphone (ADR-0048).
//!
//! With speakers instead of headphones, the mic track also hears the remote side, far more
//! quietly than the owner's own voice. VAD rightly keeps it (it is speech), and Whisper turns
//! the faint copy into degraded text or a stock phrase such as 「ご視聴ありがとうございました」
//! next to the clean system-track transcript of the same words. This is not echo
//! cancellation: after transcription, each mic segment's level is compared with the system
//! track at the same moment and with the owner's usual speaking level.

use std::path::Path;

use crate::schemas::Transcript;

/// 100 ms frames of 16 kHz PCM.
const FRAME_SAMPLES: usize = 1_600;
const FRAME_MS: i64 = 100;

/// The system track has speech during the segment (dBFS over the segment).
pub const SYSTEM_ACTIVE_DBFS: f32 = -45.0;
/// A leaked segment is quieter on the mic than on the system track, and at least this far
/// below the owner's own speaking level. A loud reply over someone else is kept.
pub const BELOW_OWN_VOICE_DB: f32 = 10.0;
/// Own-voice segments, used to learn the owner's level: the mic is at least this much
/// louder than the system track.
const OWN_VOICE_LEAD_DB: f32 = 20.0;
/// Without this many own-voice segments the level is unknown and nothing is dropped.
const OWN_VOICE_MIN_SEGMENTS: usize = 3;

/// Mean-square energy per 100 ms frame of 16 kHz mono PCM, so the PCM itself can be
/// released before segments are compared.
pub struct Envelope {
    frames: Vec<f32>,
}

impl Envelope {
    pub fn from_pcm(pcm: &[f32]) -> Self {
        let frames = pcm
            .chunks(FRAME_SAMPLES)
            .map(|frame| frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32)
            .collect();
        Self { frames }
    }

    /// Level in dBFS over `[start_ms, end_ms)`, or `None` when that lies outside the audio.
    pub fn dbfs(&self, start_ms: i64, end_ms: i64) -> Option<f32> {
        let first = (start_ms.max(0) / FRAME_MS) as usize;
        let last = ((end_ms.max(0) + FRAME_MS - 1) / FRAME_MS) as usize;
        let last = last.min(self.frames.len());
        if first >= last {
            return None;
        }
        let mean = self.frames[first..last].iter().sum::<f32>() / (last - first) as f32;
        Some(10.0 * mean.max(1e-12).log10())
    }
}

/// What [`drop_mic_crosstalk`] decided, for the diagnostic line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrosstalkReport {
    /// `None` when there were too few clear own-voice segments to judge (nothing dropped).
    pub own_voice_dbfs: Option<f32>,
    pub dropped: usize,
    pub kept: usize,
}

/// Remove mic segments that look like the system track leaking into the microphone.
///
/// Segment times are mic-track times. `mic_offset_ms` is the recording's stored offset (the
/// mic track started that much later), so mic time `t` is system time `t + mic_offset_ms`,
/// whichever sign it has (the same convention as [`crate::merge::merge_tracks`]).
pub fn drop_mic_crosstalk(
    mic: &mut Transcript,
    mic_env: &Envelope,
    system_env: &Envelope,
    mic_offset_ms: i64,
) -> CrosstalkReport {
    let levels: Vec<Option<(f32, f32)>> = mic
        .segments
        .iter()
        .map(|s| {
            let (start, end) = (s.start_ms as i64, s.end_ms as i64);
            let mic_db = mic_env.dbfs(start, end)?;
            let system_db = system_env.dbfs(start + mic_offset_ms, end + mic_offset_ms)?;
            Some((mic_db, system_db))
        })
        .collect();
    let Some(own) = own_voice_dbfs(&levels) else {
        return CrosstalkReport {
            own_voice_dbfs: None,
            dropped: 0,
            kept: mic.segments.len(),
        };
    };
    let mut leaked = levels
        .iter()
        .map(|level| matches!(level, Some((m, y)) if is_leak(*m, *y, own)));
    let before = mic.segments.len();
    mic.segments.retain(|_| !leaked.next().unwrap_or(false));
    CrosstalkReport {
        own_voice_dbfs: Some(own),
        dropped: before - mic.segments.len(),
        kept: mic.segments.len(),
    }
}

/// Decode both meeting tracks again for their levels and apply [`drop_mic_crosstalk`].
/// Best effort: a track that cannot be decoded leaves the mic transcript unchanged.
pub fn filter_mic_track(
    mic: &mut Transcript,
    mic_path: &Path,
    system_path: &Path,
    mic_offset_ms: i64,
) -> Option<CrosstalkReport> {
    if mic.segments.is_empty() {
        return None;
    }
    // One track's PCM at a time; only the small envelopes are kept.
    let envelope = |path: &Path| match crate::audio::decode_to_pcm16k_mono(path) {
        Ok(pcm) => Some(Envelope::from_pcm(&pcm)),
        Err(error) => {
            eprintln!("meeting crosstalk: skipped, could not decode a track: {error}");
            None
        }
    };
    let mic_env = envelope(mic_path)?;
    let system_env = envelope(system_path)?;
    let report = drop_mic_crosstalk(mic, &mic_env, &system_env, mic_offset_ms);
    eprintln!(
        "meeting crosstalk: own voice {}, dropped {} of {} mic segments",
        report
            .own_voice_dbfs
            .map_or("unknown".to_string(), |db| format!("{db:.1} dBFS")),
        report.dropped,
        report.dropped + report.kept,
    );
    Some(report)
}

fn is_leak(mic_db: f32, system_db: f32, own_db: f32) -> bool {
    system_db > SYSTEM_ACTIVE_DBFS && mic_db < system_db && mic_db < own_db - BELOW_OWN_VOICE_DB
}

/// Median mic level of the segments where the system track is clearly quieter.
fn own_voice_dbfs(levels: &[Option<(f32, f32)>]) -> Option<f32> {
    let mut own: Vec<f32> = levels
        .iter()
        .flatten()
        .filter(|(m, y)| m - y >= OWN_VOICE_LEAD_DB)
        .map(|(m, _)| *m)
        .collect();
    if own.len() < OWN_VOICE_MIN_SEGMENTS {
        return None;
    }
    own.sort_unstable_by(f32::total_cmp);
    let mid = own.len() / 2;
    Some(if own.len().is_multiple_of(2) {
        (own[mid - 1] + own[mid]) / 2.0
    } else {
        own[mid]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schemas::Segment;

    /// 16 kHz PCM: a square wave of `amplitude` inside each `(start_ms, end_ms)` span.
    fn pcm(total_ms: usize, spans: &[(usize, usize, f32)]) -> Vec<f32> {
        let mut samples = vec![0.0; total_ms * 16];
        for &(start, end, amplitude) in spans {
            for (n, sample) in samples[start * 16..end * 16].iter_mut().enumerate() {
                *sample = if n % 2 == 0 { amplitude } else { -amplitude };
            }
        }
        samples
    }

    fn transcript(spans: &[(u64, u64)]) -> Transcript {
        Transcript {
            language: Some("ja".into()),
            segments: spans
                .iter()
                .map(|&(start_ms, end_ms)| Segment {
                    idx: 0,
                    start_ms,
                    end_ms,
                    text: format!("{start_ms}"),
                    speaker_id: None,
                })
                .collect(),
        }
    }

    fn texts(t: &Transcript) -> Vec<&str> {
        t.segments.iter().map(|s| s.text.as_str()).collect()
    }

    const OWN: f32 = 0.06; // about -24 dBFS
    const LEAK: f32 = 0.003; // about -50 dBFS
    const REMOTE: f32 = 0.06;

    #[test]
    fn envelope_levels_and_bounds() {
        let env = Envelope::from_pcm(&pcm(2_000, &[(0, 1_000, 0.5)]));
        let loud = env.dbfs(0, 1_000).unwrap();
        assert!((loud - 20.0 * 0.5f32.log10()).abs() < 0.01, "{loud}");
        assert!((env.dbfs(0, 2_000).unwrap() - (loud - 3.01)).abs() < 0.05);
        assert!(env.dbfs(1_000, 2_000).unwrap() < -100.0);
        assert_eq!(env.dbfs(5_000, 6_000), None);
        assert_eq!(env.dbfs(500, 500), None);
    }

    #[test]
    fn drops_faint_mic_copies_of_remote_speech_only() {
        // Own voice at 0-1, 2-3 and 4-5 s with a silent system track; remote speech at
        // 10-14 s leaks faintly into the mic; a loud reply at 12-13 s overlaps it.
        let mic = pcm(
            20_000,
            &[
                (0, 1_000, OWN),
                (2_000, 3_000, OWN),
                (4_000, 5_000, OWN),
                (10_000, 12_000, LEAK),
                (12_000, 13_000, OWN),
                (13_000, 14_000, LEAK),
            ],
        );
        let system = pcm(20_000, &[(10_000, 14_000, REMOTE)]);
        let mut t = transcript(&[
            (0, 1_000),
            (2_000, 3_000),
            (4_000, 5_000),
            (10_000, 12_000),
            (12_000, 13_000),
            (13_000, 14_000),
        ]);
        let report = drop_mic_crosstalk(
            &mut t,
            &Envelope::from_pcm(&mic),
            &Envelope::from_pcm(&system),
            0,
        );
        assert_eq!(texts(&t), ["0", "2000", "4000", "12000"]);
        assert_eq!((report.dropped, report.kept), (2, 4));
        assert!((report.own_voice_dbfs.unwrap() - 20.0 * OWN.log10()).abs() < 0.1);
    }

    #[test]
    fn keeps_everything_without_enough_own_voice_segments() {
        let mic = pcm(20_000, &[(0, 1_000, OWN), (10_000, 12_000, LEAK)]);
        let system = pcm(20_000, &[(10_000, 12_000, REMOTE)]);
        let mut t = transcript(&[(0, 1_000), (10_000, 12_000)]);
        let report = drop_mic_crosstalk(
            &mut t,
            &Envelope::from_pcm(&mic),
            &Envelope::from_pcm(&system),
            0,
        );
        assert_eq!(report.own_voice_dbfs, None);
        assert_eq!(t.segments.len(), 2);
    }

    #[test]
    fn keeps_quiet_mic_text_when_the_system_track_is_silent() {
        // Quiet own speech with nobody else talking is not crosstalk.
        let mic = pcm(
            20_000,
            &[
                (0, 1_000, OWN),
                (2_000, 3_000, OWN),
                (4_000, 5_000, OWN),
                (10_000, 12_000, LEAK),
            ],
        );
        let system = pcm(20_000, &[]);
        let mut t = transcript(&[(0, 1_000), (2_000, 3_000), (4_000, 5_000), (10_000, 12_000)]);
        drop_mic_crosstalk(
            &mut t,
            &Envelope::from_pcm(&mic),
            &Envelope::from_pcm(&system),
            0,
        );
        assert_eq!(t.segments.len(), 4);
    }

    #[test]
    fn compares_against_the_system_track_at_the_offset_time() {
        // The mic track started 2 s later: mic 8-10 s is system 10-12 s.
        let mic = pcm(
            20_000,
            &[
                (0, 1_000, OWN),
                (2_000, 3_000, OWN),
                (4_000, 5_000, OWN),
                (8_000, 10_000, LEAK),
            ],
        );
        let system = pcm(22_000, &[(10_000, 12_000, REMOTE)]);
        let spans = [(0, 1_000), (2_000, 3_000), (4_000, 5_000), (8_000, 10_000)];
        let mut aligned = transcript(&spans);
        drop_mic_crosstalk(
            &mut aligned,
            &Envelope::from_pcm(&mic),
            &Envelope::from_pcm(&system),
            2_000,
        );
        assert_eq!(aligned.segments.len(), 3);
        let mut ignored_offset = transcript(&spans);
        drop_mic_crosstalk(
            &mut ignored_offset,
            &Envelope::from_pcm(&mic),
            &Envelope::from_pcm(&system),
            0,
        );
        assert_eq!(ignored_offset.segments.len(), 4);
    }
}

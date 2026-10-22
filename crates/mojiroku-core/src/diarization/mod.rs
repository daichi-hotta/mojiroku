//! 話者分離。sherpa-onnx（pyannote segmentation-3.0 の ONNX + TitaNet 埋め込み +
//! しきい値クラスタリング、torch なし）。会議は話者数未知のためしきい値ベースが肝
//! （`docs/05_decisions/ADR-0004_話者分離はsherpa-onnxで実現.md` / スパイク結果 `ADR-0009_話者分離スパイク結果.md`）。
//!
//! sherpa の素の出力は 3 人に 8–13 クラスタと過分割する（尾の小クラスタ）。そこで
//! **consolidation**（turn 単位 TitaNet 埋め込み → 尺で anchor を選び、全 turn を最近接
//! anchor へ再割当）で ~実話者数へ畳む。スパイクで A 75→94% 等の改善を実機確認済み（ADR-0009）。

use std::collections::BTreeMap;
use std::path::PathBuf;

use sherpa_onnx::{
    FastClusteringConfig, OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig,
    OfflineSpeakerDiarizationSegment, OfflineSpeakerSegmentationModelConfig,
    OfflineSpeakerSegmentationPyannoteModelConfig, SpeakerEmbeddingExtractor,
    SpeakerEmbeddingExtractorConfig,
};

use crate::error::{CoreError, Result};
use crate::lang::{default_speaker_label, Lang};
use crate::schemas::Speaker;

/// しきい値クラスタリングの既定（ADR-0009 のスイープで A/B/C 分離 + 被覆 98-99%）。
pub const DEFAULT_THRESHOLD: f32 = 0.80;

/// consolidation の anchor 採用しきい値: 絶対秒と総発話尺の割合の大きい方。
/// 「十分喋った話者」を実話者の anchor とみなす。短時間話者は anchor 未満で吸収される
/// （既知の限界・ADR-0009）。
const ANCHOR_MIN_SECONDS: f32 = 15.0;
const ANCHOR_MIN_FRACTION: f32 = 0.06;
// Duration seeds stable groups; shorter distinct voices are retained below.
// A merge needs positive voice evidence, not merely a nearest available group.
const MIN_MERGE_COSINE: f32 = 0.65;
// A weak match is not enough evidence to create another speaker. Require both
// sustained speech and clear separation from the existing group centroids.
const NEW_SPEAKER_MIN_SECONDS: f32 = 1.0;
const NEW_SPEAKER_MAX_COSINE: f32 = 0.40;
// Embeddings of short turns are noisy: in a 32-minute one-on-one, the dominant
// speaker's own sub-2 s turns scored a median cosine of 0.40 against that speaker's
// centroid. A candidate made only of such turns must be less similar to an anchor
// than the anchor's own short turns usually are (ADR-0042).
const SHORT_TURN_SECONDS: f32 = 2.0;
const SHORT_TURN_MIN_SAMPLES: usize = 10;
const SHORT_TURN_PERCENTILE: f32 = 0.05;

/// 埋め込み抽出に必要な最小窓（これ未満の turn は窓を広げて抽出を試みる）。
const EMBED_MIN_SECONDS: f32 = 0.3;

/// 埋め込み抽出に渡す最大窓。TitaNet の ONNX は入力 12288 特徴フレーム（= 122.88 秒）を
/// 超えると内部 mask の broadcast が壊れて C++ 例外を投げる（実音声で 122s OK / 123s FAIL、
/// エラー "Attempting to broadcast ... 12288 by 17364" を実測）。長い独演 turn でも話者性は
/// 数十秒あれば十分取れるため、turn の中央 120 秒へ切り詰めて回避する。
const EMBED_MAX_SECONDS: f32 = 120.0;

/// onnxruntime の intra-op スレッド数上限。sherpa-onnx の既定は 1（シングルコア）で、
/// これが長尺会議の遅さの主因だった。実会議 6 分スライスの実測（4P+4E コア機）で
/// 1→4 スレッドは 166s→69s（出力は bit 一致）、8 スレッドは 98s と逆に悪化
/// （E コア競合）したため 4 で頭打ちにする。重い ML ジョブは acquire_heavy_job で
/// アプリ全体 1 本に直列化済みなので、スレッド増がメモリピークを押し上げる心配はない。
const ONNX_MAX_THREADS: usize = 4;

/// このマシンで使う onnxruntime スレッド数（論理コア数と `ONNX_MAX_THREADS` の小さい方）。
fn onnx_num_threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get().min(ONNX_MAX_THREADS))
        .unwrap_or(1) as i32
}

/// 話者ターン（時間区間と話者）。
#[derive(Debug, Clone)]
pub struct SpeakerTurn {
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker_id: String,
}

/// 話者の声紋（重心埋め込み, L2 正規化済み）。クロス会議照合（話者ライブラリ・ADR-0018）用。
/// consolidation で話者へ再割当した全 turn の TitaNet 埋め込みを尺重み平均したもの。
#[derive(Debug, Clone)]
pub struct SpeakerEmbedding {
    pub speaker_id: String,
    pub vector: Vec<f32>,
    /// この話者の総発話尺（ms）。最小エンロール秒数ゲート（ADR-0018）の判定に使う
    /// ── 短い音声では同一人物でも一致が崩れるため、照合/登録の対象可否を尺で見る。
    pub duration_ms: u64,
}

/// 話者分離の結果。
#[derive(Debug, Clone, Default)]
pub struct DiarizationResult {
    pub speakers: Vec<Speaker>,
    pub turns: Vec<SpeakerTurn>,
    /// 話者ごとの声紋（`speakers` と同じ id 空間＝S1.. 。話者ライブラリ照合用・ADR-0018）。
    /// 埋め込みが取れなかった話者は欠落しうる（短すぎ等）。
    pub embeddings: Vec<SpeakerEmbedding>,
}

/// 後付け（再）話者分離で、旧話者の表示名（改名）を新話者へベスト努力で引き継ぐ（ADR-0024）。
/// 旧新それぞれ `(Speaker, 声紋)` を渡し、声紋 cosine の大きいペアから貪欲に対応づける。
/// `min_cos` 未満のペアは別人とみなし引き継がない。戻り値は**各新話者**の
/// `(新 speaker_id, 引き継ぐ display_name)`（対応なし or 旧側が未改名なら None）。
///
/// 声紋は L2 正規化済み前提（内積＝cosine）。次元不一致（別モデル）ペアは対象外。
pub fn carry_display_names(
    old: &[(Speaker, Vec<f32>)],
    new: &[(Speaker, Vec<f32>)],
    min_cos: f32,
) -> Vec<(String, Option<String>)> {
    // 全ペアの cosine を作り、降順に貪欲マッチ（1 対 1）。
    let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
    for (oi, (_, ov)) in old.iter().enumerate() {
        for (ni, (_, nv)) in new.iter().enumerate() {
            if ov.len() != nv.len() || ov.is_empty() {
                continue;
            }
            let cos: f32 = ov.iter().zip(nv).map(|(a, b)| a * b).sum();
            if cos >= min_cos {
                pairs.push((cos, oi, ni));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut old_used = vec![false; old.len()];
    let mut carried: Vec<Option<String>> = vec![None; new.len()];
    let mut new_matched = vec![false; new.len()];
    for (_, oi, ni) in pairs {
        if old_used[oi] || new_matched[ni] {
            continue;
        }
        old_used[oi] = true;
        new_matched[ni] = true;
        carried[ni] = old[oi].0.display_name.clone();
    }
    new.iter()
        .enumerate()
        .map(|(ni, (sp, _))| (sp.id.clone(), carried[ni].take()))
        .collect()
}

/// 話者分離の抽象。
pub trait Diarizer {
    /// `pcm` は **原音声 16kHz mono**（VAD で無音除去する前）を渡すこと。VAD 後の PCM を
    /// 食わせると無音跨ぎの連結が偽の話者交替を生む（ADR-0008/0009 のトポロジ注意）。
    fn diarize(&self, pcm: &[f32], sample_rate: u32) -> Result<DiarizationResult>;
}

/// sherpa-onnx による話者分離器。モデルパスと threshold を保持し、`diarize` のたびに
/// 構成して推論する（sherpa の重い計算は prebuilt onnxruntime 側）。
pub struct SherpaDiarizer {
    seg_model: PathBuf,
    emb_model: PathBuf,
    threshold: f32,
    /// 既定話者ラベル（「話者N」/「Speaker N」）の言語。ラベルは DB に保存されるため
    /// **生成時の言語で固定**される仕様（後から設定を変えても過去データは変えない）。
    lang: Lang,
}

impl SherpaDiarizer {
    /// segmentation（pyannote seg-3.0）と embedding（TitaNet）の onnx パスから構成。
    pub fn new(
        seg_model: impl Into<PathBuf>,
        emb_model: impl Into<PathBuf>,
        threshold: f32,
        lang: Lang,
    ) -> Self {
        Self {
            seg_model: seg_model.into(),
            emb_model: emb_model.into(),
            threshold,
            lang,
        }
    }

    fn config(&self) -> OfflineSpeakerDiarizationConfig {
        OfflineSpeakerDiarizationConfig {
            segmentation: OfflineSpeakerSegmentationModelConfig {
                pyannote: OfflineSpeakerSegmentationPyannoteModelConfig {
                    model: Some(self.seg_model.to_string_lossy().into_owned()),
                },
                num_threads: onnx_num_threads(),
                ..Default::default()
            },
            embedding: SpeakerEmbeddingExtractorConfig {
                model: Some(self.emb_model.to_string_lossy().into_owned()),
                num_threads: onnx_num_threads(),
                ..Default::default()
            },
            clustering: FastClusteringConfig {
                // 話者数未知 → しきい値ベース（固定数は潰れる。ADR-0009）。
                num_clusters: -1,
                threshold: self.threshold,
            },
            min_duration_on: 0.3,
            min_duration_off: 0.5,
        }
    }
}

impl Diarizer for SherpaDiarizer {
    fn diarize(&self, pcm: &[f32], sample_rate: u32) -> Result<DiarizationResult> {
        // FFI 例外シールド: onnxruntime の C++ 例外（メモリ枯渇の bad_alloc 等）を Err に変換。
        // シールド無しだと例外が tokio の catch_unwind に達してプロセスごと abort する
        // （docs/error.md の実クラッシュ）。
        crate::ffi_guard::guard("話者分離 (sherpa-onnx)", || {
            self.diarize_inner(pcm, sample_rate)
        })?
    }
}

impl SherpaDiarizer {
    fn diarize_inner(&self, pcm: &[f32], sample_rate: u32) -> Result<DiarizationResult> {
        let sd = OfflineSpeakerDiarization::create(&self.config())
            .ok_or_else(|| CoreError::Model("diarization の構成に失敗".into()))?;
        if sd.sample_rate() as u32 != sample_rate {
            return Err(CoreError::Model(format!(
                "diarization は {}Hz を要求（入力 {}Hz）。原音声を 16k mono で渡すこと",
                sd.sample_rate(),
                sample_rate
            )));
        }
        let t_proc = std::time::Instant::now();
        let result = sd
            .process(pcm)
            .ok_or_else(|| CoreError::Model("diarization 推論に失敗".into()))?;
        let raw = result.sort_by_start_time();
        let proc_s = t_proc.elapsed().as_secs_f32();
        // segmentation + embedding の ONNX セッション 2 本をここで解放する。consolidate は
        // 自前の SpeakerEmbeddingExtractor を作るため、解放しないとセッションが 3 本同時に
        // 生存しメモリピークを押し上げる（16GB 機での bad_alloc 予防）。
        drop(result);
        drop(sd);
        if raw.is_empty() {
            return Ok(DiarizationResult::default());
        }
        // Issue #65 の調査用: 畳み込み（consolidate）前の生の区切りも書き出す。
        if let Some(path) = std::env::var_os("MOJIROKU_DEBUG_TURNS") {
            let rows: Vec<_> = raw
                .iter()
                .map(|s| serde_json::json!({"start": s.start, "end": s.end, "speaker": s.speaker}))
                .collect();
            let mut raw_path = std::path::PathBuf::from(path);
            raw_path.set_extension("raw.json");
            let _ = std::fs::write(raw_path, serde_json::to_vec_pretty(&rows).unwrap_or_default());
        }

        let t_cons = std::time::Instant::now();
        let (turns, centroids) = consolidate(&raw, pcm, sample_rate, &self.emb_model)?;
        let cons_s = t_cons.elapsed().as_secs_f32();
        // 長尺スケーリング調査用の計時（MOJIROKU_DIAR_TIMING=1 で有効）。
        if std::env::var_os("MOJIROKU_DIAR_TIMING").is_some() {
            eprintln!(
                "[diar-timing] audio={:.1}s raw_segments={} sherpa_process={:.1}s consolidate={:.1}s",
                pcm.len() as f32 / sample_rate as f32,
                raw.len(),
                proc_s,
                cons_s
            );
        }
        Ok(build_result(turns, centroids, self.lang))
    }
}

/// consolidation 後の 1 turn（秒, 再割当後のクラスタ id）。
struct Reassigned {
    start: f32,
    end: f32,
    label: i32,
}

/// クラスタ id → 声紋（L2 正規化済み埋め込みの重心）。
type ClusterEmbeddings = BTreeMap<i32, Vec<f32>>;

/// 過分割クラスタを実話者数へ畳む（Strategy A: 尺 anchor + 最近接 centroid 再割当）。
fn consolidate(
    raw: &[OfflineSpeakerDiarizationSegment],
    pcm: &[f32],
    sr: u32,
    emb_model: &std::path::Path,
) -> Result<(Vec<Reassigned>, ClusterEmbeddings)> {
    let extractor = SpeakerEmbeddingExtractor::create(&SpeakerEmbeddingExtractorConfig {
        model: Some(emb_model.to_string_lossy().into_owned()),
        num_threads: onnx_num_threads(),
        ..Default::default()
    })
    .ok_or_else(|| CoreError::Model("埋め込み抽出器の構成に失敗".into()))?;
    let dim = extractor.dim() as usize;

    // turn 単位の埋め込み（短すぎ/失敗は None）。
    let embs: Vec<Option<Vec<f32>>> = raw
        .iter()
        .map(|s| embed_segment(&extractor, pcm, sr, s.start, s.end))
        .collect();

    // クラスタごとに尺重み centroid と総尺を集計。
    let mut dur: BTreeMap<i32, f32> = BTreeMap::new();
    let mut acc: BTreeMap<i32, Vec<f32>> = BTreeMap::new();
    for (s, e) in raw.iter().zip(embs.iter()) {
        let w = s.end - s.start;
        *dur.entry(s.speaker).or_insert(0.0) += w;
        if let Some(emb) = e {
            let v = acc.entry(s.speaker).or_insert_with(|| vec![0.0; dim]);
            for (a, x) in v.iter_mut().zip(emb.iter()) {
                *a += x * w;
            }
        }
    }
    let mut centroid: BTreeMap<i32, Vec<f32>> = BTreeMap::new();
    for (c, mut v) in acc {
        l2_normalize(&mut v);
        if valid_embedding(&v) {
            centroid.insert(c, v);
        }
    }

    // anchor: 尺 >= max(絶対, 相対) かつ centroid を持つクラスタ（尺降順）。
    let evidence = ShortTurnEvidence::measure(raw, &embs, &centroid, &dur);
    let anchors = select_supported_anchors(&centroid, &dur, &evidence);

    if anchors.is_empty() {
        // 全 turn の埋め込みに失敗し centroid が空 → anchor を採れない。ここで空へ graceful
        // degrade する（`raw.is_empty()` の早期 return と同じ「話者未割当 transcript」経路に
        // 合流し、merge は文字起こしを話者ラベル無しで保持する）。この guard が無いと直後の
        // `anchors[0]` が index panic し、spawn_blocking の JoinError 経由でコマンドが Err に
        // なって同一ジョブの文字起こしごと失われる（ADR-0021 の 16GB 機メモリ枯渇 / FFI 資源
        // 枯渇で全 turn の埋め込みが None になる状況で現実に起こりうる severe な失敗モード）。
        return Ok((Vec::new(), BTreeMap::new()));
    }

    // 各 turn を最近接 anchor へ再割当。

    let out: Vec<Reassigned> = raw
        .iter()
        .zip(embs.iter())
        .map(|(s, e)| {
            let label = match e {
                // 自前の埋め込みがあれば最近接 anchor centroid。
                Some(emb) => supported_assignment(emb, s.speaker, &anchors, &centroid),
                // 短すぎて埋め込めない turn は元クラスタの centroid で代替、無ければ最大 anchor。
                None => centroid
                    .get(&s.speaker)
                    .map(|c| supported_assignment(c, s.speaker, &anchors, &centroid))
                    .unwrap_or(s.speaker),
            };
            Reassigned {
                start: s.start,
                end: s.end,
                label,
            }
        })
        .collect();

    // 話者（anchor）ごとの声紋（ADR-0018）: 再割当後の全 turn の埋め込みを尺重み平均 → L2。
    // consolidation が既に持つ per-turn 埋め込み（embs）を再利用するだけ（再抽出なし）。
    let mut emb_acc: BTreeMap<i32, Vec<f32>> = BTreeMap::new();
    for (r, e) in out.iter().zip(embs.iter()) {
        if let Some(emb) = e {
            let w = r.end - r.start;
            let v = emb_acc.entry(r.label).or_insert_with(|| vec![0.0; dim]);
            for (a, x) in v.iter_mut().zip(emb.iter()) {
                *a += x * w;
            }
        }
    }
    let speaker_centroids: BTreeMap<i32, Vec<f32>> = emb_acc
        .into_iter()
        .map(|(c, mut v)| {
            l2_normalize(&mut v);
            (c, v)
        })
        .collect();
    Ok((out, speaker_centroids))
}

/// centroid を持つクラスタから anchor（実話者とみなす「十分喋った」クラスタ）を尺降順で選ぶ。
/// 尺 >= max(絶対 `ANCHOR_MIN_SECONDS`, 相対 `ANCHOR_MIN_FRACTION`) を採り、floor で 1 つも
/// 残らなければ centroid 付きの最大尺クラスタ 1 つを採る。centroid が空（全 turn の埋め込みが
/// 失敗）なら空を返す＝呼び出し側で graceful degrade する（`anchors[0]` の index panic 予防）。
fn select_anchors(centroid: &BTreeMap<i32, Vec<f32>>, dur: &BTreeMap<i32, f32>) -> Vec<i32> {
    let total: f32 = dur.values().sum();
    let floor = (ANCHOR_MIN_FRACTION * total).max(ANCHOR_MIN_SECONDS);
    let mut anchors: Vec<i32> = centroid
        .keys()
        .copied()
        .filter(|c| dur.get(c).copied().unwrap_or(0.0) >= floor)
        .collect();
    if anchors.is_empty() {
        // floor で 1 つも残らない（極端に短い会議等）→ centroid 付きの最大尺クラスタ 1 つ。
        if let Some(&c) = centroid.keys().max_by(|a, b| cmp_f32(dur[a], dur[b])) {
            anchors.push(c);
        }
    }
    anchors.sort_by(|a, b| cmp_f32(dur[b], dur[a]));
    anchors
}

/// What the short turns of a recording say about voice similarity.
#[derive(Default)]
struct ShortTurnEvidence {
    /// Longest raw turn of each cluster, in seconds.
    longest: BTreeMap<i32, f32>,
    /// Per duration anchor: the cosine below which only a small share of that
    /// anchor's own short turns fall. Present only with enough short turns.
    own_short_turn_floor: BTreeMap<i32, f32>,
}

impl ShortTurnEvidence {
    fn measure(
        raw: &[OfflineSpeakerDiarizationSegment],
        embs: &[Option<Vec<f32>>],
        centroid: &ClusterEmbeddings,
        dur: &BTreeMap<i32, f32>,
    ) -> Self {
        let mut longest: BTreeMap<i32, f32> = BTreeMap::new();
        for s in raw {
            let d = longest.entry(s.speaker).or_insert(0.0);
            *d = d.max(s.end - s.start);
        }
        let mut own_short_turn_floor = BTreeMap::new();
        for anchor in select_anchors(centroid, dur) {
            let voice = &centroid[&anchor];
            let mut scores: Vec<f32> = raw
                .iter()
                .zip(embs)
                .filter(|(s, _)| s.speaker == anchor && s.end - s.start < SHORT_TURN_SECONDS)
                .filter_map(|(_, e)| e.as_ref().map(|e| dot(e, voice)))
                .filter(|score| score.is_finite())
                .collect();
            if let Some(floor) = low_percentile(&mut scores) {
                own_short_turn_floor.insert(anchor, floor);
            }
        }
        Self {
            longest,
            own_short_turn_floor,
        }
    }

    /// Sub-second windows carry little voice identity, however many of them a cluster
    /// accumulates. A cluster without a longest turn on record is not restricted.
    fn has_turn_of(&self, candidate: i32, seconds: f32) -> bool {
        self.longest.get(&candidate).is_none_or(|d| *d >= seconds)
    }

    /// Highest cosine to `anchor` at which `candidate` still counts as a different voice.
    fn max_cosine(&self, candidate: i32, anchor: i32) -> f32 {
        let only_short_turns = self
            .longest
            .get(&candidate)
            .is_some_and(|d| *d < SHORT_TURN_SECONDS);
        match self.own_short_turn_floor.get(&anchor) {
            Some(floor) if only_short_turns => floor.min(NEW_SPEAKER_MAX_COSINE),
            _ => NEW_SPEAKER_MAX_COSINE,
        }
    }
}

/// `SHORT_TURN_PERCENTILE` of `scores`, or `None` below `SHORT_TURN_MIN_SAMPLES`.
fn low_percentile(scores: &mut [f32]) -> Option<f32> {
    if scores.len() < SHORT_TURN_MIN_SAMPLES {
        return None;
    }
    scores.sort_by(|a, b| cmp_f32(*a, *b));
    let i = ((scores.len() - 1) as f32 * SHORT_TURN_PERCENTILE).floor() as usize;
    Some(scores[i])
}

/// Retain short clusters whose voices are not sufficiently similar to an existing anchor.
/// Original long-duration groups remain anchor candidates.
fn select_supported_anchors(
    centroid: &ClusterEmbeddings,
    dur: &BTreeMap<i32, f32>,
    evidence: &ShortTurnEvidence,
) -> Vec<i32> {
    let mut anchors: Vec<_> = select_anchors(centroid, dur)
        .into_iter()
        .filter(|id| valid_embedding(&centroid[id]))
        .collect();
    let mut candidates: Vec<_> = centroid
        .iter()
        .filter(|(_, v)| valid_embedding(v))
        .map(|(id, _)| *id)
        .collect();
    candidates.sort_by(|a, b| cmp_f32(dur[b], dur[a]).then(a.cmp(b)));
    for candidate in candidates {
        if !anchors.contains(&candidate)
            && dur[&candidate] >= NEW_SPEAKER_MIN_SECONDS
            && evidence.has_turn_of(candidate, NEW_SPEAKER_MIN_SECONDS)
            && !anchors.iter().any(|anchor| {
                dot(&centroid[&candidate], &centroid[anchor])
                    >= evidence.max_cosine(candidate, *anchor)
            })
        {
            anchors.push(candidate);
        }
    }
    anchors
}

fn supported_assignment(
    emb: &[f32],
    original: i32,
    anchors: &[i32],
    centroids: &ClusterEmbeddings,
) -> i32 {
    nearest_anchor(emb, anchors, centroids)
        .filter(|nearest| dot(emb, &centroids[nearest]) >= MIN_MERGE_COSINE)
        // Short or noisy utterances must not resurrect discarded raw labels.
        // Their aggregate cluster voice is more stable than one weak embedding.
        .or_else(|| {
            centroids
                .get(&original)
                .and_then(|voice| nearest_anchor(voice, anchors, centroids))
        })
        .unwrap_or(original)
}

/// `emb`（L2 正規化済み）に最も近い anchor を cosine（= 内積）で選ぶ。
fn nearest_anchor(emb: &[f32], anchors: &[i32], centroid: &BTreeMap<i32, Vec<f32>>) -> Option<i32> {
    anchors
        .iter()
        .filter_map(|c| centroid.get(c).map(|v| (*c, dot(emb, v))))
        .filter(|(_, score)| score.is_finite())
        .max_by(|a, b| cmp_f32(a.1, b.1))
        .map(|(c, _)| c)
}

/// turn 音声から TitaNet 埋め込み（L2 正規化）を取る。短い turn は窓を中心に広げ、
/// 長い turn は `EMBED_MAX_SECONDS` へ中央で切り詰める（TitaNet の 122.88s 上限対策）。
fn embed_segment(
    ext: &SpeakerEmbeddingExtractor,
    pcm: &[f32],
    sr: u32,
    start_s: f32,
    end_s: f32,
) -> Option<Vec<f32>> {
    let (a, b) = embed_window(start_s, end_s, sr, pcm.len())?;
    // turn 単位の例外シールド: モデル起因の C++ 例外をこの turn の None に留め、
    // 外側 guard の Err（＝話者分離ジョブ全体の失敗）へ波及させない。
    let emb = crate::ffi_guard::guard("話者埋め込み (TitaNet)", || {
        let stream = ext.create_stream()?;
        stream.accept_waveform(sr as i32, &pcm[a..b]);
        stream.input_finished();
        if !ext.is_ready(&stream) {
            return None;
        }
        ext.compute(&stream)
    })
    .ok()
    .flatten();
    let mut emb = emb?;
    l2_normalize(&mut emb);
    valid_embedding(&emb).then_some(emb)
}

/// 埋め込み用の PCM 窓 `[a, b)` を決める純関数。`EMBED_MIN_SECONDS` 未満は中心から広げ、
/// `EMBED_MAX_SECONDS` 超は中心へ切り詰め、最後に `[0, n]` へクランプする。
fn embed_window(start_s: f32, end_s: f32, sr: u32, n: usize) -> Option<(usize, usize)> {
    let n = n as isize;
    let srf = sr as f32;
    let mut a = (start_s * srf) as isize;
    let mut b = (end_s * srf) as isize;
    let min_len = (EMBED_MIN_SECONDS * srf) as isize;
    let max_len = (EMBED_MAX_SECONDS * srf) as isize;
    if b - a < min_len {
        let center = (a + b) / 2;
        a = center - min_len / 2;
        b = a + min_len;
    } else if b - a > max_len {
        let center = (a + b) / 2;
        a = center - max_len / 2;
        b = a + max_len;
    }
    let a = a.clamp(0, n) as usize;
    let b = b.clamp(0, n) as usize;
    (b > a).then_some((a, b))
}

fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

fn valid_embedding(vector: &[f32]) -> bool {
    !vector.is_empty() && vector.iter().all(|v| v.is_finite()) && vector.iter().any(|v| *v != 0.0)
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// f32 の全順序比較（尺/類似度/時刻の整列に使う）。値は有限前提のため NaN は想定外＝panic。
fn cmp_f32(a: f32, b: f32) -> std::cmp::Ordering {
    a.partial_cmp(&b)
        .expect("finite f32 in diarization ordering")
}

/// 再割当後の turn を `DiarizationResult` へ。anchor を尺降順に S1.. へ採番し、隣接同話者を結合。
/// 既定ラベルは `lang` に追従（ja「話者N」/ en「Speaker N」。フロント speakerLabelFromId と一致）。
fn build_result(
    mut turns: Vec<Reassigned>,
    centroids: BTreeMap<i32, Vec<f32>>,
    lang: Lang,
) -> DiarizationResult {
    // 再割当後の尺で anchor を採番（S1 = 最も喋った話者）。
    let mut dur: BTreeMap<i32, f32> = BTreeMap::new();
    for t in &turns {
        *dur.entry(t.label).or_insert(0.0) += t.end - t.start;
    }
    let mut order: Vec<i32> = dur.keys().copied().collect();
    order.sort_by(|a, b| cmp_f32(dur[b], dur[a]));
    let id_of: BTreeMap<i32, String> = order
        .iter()
        .enumerate()
        .map(|(i, c)| (*c, format!("S{}", i + 1)))
        .collect();

    let speakers: Vec<Speaker> = order
        .iter()
        .enumerate()
        .map(|(i, c)| Speaker {
            id: id_of[c].clone(),
            label: default_speaker_label(i + 1, lang),
            display_name: None,
        })
        .collect();

    // 話者ごとの声紋を S-id 空間へ写す（ADR-0018）。centroid を持たない話者は欠落。
    // duration_ms は再割当後の尺（最小エンロールゲート用）。
    let embeddings: Vec<SpeakerEmbedding> = order
        .iter()
        .filter_map(|c| {
            centroids.get(c).map(|v| SpeakerEmbedding {
                speaker_id: id_of[c].clone(),
                vector: v.clone(),
                duration_ms: (dur[c] * 1000.0).round() as u64,
            })
        })
        .collect();

    // 時間順に整列し、隣接する同話者ターンを結合してノイズを減らす。
    turns.sort_by(|a, b| cmp_f32(a.start, b.start));
    let mut merged: Vec<SpeakerTurn> = Vec::new();
    for t in turns {
        let sid = id_of[&t.label].clone();
        let start_ms = (t.start * 1000.0).round() as u64;
        let end_ms = (t.end * 1000.0).round() as u64;
        if let Some(last) = merged.last_mut() {
            if last.speaker_id == sid && start_ms <= last.end_ms + 200 {
                last.end_ms = end_ms.max(last.end_ms);
                continue;
            }
        }
        merged.push(SpeakerTurn {
            start_ms,
            end_ms,
            speaker_id: sid,
        });
    }

    DiarizationResult {
        speakers,
        turns: merged,
        embeddings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spk(id: &str, name: Option<&str>) -> Speaker {
        Speaker {
            id: id.into(),
            label: format!("話者{id}"),
            display_name: name.map(Into::into),
        }
    }

    #[test]
    fn carry_display_names_matches_by_voiceprint() {
        // 旧: S1=[1,0]（田中）, S2=[0,1]（改名なし）。新: N1=[0,1], N2=[1,0]（順序入替）。
        // 声紋一致で N2←S1（田中）、N1←S2（None）。
        let old = vec![
            (spk("S1", Some("田中")), vec![1.0, 0.0]),
            (spk("S2", None), vec![0.0, 1.0]),
        ];
        let new = vec![
            (spk("N1", None), vec![0.0, 1.0]),
            (spk("N2", None), vec![1.0, 0.0]),
        ];
        let out = carry_display_names(&old, &new, 0.7);
        let n1 = out.iter().find(|(id, _)| id == "N1").unwrap();
        let n2 = out.iter().find(|(id, _)| id == "N2").unwrap();
        assert_eq!(n2.1.as_deref(), Some("田中")); // 声紋一致で引き継ぎ
        assert!(n1.1.is_none()); // 旧 S2 は未改名
    }

    #[test]
    fn carry_display_names_drops_below_threshold_and_handles_count_change() {
        // 旧 1 人（田中）、新 2 人。片方だけ一致、もう片方は min_cos 未満で引き継がない。
        let old = vec![(spk("S1", Some("田中")), vec![1.0, 0.0])];
        let new = vec![
            (spk("N1", None), vec![0.99, 0.14]),
            (spk("N2", None), vec![0.0, 1.0]),
        ];
        let out = carry_display_names(&old, &new, 0.9);
        assert_eq!(out.len(), 2);
        assert_eq!(
            out.iter().find(|(id, _)| id == "N1").unwrap().1.as_deref(),
            Some("田中")
        );
        assert!(out.iter().find(|(id, _)| id == "N2").unwrap().1.is_none());
        // 空入力は空を返す。
        assert!(carry_display_names(&[], &[], 0.5).is_empty());
    }

    #[test]
    fn build_result_exposes_per_speaker_embeddings() {
        // anchor 0（尺大）と anchor 1。各 centroid を付与 → S-id へ正しく写るか。
        let turns = vec![
            Reassigned {
                start: 0.0,
                end: 10.0,
                label: 0,
            },
            Reassigned {
                start: 10.0,
                end: 14.0,
                label: 1,
            },
        ];
        let mut centroids = BTreeMap::new();
        centroids.insert(0, vec![1.0, 0.0]);
        centroids.insert(1, vec![0.0, 1.0]);

        let r = build_result(turns, centroids, Lang::Ja);

        // S1 = 最も喋った anchor 0。
        assert_eq!(r.speakers[0].id, "S1");
        assert_eq!(r.speakers[0].label, "話者1");
        assert_eq!(r.embeddings.len(), 2);
        let s1 = r.embeddings.iter().find(|e| e.speaker_id == "S1").unwrap();
        assert_eq!(s1.vector, vec![1.0, 0.0]);
        assert_eq!(s1.duration_ms, 10_000); // anchor 0 = 10s
        let s2 = r.embeddings.iter().find(|e| e.speaker_id == "S2").unwrap();
        assert_eq!(s2.vector, vec![0.0, 1.0]);
        assert_eq!(s2.duration_ms, 4_000); // anchor 1 = 4s
    }

    /// en の既定ラベルは「Speaker N」（フロント speakerLabelFromId と一致）。
    #[test]
    fn build_result_labels_follow_lang() {
        let turns = vec![
            Reassigned {
                start: 0.0,
                end: 10.0,
                label: 0,
            },
            Reassigned {
                start: 10.0,
                end: 14.0,
                label: 1,
            },
        ];
        let r = build_result(turns, BTreeMap::new(), Lang::En);
        assert_eq!(r.speakers[0].label, "Speaker 1");
        assert_eq!(r.speakers[1].label, "Speaker 2");
    }

    /// 全 turn の埋め込みが失敗すると centroid は空になる。その時 anchor は 1 つも選べず、
    /// consolidate は空へ graceful degrade する（この空判定が無いと `anchors[0]` が index
    /// panic → 文字起こしごと喪失する severe な失敗モード）。
    #[test]
    fn select_anchors_empty_when_no_centroids() {
        let centroid: BTreeMap<i32, Vec<f32>> = BTreeMap::new();
        let mut dur: BTreeMap<i32, f32> = BTreeMap::new();
        dur.insert(0, 42.0); // 尺はあるが centroid が無い（＝全 turn 埋め込み失敗）
        dur.insert(1, 8.0);
        assert!(select_anchors(&centroid, &dur).is_empty());
    }

    /// floor 未満しか無くても centroid があれば最大尺 1 つを anchor に採る（挙動保存）。
    #[test]
    fn select_anchors_falls_back_to_largest_when_all_below_floor() {
        let mut centroid: BTreeMap<i32, Vec<f32>> = BTreeMap::new();
        centroid.insert(0, vec![1.0, 0.0]);
        centroid.insert(1, vec![0.0, 1.0]);
        let mut dur: BTreeMap<i32, f32> = BTreeMap::new();
        dur.insert(0, 1.0); // 両方 ANCHOR_MIN_SECONDS(15s) 未満
        dur.insert(1, 3.0);
        assert_eq!(select_anchors(&centroid, &dur), vec![1]); // 最大尺の 1 のみ
    }

    #[test]
    fn invalid_short_centroids_do_not_poison_valid_anchors() {
        let voices = BTreeMap::from([
            (0, vec![1.0, 0.0]),
            (1, vec![f32::NAN, 0.0]),
            (2, vec![0.0, 0.0]),
        ]);
        let durations = BTreeMap::from([(0, 60.0), (1, 1.0), (2, 1.0)]);
        let anchors = select_supported_anchors(&voices, &durations, &ShortTurnEvidence::default());
        assert_eq!(anchors, vec![0]);
        assert_eq!(supported_assignment(&[1.0, 0.0], 0, &anchors, &voices), 0);
        assert_eq!(nearest_anchor(&[1.0, 0.0], &[0, 1], &voices), Some(0));
    }

    #[test]
    fn short_distinct_voices_survive_duration_cleanup() {
        let voices = BTreeMap::from([(0, vec![1.0, 0.0]), (1, vec![0.0, 1.0])]);
        for durations in [
            BTreeMap::from([(0, 60.0), (1, 10.0)]),
            BTreeMap::from([(0, 600.0), (1, 30.0)]),
            BTreeMap::from([(0, 3.0), (1, 1.0)]),
        ] {
            let anchors =
                select_supported_anchors(&voices, &durations, &ShortTurnEvidence::default());
            assert_eq!(anchors.len(), 2);
            assert_eq!(supported_assignment(&voices[&1], 1, &anchors, &voices), 1);
        }
    }

    #[test]
    fn short_voice_variations_do_not_create_extra_speakers() {
        let voices = BTreeMap::from([
            (0, vec![1.0, 0.0, 0.0]),
            (1, vec![0.48, 0.8772685, 0.0]),
            (2, vec![0.43, 0.0, 0.9028289]),
            (3, vec![0.2, 0.0, -0.9797959]),
        ]);
        let durations = BTreeMap::from([(0, 60.0), (1, 2.4), (2, 1.7), (3, 0.6)]);
        let anchors = select_supported_anchors(&voices, &durations, &ShortTurnEvidence::default());
        assert_eq!(anchors, vec![0]);
        for (id, voice) in &voices {
            assert_eq!(supported_assignment(voice, *id, &anchors, &voices), 0);
        }
    }

    #[test]
    fn clearly_distinct_brief_voice_survives_without_splitting_established_voice() {
        let voices = BTreeMap::from([
            (0, vec![1.0, 0.0, 0.0]),
            (1, vec![0.48, 0.8772685, 0.0]),
            (2, vec![0.0, 0.0, 1.0]),
        ]);
        let durations = BTreeMap::from([(0, 60.0), (1, 2.4), (2, 2.5)]);
        let anchors = select_supported_anchors(&voices, &durations, &ShortTurnEvidence::default());
        assert_eq!(anchors, vec![0, 2]);
        assert_eq!(supported_assignment(&voices[&1], 1, &anchors, &voices), 0);
        assert_eq!(supported_assignment(&voices[&2], 2, &anchors, &voices), 2);
    }

    /// Anchor 0's own short turns sometimes score as low as 0.1 against its centroid,
    /// so a candidate made of short turns at 0.2 is not evidence of another voice.
    fn one_on_one_evidence(candidate_longest: f32) -> ShortTurnEvidence {
        ShortTurnEvidence {
            longest: BTreeMap::from([(0, 40.0), (1, candidate_longest)]),
            own_short_turn_floor: BTreeMap::from([(0, 0.1)]),
        }
    }

    #[test]
    fn short_turn_fragments_merge_when_the_anchor_itself_scores_that_low() {
        let voices = BTreeMap::from([(0, vec![1.0, 0.0]), (1, vec![0.2, 0.9797959])]);
        let durations = BTreeMap::from([(0, 900.0), (1, 2.5)]);
        let anchors = select_supported_anchors(&voices, &durations, &one_on_one_evidence(1.9));
        assert_eq!(anchors, vec![0]);
        assert_eq!(supported_assignment(&voices[&1], 1, &anchors, &voices), 0);
    }

    #[test]
    fn candidate_with_a_long_turn_keeps_the_fixed_threshold() {
        let voices = BTreeMap::from([(0, vec![1.0, 0.0]), (1, vec![0.2, 0.9797959])]);
        let durations = BTreeMap::from([(0, 900.0), (1, 2.5)]);
        let anchors = select_supported_anchors(&voices, &durations, &one_on_one_evidence(2.5));
        assert_eq!(anchors, vec![0, 1]);
    }

    #[test]
    fn short_candidate_clearly_below_the_anchor_floor_survives() {
        let voices = BTreeMap::from([(0, vec![1.0, 0.0]), (1, vec![0.05, 0.998_749_2])]);
        let durations = BTreeMap::from([(0, 900.0), (1, 1.3)]);
        let anchors = select_supported_anchors(&voices, &durations, &one_on_one_evidence(1.3));
        assert_eq!(anchors, vec![0, 1]);
    }

    #[test]
    fn accumulated_subsecond_turns_do_not_create_a_speaker() {
        let voices = BTreeMap::from([(0, vec![1.0, 0.0]), (1, vec![0.0, 1.0])]);
        let durations = BTreeMap::from([(0, 900.0), (1, 1.6)]);
        let anchors = select_supported_anchors(&voices, &durations, &one_on_one_evidence(0.5));
        assert_eq!(anchors, vec![0]);
    }

    #[test]
    fn few_short_turns_do_not_set_a_floor() {
        let mut scores = vec![0.1; SHORT_TURN_MIN_SAMPLES - 1];
        assert_eq!(low_percentile(&mut scores), None);
        let mut scores: Vec<f32> = (0..21).map(|i| i as f32 / 20.0).collect();
        scores.reverse();
        assert_eq!(low_percentile(&mut scores), Some(0.05));
    }

    #[test]
    fn weak_turn_does_not_resurrect_a_merged_fragment() {
        let voices = BTreeMap::from([(0, vec![1.0, 0.0]), (1, vec![0.8, 0.6])]);
        let durations = BTreeMap::from([(0, 60.0), (1, 2.0)]);
        let anchors = select_supported_anchors(&voices, &durations, &ShortTurnEvidence::default());
        assert_eq!(anchors, vec![0]);
        // The cluster has a supported identity even when this short turn does not.
        assert_eq!(supported_assignment(&[0.1, 0.9949874], 1, &anchors, &voices), 0);
    }

    #[test]
    fn similar_short_fragments_merge_but_unsupported_turns_keep_their_group() {
        let voices = BTreeMap::from([(0, vec![1.0, 0.0]), (1, vec![0.8, 0.6])]);
        let durations = BTreeMap::from([(0, 60.0), (1, 2.0)]);
        let anchors = select_supported_anchors(&voices, &durations, &ShortTurnEvidence::default());
        assert_eq!(anchors, vec![0]);
        assert_eq!(supported_assignment(&voices[&1], 1, &anchors, &voices), 0);
        assert_eq!(supported_assignment(&[0.0, 1.0], 2, &anchors, &voices), 2);
    }

    /// 普通の尺の turn は窓をそのまま使う。
    #[test]
    fn embed_window_passthrough_for_normal_turn() {
        let n = 16000 * 600; // 10 分の PCM
        assert_eq!(embed_window(10.0, 40.0, 16000, n), Some((160_000, 640_000)));
    }

    /// 短い turn は EMBED_MIN_SECONDS まで中心から広げる。
    #[test]
    fn embed_window_widens_short_turn() {
        let n = 16000 * 600;
        let (a, b) = embed_window(100.0, 100.1, 16000, n).unwrap();
        assert_eq!(b - a, (EMBED_MIN_SECONDS * 16000.0) as usize);
    }

    /// 長い turn は EMBED_MAX_SECONDS へ中央で切り詰める（TitaNet の 122.88s 上限対策）。
    /// 173.6s の独演 turn が実会議で onnxruntime の broadcast 例外を起こした。
    #[test]
    fn embed_window_caps_long_turn_at_max() {
        let n = 16000 * 600;
        let (a, b) = embed_window(10.0, 183.6, 16000, n).unwrap();
        assert_eq!(b - a, (EMBED_MAX_SECONDS * 16000.0) as usize);
        // 中央 (10+183.6)/2 = 96.8s を挟んで前後 60s。
        assert_eq!(a, ((96.8 - 60.0) * 16000.0) as usize);
    }

    /// PCM 範囲外へはみ出す窓はクランプされ、空になれば None。
    #[test]
    fn embed_window_clamps_and_rejects_empty() {
        assert!(embed_window(5.0, 6.0, 16000, 0).is_none()); // 空 PCM
        let (a, b) = embed_window(-1.0, 0.2, 16000, 16000).unwrap(); // 頭で min 広げ→クランプ
        assert!(a < b && b <= 16000);
    }

    /// floor を越えるクラスタは尺降順で並ぶ（anchors[0] = 最も喋った話者）。
    #[test]
    fn select_anchors_sorted_by_duration_desc() {
        let mut centroid: BTreeMap<i32, Vec<f32>> = BTreeMap::new();
        centroid.insert(0, vec![1.0, 0.0]);
        centroid.insert(1, vec![0.0, 1.0]);
        let mut dur: BTreeMap<i32, f32> = BTreeMap::new();
        dur.insert(0, 20.0);
        dur.insert(1, 40.0);
        assert_eq!(select_anchors(&centroid, &dur), vec![1, 0]); // 40s の 1 が先頭
    }
}

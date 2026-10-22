//! iCal (RFC5545) の解析と繰り返し予定（RRULE）の展開。
//!
//! `mod.rs`（fetch / 公開 API）から分離した**純粋層**: ネットワーク I/O を持たず、
//! iCal 本文テキスト → 内部表現（`RawEvent`）→ 表示用 `CalendarEvent` への変換だけを担う。
//! 壁時計展開・DAILY/WEEKLY 限定などの設計背景と MVP 制限は親モジュール `mod.rs` の doc を参照。
//!
//! 内部表現（`DtValue` / `Freq` / `RRule` / `RawEvent`）はパース関数と展開関数の**両方**が共有する
//! ため、両者を 1 ファイルに同居させる（パースだけ・展開だけへ切り出すと型を跨いで公開する必要が出る）。

use super::{CalendarEvent, WALL_FMT};
use chrono::{
    DateTime, Datelike, Days, Duration, FixedOffset, NaiveDate, NaiveDateTime, Utc, Weekday,
};
use std::collections::{HashMap, HashSet};

/// RRULE 展開の反復ハード上限（無限ループ・古い DAILY 予定への保険）。
const MAX_RRULE_ITER: usize = 100_000;

// ───────────────────────── パース（内部表現） ─────────────────────────

/// iCal の日時値（3 形態）。
#[derive(Debug, Clone, PartialEq)]
enum DtValue {
    /// 全日（VALUE=DATE）。一覧からは除外する。
    Date(NaiveDate),
    /// 浮動 / TZID 付き（壁時計をそのままローカル扱い）。
    Floating(NaiveDateTime),
    /// UTC（末尾 `Z`）。注入オフセットでローカルへ変換する。
    Utc(NaiveDateTime),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Freq {
    Daily,
    Weekly,
    /// MONTHLY/YEARLY 等（MVP 未対応 → マスタ 1 回のみ）。
    Other,
}

#[derive(Debug, Clone)]
struct RRule {
    freq: Freq,
    interval: i64,
    count: Option<u32>,
    until: Option<DtValue>,
    /// WEEKLY の BYDAY（曜日）。ordinal（`1MO` 等）は無視し曜日のみ。
    byday: Vec<Weekday>,
}

#[derive(Debug, Clone, Default)]
struct RawEvent {
    uid: String,
    summary: String,
    location: Option<String>,
    start: Option<DtValue>,
    end: Option<DtValue>,
    rrule: Option<RRule>,
    exdates: Vec<DtValue>,
    /// 繰り返しの 1 発生を差し替える override（その UID のマスタ展開から当該時刻を除外する）。
    recurrence_id: Option<DtValue>,
}

/// RFC5545 の行アンフォールド: 行頭が空白/タブの行は直前行の継続（先頭 1 文字を除いて連結）。
/// CRLF/LF どちらも許容。
fn unfold_lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix([' ', '\t']) {
            if let Some(last) = out.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        out.push(line.to_string());
    }
    out
}

/// プロパティ行を (name, params, value) に分解する。
/// 値内の `:` は最初の「クォート外の `:`」で区切る（`TZID="x:y":...` のような param を守る）。
fn split_property(line: &str) -> Option<(String, String, String)> {
    let mut in_quote = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quote = !in_quote,
            ':' if !in_quote => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    // name は最初の `;` まで。残りが params。
    let (name, params) = match head.find(';') {
        Some(p) => (&head[..p], &head[p + 1..]),
        None => (head, ""),
    };
    Some((
        name.to_ascii_uppercase(),
        params.to_string(),
        value.to_string(),
    ))
}

/// TEXT 値のアンエスケープ（`\n`/`\N`→改行, `\,`→`,`, `\;`→`;`, `\\`→`\`）。
fn unescape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(',') => out.push(','),
                Some(';') => out.push(';'),
                Some('\\') => out.push('\\'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// 日時値 1 個をパース（`20260115T150000Z` / `20260115T150000` / `20260115`）。
fn parse_dt_value(value: &str) -> Option<DtValue> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    // 全日（YYYYMMDD）。
    if v.len() == 8 && v.bytes().all(|b| b.is_ascii_digit()) {
        return NaiveDate::parse_from_str(v, "%Y%m%d")
            .ok()
            .map(DtValue::Date);
    }
    if let Some(core) = v.strip_suffix('Z') {
        return NaiveDateTime::parse_from_str(core, "%Y%m%dT%H%M%S")
            .ok()
            .map(DtValue::Utc);
    }
    NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S")
        .ok()
        .map(DtValue::Floating)
}

/// EXDATE 等のカンマ区切り日時を全て返す。
fn parse_dt_list(value: &str) -> Vec<DtValue> {
    value.split(',').filter_map(parse_dt_value).collect()
}

fn parse_weekday(code: &str) -> Option<Weekday> {
    // 先頭の ordinal（`+1`/`-2` 等）を除いた末尾 2 文字が曜日コード。
    let c = code.trim();
    let day = c.get(c.len().saturating_sub(2)..)?;
    match day.to_ascii_uppercase().as_str() {
        "MO" => Some(Weekday::Mon),
        "TU" => Some(Weekday::Tue),
        "WE" => Some(Weekday::Wed),
        "TH" => Some(Weekday::Thu),
        "FR" => Some(Weekday::Fri),
        "SA" => Some(Weekday::Sat),
        "SU" => Some(Weekday::Sun),
        _ => None,
    }
}

fn parse_rrule(value: &str) -> RRule {
    let mut freq = Freq::Other;
    let mut interval = 1i64;
    let mut count = None;
    let mut until = None;
    let mut byday = Vec::new();
    for part in value.split(';') {
        let (k, v) = match part.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        match k.trim().to_ascii_uppercase().as_str() {
            "FREQ" => {
                freq = match v.trim().to_ascii_uppercase().as_str() {
                    "DAILY" => Freq::Daily,
                    "WEEKLY" => Freq::Weekly,
                    _ => Freq::Other,
                }
            }
            "INTERVAL" => {
                interval = v
                    .trim()
                    .parse::<i64>()
                    .ok()
                    .filter(|n| *n >= 1)
                    .unwrap_or(1)
            }
            "COUNT" => count = v.trim().parse::<u32>().ok(),
            "UNTIL" => until = parse_dt_value(v.trim()),
            "BYDAY" => byday = v.split(',').filter_map(parse_weekday).collect(),
            _ => {}
        }
    }
    RRule {
        freq,
        interval,
        count,
        until,
        byday,
    }
}

/// 本文 → VEVENT のリスト。
fn parse_events(text: &str) -> Vec<RawEvent> {
    let mut events = Vec::new();
    let mut cur: Option<RawEvent> = None;
    // VEVENT 内のネスト深さ（VALARM 等のサブコンポーネント）。>0 の間はプロパティを拾わない。
    // ⚠️ これが無いと VALARM の SUMMARY/LOCATION（EMAIL リマインダ等）が VEVENT のものを上書きする。
    let mut sub_depth: u32 = 0;
    for line in unfold_lines(text) {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("BEGIN:VEVENT") {
            cur = Some(RawEvent::default());
            sub_depth = 0;
            continue;
        }
        if trimmed.eq_ignore_ascii_case("END:VEVENT") {
            if let Some(ev) = cur.take() {
                events.push(ev);
            }
            sub_depth = 0;
            continue;
        }
        let ev = match cur.as_mut() {
            Some(ev) => ev,
            None => continue, // VEVENT 外（VTIMEZONE 等）は無視
        };
        // VEVENT 内のサブコンポーネント（BEGIN:VALARM … END:VALARM 等）はプロパティを拾わない。
        // バイト列で比べる（`&str` を固定長で切ると、境界が多バイト文字の途中に来たとき panic する）。
        let bytes = trimmed.as_bytes();
        if bytes
            .get(..6)
            .is_some_and(|b| b.eq_ignore_ascii_case(b"BEGIN:"))
        {
            sub_depth += 1;
            continue;
        }
        if bytes
            .get(..4)
            .is_some_and(|b| b.eq_ignore_ascii_case(b"END:"))
        {
            sub_depth = sub_depth.saturating_sub(1);
            continue;
        }
        if sub_depth > 0 {
            continue;
        }
        let (name, _params, value) = match split_property(&line) {
            Some(t) => t,
            None => continue,
        };
        match name.as_str() {
            "UID" => ev.uid = value.trim().to_string(),
            "SUMMARY" => ev.summary = unescape_text(&value),
            "LOCATION" => ev.location = Some(unescape_text(&value)),
            "DTSTART" => ev.start = parse_dt_value(&value),
            "DTEND" => ev.end = parse_dt_value(&value),
            "RRULE" => ev.rrule = Some(parse_rrule(&value)),
            "EXDATE" => ev.exdates.extend(parse_dt_list(&value)),
            "RECURRENCE-ID" => ev.recurrence_id = parse_dt_value(&value),
            _ => {}
        }
    }
    events
}

// ───────────────────────── 展開（発生生成・窓フィルタ） ─────────────────────────

/// 日時値をローカル壁時計の NaiveDateTime へ。全日(Date)は `None`（一覧から除外する印）。
fn to_local_naive(dt: &DtValue, offset: FixedOffset) -> Option<NaiveDateTime> {
    match dt {
        DtValue::Date(_) => None,
        DtValue::Floating(n) => Some(*n),
        DtValue::Utc(n) => Some(
            DateTime::<Utc>::from_naive_utc_and_offset(*n, Utc)
                .with_timezone(&offset)
                .naive_local(),
        ),
    }
}

/// DAILY/WEEKLY を窓内へ展開（壁時計）。COUNT は EXDATE 適用前の生成数に効く（RFC 準拠）。
/// 返すのは窓終端までの全発生（過去含む）。「今より先か」は呼び出し側が end で判定する。
fn expand_recurrence(
    rr: &RRule,
    start: NaiveDateTime,
    until: Option<NaiveDateTime>,
    window_end: NaiveDateTime,
    excluded: &HashSet<NaiveDateTime>,
) -> Vec<NaiveDateTime> {
    let mut out = Vec::new();
    let mut produced = 0u32;
    let mut iter = 0usize;
    let time = start.time();

    // 1 発生を処理して「続行するか」を返すクロージャ。
    let emit = |occ: NaiveDateTime, out: &mut Vec<NaiveDateTime>, produced: &mut u32| -> bool {
        if occ < start {
            return true; // 初週で DTSTART より前の BYDAY は発生ではない（カウントもしない）
        }
        if let Some(c) = rr.count {
            if *produced >= c {
                return false;
            }
        }
        if let Some(u) = until {
            if occ > u {
                return false;
            }
        }
        *produced += 1;
        if occ > window_end {
            return false; // 昇順生成なので以降は窓外
        }
        if !excluded.contains(&occ) {
            out.push(occ);
        }
        true
    };

    match rr.freq {
        Freq::Daily => {
            let mut n: u64 = 0;
            loop {
                iter += 1;
                if iter > MAX_RRULE_ITER {
                    break;
                }
                let occ = match start.checked_add_days(Days::new(n)) {
                    Some(d) => d,
                    None => break,
                };
                if !emit(occ, &mut out, &mut produced) {
                    break;
                }
                n += rr.interval as u64;
            }
        }
        Freq::Weekly => {
            let mut bydays = if rr.byday.is_empty() {
                vec![start.weekday()]
            } else {
                rr.byday.clone()
            };
            bydays.sort_by_key(|w| w.num_days_from_monday());
            bydays.dedup();
            // 週の起点（月曜）。
            let week0 = match start
                .date()
                .checked_sub_days(Days::new(start.weekday().num_days_from_monday() as u64))
            {
                Some(d) => d,
                None => return out,
            };
            let mut w: u64 = 0;
            'weeks: loop {
                iter += 1;
                if iter > MAX_RRULE_ITER {
                    break;
                }
                let monday = match week0.checked_add_days(Days::new(w * 7)) {
                    Some(d) => d,
                    None => break,
                };
                for wd in &bydays {
                    let date = match monday
                        .checked_add_days(Days::new(wd.num_days_from_monday() as u64))
                    {
                        Some(d) => d,
                        None => continue,
                    };
                    let occ = date.and_time(time);
                    if !emit(occ, &mut out, &mut produced) {
                        break 'weeks;
                    }
                }
                w += rr.interval as u64;
            }
        }
        Freq::Other => {}
    }
    out
}

/// 本文 → 表示用イベント（展開・全日除外・窓フィルタ・昇順・件数上限）。
pub(super) fn parse_and_expand(
    text: &str,
    now: DateTime<FixedOffset>,
    window_days: i64,
    max: usize,
) -> Vec<CalendarEvent> {
    let now_naive = now.naive_local();
    let offset = *now.offset();
    let window_end = now_naive + Duration::days(window_days);
    let raws = parse_events(text);

    // override（RECURRENCE-ID）の時刻を UID 別に集約 → マスタ展開から除外する。
    let mut overrides: HashMap<String, HashSet<NaiveDateTime>> = HashMap::new();
    for r in &raws {
        if let (Some(rid), false) = (&r.recurrence_id, r.uid.is_empty()) {
            if let Some(n) = to_local_naive(rid, offset) {
                overrides.entry(r.uid.clone()).or_default().insert(n);
            }
        }
    }

    let mut out: Vec<CalendarEvent> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for r in &raws {
        let start_dt = match &r.start {
            Some(s) => s,
            None => continue,
        };
        // 全日（Date）は to_local_naive が None → 一覧から除外。
        let start_local = match to_local_naive(start_dt, offset) {
            Some(n) => n,
            None => continue,
        };
        let end_local = r.end.as_ref().and_then(|e| to_local_naive(e, offset));
        let dur = end_local
            .map(|e| e - start_local)
            .filter(|d| *d > Duration::zero());

        let starts: Vec<NaiveDateTime> = if r.recurrence_id.is_some() {
            // override 自体は単発（差し替え後の時刻）。
            vec![start_local]
        } else if let Some(rr) = &r.rrule {
            match rr.freq {
                Freq::Daily | Freq::Weekly => {
                    let mut exset: HashSet<NaiveDateTime> = r
                        .exdates
                        .iter()
                        .filter_map(|e| to_local_naive(e, offset))
                        .collect();
                    if let Some(ov) = overrides.get(&r.uid) {
                        exset.extend(ov.iter().copied());
                    }
                    let until_local = rr.until.as_ref().and_then(|u| to_local_naive(u, offset));
                    expand_recurrence(rr, start_local, until_local, window_end, &exset)
                }
                // 未対応 FREQ はマスタ 1 回のみ（窓内なら出る・MVP の開示済み制限）。
                Freq::Other => vec![start_local],
            }
        } else {
            vec![start_local]
        };

        for occ in starts {
            let occ_end = match dur {
                Some(d) => occ + d,
                None => occ + Duration::hours(1), // フィルタ用の既定（表示はしない）
            };
            // 進行中 or 未来、かつ窓内のみ。
            if occ_end < now_naive || occ > window_end {
                continue;
            }
            let title = {
                let t = r.summary.trim();
                if t.is_empty() {
                    "(無題)".to_string()
                } else {
                    t.to_string()
                }
            };
            let key = if r.uid.is_empty() { &title } else { &r.uid };
            let id = format!("{}#{}", key, occ.format("%Y%m%dT%H%M%S"));
            if !seen.insert(id.clone()) {
                continue; // 同一 ID は 1 度だけ
            }
            out.push(CalendarEvent {
                id,
                title,
                start: occ.format(WALL_FMT).to_string(),
                // 元に DTEND があったときだけ終了を返す（無ければフロントは開始のみ表示）。
                end: end_local.map(|_| occ_end.format(WALL_FMT).to_string()),
                location: r.location.clone().filter(|s| !s.trim().is_empty()),
            });
        }
    }

    out.sort_by(|a, b| a.start.cmp(&b.start));
    out.truncate(max);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// JST 固定の now（テスト決定的）。
    fn jst(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(9 * 3600)
            .unwrap()
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .unwrap()
    }

    fn wrap(body: &str) -> String {
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{body}\r\nEND:VCALENDAR\r\n")
    }

    #[test]
    fn unfold_joins_continuation_lines() {
        let lines = unfold_lines("SUMMARY:長い\r\n タイトル\r\nUID:x");
        assert_eq!(lines, vec!["SUMMARY:長いタイトル", "UID:x"]);
    }

    #[test]
    fn split_property_handles_params_and_quoted_colon() {
        let (n, p, v) = split_property("DTSTART;TZID=\"Asia/Tokyo\":20260115T150000").unwrap();
        assert_eq!(n, "DTSTART");
        assert_eq!(p, "TZID=\"Asia/Tokyo\"");
        assert_eq!(v, "20260115T150000");
    }

    #[test]
    fn unescape_text_handles_iana_escapes() {
        assert_eq!(unescape_text(r"A\, B\; C\nD\\E"), "A, B; C\nD\\E");
    }

    #[test]
    fn utc_time_converts_to_local() {
        // 06:00Z → JST 15:00。
        let ics =
            wrap("BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:朝会\r\nDTSTART:20260115T060000Z\r\nEND:VEVENT");
        let ev = parse_and_expand(&ics, jst(2026, 1, 15, 0, 0), 14, 20);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].title, "朝会");
        assert_eq!(ev[0].start, "2026-01-15T15:00:00");
    }

    #[test]
    fn all_day_event_is_excluded() {
        let ics = wrap(
            "BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:休暇\r\nDTSTART;VALUE=DATE:20260116\r\nEND:VEVENT",
        );
        let ev = parse_and_expand(&ics, jst(2026, 1, 15, 0, 0), 14, 20);
        assert!(ev.is_empty());
    }

    #[test]
    fn floating_time_treated_as_local() {
        let ics = wrap(
            "BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:面談\r\nDTSTART;TZID=Asia/Tokyo:20260116T110000\r\nDTEND;TZID=Asia/Tokyo:20260116T120000\r\nLOCATION:301 室\r\nEND:VEVENT",
        );
        let ev = parse_and_expand(&ics, jst(2026, 1, 15, 0, 0), 14, 20);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].start, "2026-01-16T11:00:00");
        assert_eq!(ev[0].end.as_deref(), Some("2026-01-16T12:00:00"));
        assert_eq!(ev[0].location.as_deref(), Some("301 室"));
    }

    #[test]
    fn weekly_rrule_expands_next_occurrences() {
        // 毎週木 15:00（基準は過去）。now=木 2026-01-15 の朝 → 当日含む直近を出す。
        let ics = wrap(
            "BEGIN:VEVENT\r\nUID:w\r\nSUMMARY:週次定例\r\nDTSTART;TZID=Asia/Tokyo:20250102T150000\r\nDTEND;TZID=Asia/Tokyo:20250102T160000\r\nRRULE:FREQ=WEEKLY;BYDAY=TH\r\nEND:VEVENT",
        );
        let ev = parse_and_expand(&ics, jst(2026, 1, 15, 8, 0), 14, 20);
        // 2026-01-15(木) と 2026-01-22(木) の 2 回が窓内。
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0].start, "2026-01-15T15:00:00");
        assert_eq!(ev[1].start, "2026-01-22T15:00:00");
    }

    #[test]
    fn weekly_rrule_with_exdate_skips_instance() {
        let ics = wrap(
            "BEGIN:VEVENT\r\nUID:w\r\nSUMMARY:週次定例\r\nDTSTART;TZID=Asia/Tokyo:20250102T150000\r\nRRULE:FREQ=WEEKLY;BYDAY=TH\r\nEXDATE;TZID=Asia/Tokyo:20260115T150000\r\nEND:VEVENT",
        );
        let ev = parse_and_expand(&ics, jst(2026, 1, 15, 8, 0), 14, 20);
        // 1/15 は EXDATE → 1/22 のみ。
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].start, "2026-01-22T15:00:00");
    }

    #[test]
    fn daily_rrule_with_until_limits_range() {
        let ics = wrap(
            "BEGIN:VEVENT\r\nUID:d\r\nSUMMARY:毎日\r\nDTSTART;TZID=Asia/Tokyo:20260115T090000\r\nRRULE:FREQ=DAILY;UNTIL=20260116T235900Z\r\nEND:VEVENT",
        );
        let ev = parse_and_expand(&ics, jst(2026, 1, 15, 0, 0), 14, 20);
        // 1/15, 1/16 のみ（UNTIL で打ち止め）。
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0].start, "2026-01-15T09:00:00");
        assert_eq!(ev[1].start, "2026-01-16T09:00:00");
    }

    #[test]
    fn window_filters_past_and_far_future() {
        let ics = wrap(
            "BEGIN:VEVENT\r\nUID:p\r\nSUMMARY:過去\r\nDTSTART;TZID=Asia/Tokyo:20260101T100000\r\nDTEND;TZID=Asia/Tokyo:20260101T110000\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:f\r\nSUMMARY:遠い未来\r\nDTSTART;TZID=Asia/Tokyo:20260301T100000\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:n\r\nSUMMARY:窓内\r\nDTSTART;TZID=Asia/Tokyo:20260116T100000\r\nEND:VEVENT",
        );
        let ev = parse_and_expand(&ics, jst(2026, 1, 15, 12, 0), 14, 20);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].title, "窓内");
    }

    #[test]
    fn ongoing_event_is_included() {
        // now=10:30、10:00-11:00 の進行中ミーティングは出す。
        let ics = wrap(
            "BEGIN:VEVENT\r\nUID:o\r\nSUMMARY:進行中\r\nDTSTART;TZID=Asia/Tokyo:20260115T100000\r\nDTEND;TZID=Asia/Tokyo:20260115T110000\r\nEND:VEVENT",
        );
        let ev = parse_and_expand(&ics, jst(2026, 1, 15, 10, 30), 14, 20);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].title, "進行中");
    }

    #[test]
    fn recurrence_id_override_dedups_master() {
        // マスタ（毎週木 15:00）の 1/15 を override で 16:00 に差し替え。
        let ics = wrap(
            "BEGIN:VEVENT\r\nUID:w\r\nSUMMARY:週次定例\r\nDTSTART;TZID=Asia/Tokyo:20250102T150000\r\nRRULE:FREQ=WEEKLY;BYDAY=TH\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:w\r\nSUMMARY:週次定例(変更)\r\nRECURRENCE-ID;TZID=Asia/Tokyo:20260115T150000\r\nDTSTART;TZID=Asia/Tokyo:20260115T160000\r\nEND:VEVENT",
        );
        let ev = parse_and_expand(&ics, jst(2026, 1, 15, 8, 0), 8, 20);
        // 1/15 はマスタ 15:00 を除外し override 16:00 のみ。さらに 1/22 マスタ。
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0].start, "2026-01-15T16:00:00");
        assert_eq!(ev[0].title, "週次定例(変更)");
        assert_eq!(ev[1].start, "2026-01-22T15:00:00");
    }

    #[test]
    fn max_events_truncates() {
        let ics = wrap(
            "BEGIN:VEVENT\r\nUID:d\r\nSUMMARY:毎日\r\nDTSTART;TZID=Asia/Tokyo:20260115T090000\r\nRRULE:FREQ=DAILY\r\nEND:VEVENT",
        );
        let ev = parse_and_expand(&ics, jst(2026, 1, 15, 0, 0), 30, 3);
        assert_eq!(ev.len(), 3); // 上限 3
    }

    #[test]
    fn valarm_does_not_clobber_event_title() {
        // VEVENT 内の VALARM（EMAIL リマインダで SUMMARY/LOCATION を持つ）が
        // 予定本体のタイトル/場所を上書きしないこと。
        let ics = wrap(
            "BEGIN:VEVENT\r\nUID:a\r\nSUMMARY:本物の会議\r\nLOCATION:本物の場所\r\nDTSTART;TZID=Asia/Tokyo:20260116T100000\r\nBEGIN:VALARM\r\nACTION:EMAIL\r\nSUMMARY:リマインダ件名\r\nLOCATION:アラーム場所\r\nDESCRIPTION:通知\r\nTRIGGER:-PT10M\r\nEND:VALARM\r\nEND:VEVENT",
        );
        let ev = parse_and_expand(&ics, jst(2026, 1, 15, 0, 0), 14, 20);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].title, "本物の会議");
        assert_eq!(ev[0].location.as_deref(), Some("本物の場所"));
    }

    #[test]
    fn non_ascii_lines_do_not_panic() {
        // `UID:日本語` は 4・6 バイト目が「日」の途中に来る。BEGIN:/END: の判定で panic しないこと。
        let ics = wrap(
            "BEGIN:VEVENT\r\nUID:日本語の予定\r\nSUMMARY:定例\r\nDTSTART;TZID=Asia/Tokyo:20260116T100000\r\nEND:VEVENT",
        );
        let ev = parse_and_expand(&ics, jst(2026, 1, 15, 0, 0), 14, 20);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].title, "定例");
        assert!(ev[0].id.starts_with("日本語の予定#"));
    }
}

// Tauri コマンド: https://tauri.app/develop/calling-rust/
//
// コマンド本体はドメイン別に `commands/` 配下へ分割している（transcription / recording /
// history / speaker / export / settings）。本ファイルはアプリのセットアップとハンドラ登録
// （`run()`）だけを持つ。`mojiroku-core` は UI 非依存で、ここは OS API・State 管理・イベント
// 発行のシェル層。

mod audio;
mod commands;
mod jobs;
mod live_stt;
mod live_translation;
mod mic;
mod model_downloads;
mod oauth;
mod scheduler;
mod secrets;
mod settings;
mod system_audio;
mod translation_history;
mod tray;

use mojiroku_core::store::SqliteStore;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        // OS 情報（macOS バージョン / arch）取得。フィードバックフォームのプリフィルに使う。
        .plugin(tauri_plugin_os::init())
        // アプリ内アップデート（Tauri v2 updater）: 更新の検出/DL/インストール。
        // 更新適用後の relaunch に process プラグインを併用。詳細は docs/04_operations/updater-plan.md。
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // 会議開始の自動録音プロンプト（増分1 / ADR-0026）: 予定開始時の OS 通知。
        .plugin(tauri_plugin_notification::init())
        // メニューバー常駐（ADR-0026）: 閉じるボタンでウィンドウを**破棄せず隠す**。破棄すると
        // get_webview_window("main") が None になり、トレイ「表示」も通知クリックの前面化も死に、
        // スケジューラを載せたプロセスごと終了しかねない（＝会議開始時に通知が出ない）。隠せば
        // プロセスは生き、再表示できる。実際の終了はトレイ「終了」（app.exit）で行う。
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(|app| {
            // 履歴 DB を起動時に開く（hard-fail: DB 異常はアプリ起動を止める＝MVP の明示選択）。
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let store = SqliteStore::open(&data_dir.join("mojiroku.db"))?;
            app.manage(store);
            app.manage(mic::MicState::new());
            app.manage(system_audio::SystemAudioState::new());
            app.manage(live_stt::LiveSttState::new());
            app.manage(live_translation::LiveTranslationState::default());
            app.manage(model_downloads::ModelDownloadsState::default());
            // バックグラウンドジョブ基盤（ADR-0024）: enqueue 通知チャネルを管理し、ワーカーを起動する。
            // ワーカーは起動時に中断された running を pending へ戻し（再起動継続）、以後 pending を
            // 1 本ずつ直列処理する。キャプチャは permit を取らないので並行録音は常に開始できる。
            app.manage(jobs::JobQueue::new());
            jobs::spawn_worker(app.handle().clone());
            // 詳細ビューの音声再生（convertFileSrc → asset://）用に recordings/ を
            // assetProtocol スコープへ許可（config の enable と対）。静的 $APPDATA グロブの
            // 不確実性を避け、他コードと同じ app_data_dir を実行時に許可する。
            let rec_dir = data_dir.join("recordings");
            std::fs::create_dir_all(&rec_dir)?;
            app.asset_protocol_scope()
                .allow_directory(&rec_dir, false)?;
            // 録音 spool（ADR-0023）のクラッシュ残骸を掃除する（正常経路は stop/cancel が
            // rename/削除済み。ここに残っている = 前回異常終了の書きかけ）。best-effort。
            let _ = std::fs::remove_dir_all(rec_dir.join(".spool"));
            // メニューバートレイ常駐（増分1 / ADR-0026）。会議開始通知を出すためアプリを
            // バックグラウンドに常駐させる土台。通知権限は best-effort で要求（拒否でもアプリは動く）。
            tray::setup_tray(app.handle())?;
            // 会議開始スケジューラ（ADR-0026）: カレンダー予定の開始時刻に録音を促す通知を出す。
            // 設定 auto_record_prompt が OFF なら各 tick で何もしない（明示オプトイン）。
            app.manage(scheduler::SchedulerState::new());
            scheduler::spawn(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            model_downloads::list_model_downloads,
            model_downloads::start_model_download,
            translation_history::list_live_translations,
            live_translation::begin_live_translation,
            live_translation::end_live_translation,
            live_translation::cancel_live_translation_request,
            live_translation::translate_live_line,
            commands::transcription::health,
            commands::transcription::transcribe_file,
            commands::transcription::summarize,
            commands::transcription::generate_title,
            commands::transcription::summary_model_info,
            commands::transcription::transcription_model_info,
            commands::transcription::download_live_transcription_models,
            commands::history::list_recordings,
            commands::history::list_recording_rows,
            commands::recording::cancel_mic_recording,
            commands::recording::recording_levels,
            commands::history::search_recordings,
            commands::history::get_recording,
            commands::history::delete_recording,
            commands::history::rename_recording,
            commands::speaker::rename_speaker,
            commands::speaker::set_segment_speaker,
            commands::speaker::list_speaker_library,
            commands::speaker::add_speaker_to_library,
            commands::speaker::rename_speaker_library,
            commands::speaker::delete_speaker_library,
            commands::speaker::identify_speakers,
            commands::speaker::link_speaker,
            commands::speaker::unlink_speaker,
            commands::recording::recording_audio_src,
            commands::recording::start_mic_recording,
            commands::recording::stop_mic_recording,
            commands::recording::check_system_audio_permission,
            commands::recording::start_meeting_recording,
            commands::recording::cancel_meeting_recording,
            commands::recording::stop_meeting_recording,
            commands::jobs::transcribe_recording,
            commands::jobs::diarize_recording,
            commands::jobs::list_jobs,
            commands::jobs::cancel_job,
            commands::settings::get_settings,
            commands::settings::set_settings,
            commands::settings::set_secret,
            commands::settings::delete_secret,
            commands::settings::has_secret,
            commands::settings::export_text_file,
            commands::export::export_to_notion,
            commands::export::export_to_slack,
            commands::export::list_calendar_events,
            commands::export::oauth_connect,
            commands::export::notion_accessible_pages,
            scheduler::get_pending_meeting,
            scheduler::clear_pending_meeting,
            scheduler::resolve_meeting_title
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app_handle, event| {
            // macOS: Dock アイコン再クリック等でアプリが再アクティブ化されたら、閉じるボタンで
            // 隠したウィンドウ（ADR-0026 常駐）を再表示する。トレイ「表示」と通知クリックに加えた
            // もう一つの再表示導線＝閉じた後にドックが無反応で戸惑わせない。
            if let tauri::RunEvent::Reopen { .. } = event {
                tray::show_main(app_handle);
            }
        });
}

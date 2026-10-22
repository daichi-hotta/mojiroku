//! 設定・シークレット・テキスト書き出しコマンド。

use super::*;
use crate::settings;

/// アプリ設定を読む（無ければ既定）。シークレット（API キー）は含まない。
#[tauri::command]
pub(crate) fn get_settings(app: AppHandle) -> Result<settings::Settings, String> {
    load_settings(&app)
}

/// アプリ設定を保存（settings.json を原子的に上書き）。
#[tauri::command]
pub(crate) fn set_settings(app: AppHandle, settings: settings::Settings) -> Result<(), String> {
    let data_dir = resolve_app_data_dir(&app)?;
    std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
    settings::save(&data_dir, &settings)
}

/// webview から操作できるシークレットのスロット名（既知キーの許可リスト）。
/// 読み取りコマンドは無いので漏洩経路にはならないが、任意 name を受けると
/// OAuth トークン等の他スロットを webview から上書き/削除できてしまうため閉じる。
fn is_known_secret(name: &str) -> bool {
    name == secrets::NOTION_TOKEN_KEY
        || name == secrets::SLACK_WEBHOOK_KEY
        || name == secrets::CALENDAR_ICAL_KEY
        || name == crate::oauth::GOOGLE_ACCESS_KEY
        || name == crate::oauth::GOOGLE_REFRESH_KEY
        || name == crate::oauth::GOOGLE_EXPIRY_KEY
        || name == secrets::byok_key_name("anthropic")
        || name == secrets::byok_key_name("openai")
}

/// webview から値を書き込めるスロット（BYOK の API キーだけ）。
/// Notion / Slack / Google のトークンは Rust 側の OAuth が書く。webview から書けると、
/// 乗っ取られた webview が自分のトークンや Webhook を仕込み、次の書き出しで議事録を
/// 攻撃者の宛先へ送らせられるため、書き込みは BYOK に限る（has/delete は全スロット可）。
fn is_settable_secret(name: &str) -> bool {
    name == secrets::byok_key_name("anthropic") || name == secrets::byok_key_name("openai")
}

fn ensure_known_secret(name: &str) -> Result<(), String> {
    if is_known_secret(name) {
        Ok(())
    } else {
        Err(format!("error.secret.unknown_key: {name}"))
    }
}

/// シークレット（API キー等）をキーチェーンに保存。鍵の値は保存専用で JS へは戻さない。
/// `(async)` でスレッドプールへ逃がす（キーチェーンの許可ダイアログ中にメインスレッドを止めない）。
#[tauri::command(async)]
pub(crate) fn set_secret(name: String, value: String) -> Result<(), String> {
    if !is_settable_secret(&name) {
        return Err(format!("error.secret.unknown_key: {name}"));
    }
    secrets::set(&name, &value)
}

/// シークレットを削除（未登録でも成功）。
#[tauri::command(async)]
pub(crate) fn delete_secret(name: String) -> Result<(), String> {
    ensure_known_secret(&name)?;
    secrets::delete(&name)
}

/// シークレットが保存済みか（UI のバッジ表示用。値そのものは返さない）。
#[tauri::command(async)]
pub(crate) fn has_secret(name: String) -> Result<bool, String> {
    ensure_known_secret(&name)?;
    secrets::has(&name)
}

/// テキストをファイルへ書き出す（議事録/文字起こしのエクスポート）。
/// 保存ダイアログを **Rust 側で開く**ことで書き込み先を必ずユーザー選択パスに限定する
/// （旧 save_text_file は webview から任意パスを受けており、webview 侵害時に任意ファイル
/// 書き込みへ繋がるため廃止）。キャンセルは Ok(false)。
/// `(async)` でダイアログ待ち・大きな文字起こしの書き込みをメインスレッド外へ。
#[tauri::command(async)]
pub(crate) fn export_text_file(
    app: AppHandle,
    default_name: String,
    ext: String,
    filter_name: String,
    content: String,
) -> Result<bool, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app
        .dialog()
        .file()
        .set_file_name(&default_name)
        .add_filter(&filter_name, &[&ext])
        .blocking_save_file();
    let Some(path) = picked else {
        return Ok(false);
    };
    let path = path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, content).map_err(|e| format!("file write: {e}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webview_can_set_only_byok_keys() {
        assert!(is_settable_secret(&secrets::byok_key_name("anthropic")));
        assert!(is_settable_secret(&secrets::byok_key_name("openai")));
        for name in [
            secrets::NOTION_TOKEN_KEY,
            secrets::SLACK_WEBHOOK_KEY,
            secrets::CALENDAR_ICAL_KEY,
            crate::oauth::GOOGLE_ACCESS_KEY,
            crate::oauth::GOOGLE_REFRESH_KEY,
            crate::oauth::GOOGLE_EXPIRY_KEY,
        ] {
            assert!(!is_settable_secret(name), "{name} must not be settable");
            // 連携の解除（delete）と状態表示（has）は引き続き webview から使う。
            assert!(is_known_secret(name), "{name} must stay deletable");
        }
    }
}

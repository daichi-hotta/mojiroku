// UI 文言辞書（日本語・形の単一情報源）。
// en.ts は Dict 型（= typeof ja）に従うため、キーの欠落・構造ズレは tsc が検出する。
// 命名: 名前空間 = feature 名（sidebar/home/detail/settings/...）。可変部は関数で持つ。
// ⚠️ ここは UI 表示の言語。文字起こし・要約など「コンテンツの言語」は settings.language /
// transcribe_language（Rust 側）が決める（P4 で配線）。

const ja = {
  common: {
    save: "保存",
    cancel: "キャンセル",
    delete: "削除",
    close: "閉じる",
    open: "開く",
    retry: "再試行",
    loading: "読み込み中…",
    clickToRename: "クリックで改名",
  },

  app: {
    meetingBar: {
      recording: "会議を記録中",
      backToMeeting: "会議へ戻る",
      saving: "保存中…",
      stopAndSave: "停止して保存",
      dismiss: "バーを閉じる",
      dismissHint: "閉じる（録音は続きます）",
    },
    // 会議開始の自動録音プロンプト（ADR-0026）。
    meetingStartPrompt: {
      heading: "会議が始まりました",
      body: (title: string) => `「${title}」を録音しますか？`,
      record: "録音する",
      dismiss: "閉じる",
    },
    systemAudioDenied:
      "画面とシステムオーディオの収録を許可してください（システム設定 > プライバシーとセキュリティ）",
    longRecordingWarn:
      "録音 90 分超 — 長時間会議はメモリ使用が増えます。区切って保存すると安全です",
    autoStopAtLimit: "3 時間に達したため自動保存して停止します（長時間会議のメモリ保護）",
  },

  sidebar: {
    collapse: "サイドバーを畳む",
    expand: "サイドバーを広げる",
    newRecording: "新しい録音",
    nav: {
      meeting: "会議モード",
      home: "ホーム",
      history: "履歴",
      speakers: "話者ライブラリ",
      integrations: "連携",
      settings: "設定",
    },
    recordingDot: "録音中",
    recent: "最近",
    recentEmpty: "まだありません",
    sendFeedback: "フィードバックを送る",
  },

  home: {
    recordMicDesc: "話しながらその場で文字起こし",
    dropHint: "ここへドラッグ&ドロップでも取り込めます",
    upcoming: "これからの予定",
    recordThisMeeting: "この会議を記録",
    recent: "最近の録音",
    seeAll: "すべて見る",
    title: "新しい録音",
    subtitle: "会議もメモも、この Mac の中で文字起こしして議事録まで作ります。",
    meetingCard: {
      title: "会議を記録",
      desc: "Zoom / Google Meet などを、相手（システム音声）と自分（マイク）の両方からローカルでキャプチャ。停止すると話者分離つきで文字起こしして保存します。",
      start: "会議の録音を開始",
    },
    otherImports: "その他の取り込み",
    dropTitle: "音声ファイルをドラッグ&ドロップ",
    chooseFile: "ファイルを選択",
    recordMic: "マイクで録音",
    diarize: {
      label: "話者分離",
      title: "話者分離（誰が話したか）",
      desc: "初回は追加モデル（約 110MB）を DL。文字起こしに加えて話者分離も走るため、完了まで少し時間がかかります。",
    },
    recordOnly: {
      label: "音声だけ保存",
      title: "音声だけ保存（後から文字起こし）",
      desc: "停止/取り込み時は文字起こしせず音声だけ保存します。後で詳細画面の「文字起こしを実行」から処理できます。",
    },
    privacy: "録音も推論も Mac の中だけ。クラウドへ送信しません。",
    meetingBusy: "会議を記録中です。停止してからお試しください",
    unsupportedFile: "対応していないファイル形式です",
    audioFilterName: "音声ファイル",
  },

  history: {
    title: "履歴",
    searchPlaceholder: "履歴を全文検索…",
    clearSearch: "検索をクリア",
    filters: {
      all: "すべて",
      withSummary: "要約あり",
      withSpeakers: "話者あり",
      week: "今週",
    },
    count: (n: number) => `${n}件`,
    countMatch: (n: number) => `${n}件 一致`,
    empty: {
      noMatch: (q: string) => `『${q}』に一致する履歴はありません`,
      noneFiltered: "この条件に合う履歴はありません",
      none: "まだ履歴がありません",
      hint: "ホームで音声を文字起こしすると残ります",
    },
    state: {
      processing: "処理中",
      failed: "失敗",
      untranscribed: "未文字起こし",
      noSpeech: "発話なし",
      summarized: "要約あり",
      transcribed: "文字起こし済み",
    },
    deleted: "削除しました",
    renamed: "タイトルを変更しました",
    renameTitle: "タイトルを変更",
    generateTitle: "文字起こしからタイトルを生成",
    titleGenerated: "タイトルを生成しました",
    generateTitleCloudTitle: "クラウドでタイトルを生成しますか？",
    generateTitleCloudBody:
      "要約エンジンがクラウドに設定されているため、文字起こしをクラウドの AI に送ってタイトルを作ります。",
    generateTitleCloudConfirm: "送信して生成",
    deleteConfirmTitle: "録音を削除しますか？",
    deleteConfirmBody: (title: string) =>
      `『${title}』と、その文字起こし・要約をすべて削除します。元に戻せません。`,
  },

  // 初回のモデル準備（#112）。
  setup: {
    readyNextMeeting: "モデルの準備ができました。この会議のライブ文字起こしは表示されませんが、停止して保存すると文字起こしされます。次の会議からはライブでも表示されます。",
    whisperName: "文字起こしモデル（Whisper large-v3-turbo）",
    vadName: "無音検出モデル（Silero VAD）",
    home: {
      title: "はじめに文字起こしモデルを用意します",
      body: "文字起こしはこの Mac の中で行うため、モデルを一度だけダウンロードします。今のうちに取得しておくと、最初の録音をすぐ文字起こしでき、会議中のライブ文字起こしも使えます。",
    },
    meeting: {
      title: "ライブ文字起こしにはモデルが必要です",
      body: "モデルが無くても録音と保存はできます。会議中に文字起こしを見るには、先にダウンロードしてください。",
    },
    live: {
      title: "ライブ文字起こしはこの会議では表示されません",
      body: "文字起こしモデルがまだありません。録音は続いており、停止して保存すると文字起こしされます。今ダウンロードすると、次の会議から表示されます。",
    },
  },

  meeting: {
    translation: {
      title: "ライブ翻訳",
      preview: "プレビュー",
      target: "翻訳先",
      japanese: "日本語",
      english: "英語",
      enable: "翻訳する",
      disable: "オフにする",
      description: "ライブ文字起こしをこの Mac の中で翻訳します。翻訳し終えた行は会議と一緒に保存されます。",
      downloadHint: "メモリ 16GB 以上と 5.68GB のモデルが必要です。会議の前に設定画面でモデルを取得してください。",
      about: "説明",
      retained: "翻訳し終えた行は、画面を移っても、言語を変えても、翻訳をオフにしても残ります。",
      savedTitle: "ライブ翻訳",
      savedHint: "会議中に保存した翻訳です。最終的な文字起こしとは言い回しが異なることがあります。",
      savedEmpty: "この録音には保存されたライブ翻訳がありません。",
      saveInvalid: "翻訳を保存の形に整えられませんでした。録音は続いており、翻訳も残っています。",
      savedFailed: "保存された翻訳を読み込めませんでした。",
      historyFull: "翻訳の保存件数が上限に達しました。録音は続きます。新しい翻訳を始めるには、いったん停止して保存してください。",
      temporary: "一時的なプレビューです。オフにする・言語を変える・画面を離れる・録音を止めると消え、保存されません。",
      preparing: "翻訳を準備しています…",
      downloading: "モデルをダウンロードしています…",
      waiting: "ほかのローカル AI 処理が終わるのを待っています…",
      translating: "翻訳しています…",
      listening: "ライブ文字起こしを待っています…",
      pending: "待機中",
      unavailable: "この Mac ではライブ翻訳を使えません（メモリ 16GB 以上が必要）。録音は続きます。",
      failed: "翻訳を一時停止しました。ネットワークと空き容量を確認してから再試行してください。録音は続きます。",
      retry: "翻訳を再試行",
      skipped: "追いつくため、古い行の一部を飛ばしました。",
      results: "翻訳結果",
      tooLong: "この行は長すぎてライブでは翻訳できません。",
      rowFailed: "翻訳できませんでした。",
      rowSkipped: "追いつくため飛ばしました。",
      rowQueued: "翻訳を待っています…",
    },
    idle: {
      title: "会議を記録",
      desc: "Zoom / Google Meet などの会議を、相手（システム音声）と自分（マイク）の両方からローカルでキャプチャ。停止すると話者分離つきで文字起こしして保存します。",
      permTitle: "システム音声の許可が必要です",
      permBody:
        "システム設定 > プライバシーとセキュリティ > 画面とシステムオーディオの収録 で mojiroku を許可してから、開始してください。",
      permStart: "許可を確認して開始",
      start: "会議の録音を開始",
      privacy: "音声はこの Mac の中だけで処理 · クラウド送信なし",
      headphoneHint:
        "ヘッドホン推奨 — スピーカー再生だと相手の声がマイクに回り込み「あなた」に誤帰属することがあります。",
    },
    live: {
      title: "会議を記録中",
      subtitle: "ローカルで録音中 · 停止すると話者分離つきで文字起こしして保存します",
      meterCapturing: "システム音声＋マイク 録音中",
      micLevel: "マイク（自分）の入力",
      systemLevel: "システム音声（相手）の入力",
      silentWarning: "しばらく音を拾えていません。マイクのミュートや、会議アプリの音声出力先を確認してください。録音は続いています。",
      discard: "破棄",
      privacy: "ボットは参加していません · 音声はこのMacの中だけで処理 · クラウド送信なし",
      transcriptLabel: "ライブ文字起こし",
      draftFooter:
        "ライブ表示は下書きです（保存時に話者分離つきで作り直します）。ヘッドホン推奨 — スピーカー再生だと相手の声がマイクに回り込み「あなた」に誤帰属することがあります。",
      warmupTitle: "システム音声＋マイクを録音中",
      warmupHint: "話し始めるとライブ文字起こしがここに表示されます",
      aiNotesLabel: "ライブ AI ノート",
      aiNotesAfterStop: "停止後に作成",
      aiNotesDetail:
        "保存後の詳細画面で、AI議事録・要約・アクションアイテムをローカルで作成できます。",
    },
    discardConfirm: {
      title: "この録音を破棄しますか？",
      body: "ここまでの会議音声は保存されず破棄されます。元に戻せません。",
      confirm: "破棄する",
    },
  },

  speakers: {
    title: "話者ライブラリ",
    privacyNote: "声の特徴（ベクトル）だけを端末内に保存。音声そのものは保存しません。",
    addPlaceholder: "人物名を入力して登録（例: 田中さん）",
    add: "登録",
    registered: "登録済みの話者",
    empty:
      "まだ登録された話者はいません。上で登録するか、録音の詳細画面で話者をライブラリに対応づけてください。",
    identifiedCount: (n: number) => `${n} 件で識別`,
    cancelDelete: "取消",
    footer: "録音の詳細画面で、その録音の話者をここに登録した人物へ対応づけられます。",
  },

  // バックグラウンドジョブ（ADR-0024）。詳細ビューの処理中表示・完了/失敗トースト。
  job: {
    canceling: "中断しています…",
    canceledToast: "処理を中断しました",
    cancelConfirmTitle: "処理を中断しますか？",
    cancelConfirmBody: "途中までの結果は保存されません。録音はそのまま残り、あとでやり直せます。",
    cancelConfirm: "中断する",
    processing: "ローカルで処理中",
    queued: "順番待ち（他の処理の完了を待っています）",
    elapsed: "経過",
    remaining: (min: number) => (min <= 1 ? "残り約1分" : `残り約${min}分`),
    cancel: "中断",
    transcribeCompleted: "文字起こしが完了しました",
    diarizeCompleted: "話者分離が完了しました",
    failedToast: "処理に失敗しました",
    // stage キー → 表示名（core の on_progress の stage 名と一致。未知キーは stageLabel でフォールバック）。
    stages: {
      queued: "順番待ち",
      download: "モデルをダウンロード",
      download_llm: "モデルをダウンロード",
      decode: "音声をデコード",
      transcribe: "文字起こし",
      diarization: "話者を分離",
      merge: "話者を対応付け",
    } as Record<string, string>,
  },

  ui: {
    /** ConfirmDialog の既定の確認ボタン（削除など取り消し不可な操作）。 */
    confirmDelete: "削除する",
  },

  composite: {
    translated: "訳",
    /** 発言単位の話者訂正（Issue #19）。 */
    speakerUnknown: "?",
    clickToFixSpeaker: "クリックで話者を訂正",
    playFromHere: "ここから再生",
    fixSpeakerHeading: "この発言の話者",
    fixSpeakerToUnknown: "話者不明に戻す",
    speakerFixed: "話者を訂正しました",
    speakerUnchanged: "すでにその話者です",
    valueProps: {
      local: { title: "ローカル完結", body: "録音も推論も Mac の中だけ。送信なし。" },
      free: { title: "基本無料", body: "ローカル推論で API 不要。維持費 $0。" },
      speakers: { title: "話者つき議事録", body: "誰が話したかつきで要約・議事録に。" },
    },
    localStatus: "ローカル推論 · Metal · 無料",
    previewTag: "プレビュー",
    previewTagTitle:
      "この画面はデザインプレビュー（モックデータ）。バックエンドは未実装です。",
  },

  recording: {
    micLevel: "マイクの入力",
    silentWarning: "マイクの音を拾えていません。マイクがミュートされていないか、入力装置が正しいか確認してください。",
    discard: "破棄",
    discarded: "録音を破棄しました",
    discardConfirmTitle: "この録音を破棄しますか？",
    discardConfirmBody: "録った音声は保存されず、元に戻せません。",
    status: "録音中 · 自動で文字起こしします",
    statusRecordOnly: "録音中 · 音声だけ保存します",
    stopAndTranscribe: "停止して文字起こし",
    stopAndSaveOnly: "停止して音声だけ保存",
    diarizeOn: "話者分離あり",
    diarizeOff: "話者分離なし",
    footer: "すべて Mac の中で処理 · 送信なし",
    recordOnlyHint: "音声だけ保存 · あとで詳細画面から文字起こし",
  },

  update: {
    newVersion: (version: string) => `新しいバージョン ${version} があります`,
    failed: "更新に失敗しました",
    updating: "更新中…",
    updateNow: "今すぐ更新して再起動",
    later: "後で",
  },

  detail: {
    loadFailed: "録音を表示できませんでした",
    backToHistory: "履歴に戻る",
    noAudio: "この録音には音声がありません",
    // 詳細画面の UI ラベル（lib/templates.ts のエクスポート出力見出しとは別物）。
    templateNames: {
      minutes: "AI議事録",
      summary: "要約",
      action_items: "アクションアイテム",
    } as Record<string, string>,
    summaryCardLabel: (name: string) => `${name} · ローカル生成`,
    regenerate: "再生成",
    createMinutesCta: "AI議事録を作成",
    createMinutesCtaDesc: "決定事項・宿題・論点つきの議事録をローカルで生成します。",
    tabs: {
      transcript: "文字起こし",
    },
    noTranscriptTitle: "まだ文字起こしされていません",
    noTranscriptHint: "上の「文字起こしを実行」で、この録音をローカルで文字起こしできます。",
    transcriptPendingTitle: "文字起こし中です",
    transcriptPendingHint: "終わるとここに表示されます。ほかの画面に移っても処理は続きます。",
    needsTranscript: "文字起こしが終わると使えます",
    needsJobDone: "処理が終わると使えます",
    // 後付け処理（ADR-0024）。
    runTranscribe: "文字起こしを実行",
    runTranscribeDesc: "この録音の音声をローカルで文字起こしします。",
    runTranscribeDiarize: "話者分離もあわせて実行",
    runDiarize: "話者分離を実行",
    runDiarizeDesc: "誰が話したかを後から解析して発話に割り当てます。",
    rerunDiarize: "話者分離をやり直す",
    rerunDiarizeDesc:
      "話者を解析し直します。付けた名前は、声が近ければ引き継ぎます。会議では自分の発言はそのままです。",
    summaryStale: "内容が古い可能性",
    summaryStaleTitle: "文字起こしまたは話者が更新されました。作り直しをおすすめします。",
    aiCreate: "AIで作成",
    createMinutes: "議事録を作成",
    createSummary: "要約（3行）",
    createActionItems: "アクションアイテム",
    mcpNote: "MCP 経由で Claude などからも参照できます",
    find: {
      open: "文字起こし内を検索",
      placeholder: "文字起こし内を検索",
      count: (i: number, n: number) => `${i} / ${n}`,
      prev: "前の一致",
      next: "次の一致",
    },
    audio: {
      play: "再生",
      pause: "一時停止",
      seek: "再生位置",
      spaceHint: "再生 / 一時停止（スペースキー）",
      follow: "再生位置に戻る",
      back5: "5 秒戻る",
      forward15: "15 秒進む",
      rate: (r: number) => `再生速度 ${r} 倍（押すと切り替え）`,
    },
    speakerPanel: {
      title: "話者",
      librarySpeaker: "登録話者",
      unlink: "解除",
      noVoiceprint: "声紋なし",
      confirmSuggestion: "このサジェストを確定",
      someoneElse: "別の人…",
      link: "対応づけ",
      registerPlaceholder: "新しい人物名で登録",
      registerAndLink: "登録して対応づけ",
    },
    templateModal: {
      titleFor: (what: string) => `${what}を作成`,
      templates: {
        minutes: { title: "議事録", desc: "決定事項・宿題・論点つき" },
        summary: { title: "要約（3行）", desc: "要点だけ短く" },
        actionItems: { title: "アクションアイテム", desc: "担当・期限つきの ToDo" },
      },
      engineCloud: (provider: string) => `クラウド / ${provider}`,
      engineLocal: "ローカル / Metal",
      cloudBadge: (provider: string) => `クラウド（${provider}）· BYOK`,
      cloudWarn: (provider: string) =>
        `要約のため、文字起こし内容が ${provider} へ送信されます。`,
      localBadge: "ローカル · 無料",
      localNote: "すべて Mac の中で処理 · 送信なし。",
      engineHint: "エンジンは 設定 → 要約エンジン で切り替えられます。",
      sendAndGenerate: (provider: string) => `${provider} に送信して作成`,
      replaceNote: (what: string) => `今の${what}を新しく作り直します。`,
      progressQueued: "他の処理（文字起こしなど）の完了を待っています…",
      progressDownload: (pct: number) => `要約モデルをDL中… ${pct}%`,
      progressGenerating: (engine: string) => `要約を生成中…（${engine}）`,
      created: "作成しました",
    },
    share: {
      /** 詳細画面右上のボタン。コピー・書き出し・送信・AI で開くをまとめたメニュー（#105）。 */
      button: "共有",
      secCopy: "コピー",
      minutesMd: "議事録（Markdown）",
      minutesMdSub: "見出し・箇条書きつき",
      summaryRow: "要約（3行）",
      transcript: "文字起こし",
      needMinutes: "議事録を作成してください",
      copied: "コピーしました",
      copyFailed: (e: string) => `コピーに失敗しました: ${e}`,
      secAi: "生成AIで開く",
      openChatGpt: "ChatGPTで開く",
      openClaude: "Claudeで開く",
      aiOpened: "プロンプトをコピーして開きました",
      aiOpenFailed: (e: string) => `AI で開けませんでした: ${e}`,
      secExport: "ファイルに書き出し",
      fmtObsidian: "Markdown（Obsidian）",
      fmtMarkdown: "Markdown",
      fmtText: "テキスト",
      fmtSrt: "SRT 字幕",
      obsidianNote: "Obsidian ノート",
      srtButton: ".srt 字幕",
      pdfButton: "PDF（印刷）",
      exported: "書き出しました",
      exportFailed: (e: string) => `書き出しに失敗しました: ${e}`,
      pdfFailed: (e: string) => `PDF を開けませんでした: ${e}`,
      exportNote:
        "Obsidian ノートは frontmatter + 要約 + 文字起こし。.md/.txt/.srt は文字起こし。PDF は印刷ダイアログから「PDF として保存」（要約 + 文字起こし）。",
      secIntegrations: "連携",
      notionTitle: "Notion に送る",
      notionSub: "要約 + 文字起こしをページとして作成",
      notionSending: "Notion に送信中…",
      notionSent: "Notion に送りました",
      notionFailed: (e: string) => `Notion 送信に失敗: ${e}`,
      slackTitle: "Slack に送る",
      slackSub: "要約を設定チャンネルへ投稿（文字起こしは送らない）",
      slackSending: "Slack に送信中…",
      slackSent: "Slack に送りました",
      slackFailed: (e: string) => `Slack 送信に失敗: ${e}`,
      integrationsNote:
        "外部サーバへ送信されます（Notion＝要約+文字起こし / Slack＝要約のみ）。設定はサイドバーの「連携」から。",
      privacyFooter:
        "コピー・書き出しはローカル · 外部送信は AI / Notion / Slack を押したときだけ",
    },
  },

  integrations: {
    title: "連携",
    intro:
      "予定や通話とつないで記録をなめらかに。取り込みはこの画面を開いたとき、書き出しはあなたがボタンを押したときだけ — 自動で外に出ることはありません。",
    previewToast: "プレビューです",
    howToTitle: "はじめ方",
    disconnectFailed: (e: string) => `解除に失敗: ${e}`,
    footer:
      "どの連携も、取り込み・書き出しはあなたの操作が起点。バックグラウンドで勝手に送受信しません。",
    connect: {
      connected: "連携済み",
      reconnect: "再連携",
      disconnect: "解除",
      waitingBrowser: "ブラウザで許可を待っています…",
    },
    calendar: {
      title: "カレンダー",
      desc: "予定からタイトルを自動入力し、ワンクリックで記録を始められます。読み取り専用・このMac内で完結します。",
      checking: "接続状態を確認中…",
      googleName: "Google カレンダー",
      connectedBadge: "連携済み · 読み取り専用",
      refresh: "更新",
      disconnect: "切断",
      upcoming: "次の予定",
      loadingEvents: "予定を取得中…",
      loadFailed: (e: string) => `予定を取得できませんでした：${e}`,
      noEvents: "直近の予定はありません（今日から2週間以内・全日予定は除く）。",
      prepare: "記録を準備",
      cacheNote:
        "新しく追加・変更した予定は、Google 側のキャッシュにより反映が数分〜数時間遅れることがあります。「更新」で再取得できます。",
      connectTitle: "Google カレンダーと連携",
      connectDesc:
        "ボタンひとつで連携できます。読み取り専用（予定の参照のみ）・このMac内で完結し、トークンはキーチェーンにのみ保存されます。",
      connectCta: "Google と連携",
      connectedToast: "Google カレンダーと連携しました",
      connectFailed: (e: string) => `連携に失敗: ${e}`,
      disconnectedToast: "カレンダーの連携を解除しました",
      step1: "「Google と連携」を押すとブラウザで Google の許可画面が開きます",
      step2: "アカウントを選び「許可」",
      step2Note: "（未確認アプリの警告が出たら「詳細」→「移動」で続行）",
      step3: "mojiroku に戻れば連携完了です",
    },
    platforms: {
      title: "会議プラットフォーム",
      desc: "通話を検知してシステム音声を取り込みます。",
      noBots: "ボットは参加しません。",
      toggleLabel: (name: string) => `${name} 連携`,
    },
    export: {
      title: "書き出し先",
      desc: "議事録を外部サービスへ書き出します。送信はあなたがボタンを押したときだけ行われます。",
      configured: "設定済み",
      pageNotSelected: "ページ未選択",
      notConfigured: "未設定",
    },
    notion: {
      desc: "ボタンひとつで連携できます。書き出し先のページを選ぶと、議事録（要約＋文字起こし）を Notion に書き出せます。",
      connectCta: "Notion と連携",
      parentLabel: "書き出し先ページ",
      parentPlaceholder: "選択してください",
      noPages:
        "共有されたページが見つかりません。「再連携」で Notion の許可画面に進み、書き出し先にしたいページを選択してください。",
      step1: "「Notion と連携」を押すとブラウザで Notion の許可画面が開きます",
      step2: "書き出し先にしたいページを選んで「アクセスを許可する」",
      step3: "mojiroku に戻り、上の「書き出し先ページ」で書き出し先を選べば完了です",
      // 開示文（<strong> を挟むため 前半 / 強調 / 後半 に分割。JSX はコンポーネント側）。
      notePre: "Notion へ送ると、",
      noteStrong: "要約と文字起こしが Notion のサーバへ送信",
      notePost:
        "されます（ローカル要約を使っていても送信されます）。トークンはこの Mac のキーチェーンにのみ保存され、太字などの装飾は除かれます。",
      noPagesToast:
        "連携しましたが共有ページがありません。再連携で書き出し先ページを選んでください。",
      connectedToast: "Notion と連携しました",
      connectFailed: (e: string) => `Notion 連携に失敗: ${e}`,
      disconnectedToast: "Notion の連携を解除しました",
    },
    slack: {
      desc: "ボタンひとつで連携できます。Slack の画面で選んだチャンネルへ要約を投稿します（文字起こしは送りません）。",
      connectCta: "Slack と連携",
      step1: "「Slack と連携」を押すとブラウザで Slack の許可画面が開きます",
      step2: "投稿先のワークスペースとチャンネルを選んで「許可する」",
      step3: "mojiroku に戻れば連携完了です",
      notePre: "Slack へ送ると、",
      noteStrong: "要約が Slack のサーバへ送信",
      notePost:
        "されます（文字起こしは送りません。ローカル要約を使っていても送信されます）。Webhook URL はこの Mac のキーチェーンにのみ保存され、太字などの装飾は Slack 記法に変換されます。",
      connectedToast: "Slack と連携しました",
      connectFailed: (e: string) => `Slack 連携に失敗: ${e}`,
      disconnectedToast: "Slack の連携を解除しました",
    },
  },

  settings: {
    title: "設定",
    nav: {
      models: "モデル",
      engine: "要約エンジン",
      privacy: "プライバシー",
      general: "一般",
    },
    loadFailed: (e: string) => `設定の読み込みに失敗: ${e}`,
    saveFailed: (e: string) => `設定の保存に失敗: ${e}`,
    models: {
      transcriptionPickerLabel: "録音・音声ファイルに使うモデル",
      liveModelMissing: "録音中の文字起こしには turbo と、声のある区間を見つけるモデル（合計最大 575 MB）が必要です。録音・音声ファイル用のモデルを変えずに、別途ダウンロードできます。取得後、新しく開始する会議の録音から使えます。",
      downloadLiveModel: "録音中の文字起こし用モデルをダウンロード",
      transcriptionPickerDesc: "large-v3 は処理に時間がかかりますが、認識精度が上がる場合があります。録音中の文字起こしには常に turbo を使います。取得済みのモデルは削除しません。",
      transcriptionWillDownload: (size: string) => `次の録音・音声ファイルの文字起こし時に ${size} をダウンロードします。`,
      desc: "すべてこの Mac に保存され、オフラインで動作します。",
      stt: "文字起こし",
      summarize: "要約",
      diarize: "話者分離",
      manage: "管理",
      fetch: "取得",
      savedBadge: "保存済み",
      onDemandBadge: "必要時にDL",
      // Summary model switch (ADR-0030). Auto = chosen from the Mac's memory and models on disk.
      pickerLabel: "要約に使うモデル",
      pickerDesc: "乗り換えると次の要約でそのモデルをダウンロードします。手元のモデルは消しません。",
      auto: (label: string) => `この Mac に合わせる（${label}）`,
      needsDownload: "要DL",
      willDownload: (size: string) => `次の要約で ${size} をダウンロードします。`,
      exceedsTier: "この Mac の搭載メモリには大きすぎ、メモリ不足で落ちることがあります。",
    },
    engine: {
      desc: "既定はローカル。品質を求めるときだけ自分の API キーでクラウドに切り替えられます。",
      local: {
        title: "ローカル",
        badge: "既定 · 無料 · 送信なし",
        desc: "同梱モデルで生成。追加費用なし・送信なし。",
      },
      cloud: {
        title: "クラウド（BYOK）",
        badge: "高品質",
        desc: "OpenAI / Anthropic を自分のキーで利用。",
      },
      provider: "プロバイダ",
      model: "モデル",
      modelEmptyHint: "（空欄で既定）",
      apiKey: "API キー",
      keySavedBadge: "保存済み",
      keySavedPlaceholder: "（保存済み · 再入力で更新）",
      keySavedToast: "API キーをキーチェーンに保存しました",
      keySaveFailed: (e: string) => `保存に失敗: ${e}`,
      keyDeletedToast: "API キーを削除しました",
      keyDeleteFailed: (e: string) => `削除に失敗: ${e}`,
      // 送信注記（<strong> を挟むため 前半 / 強調 / 後半 に分割。JSX はコンポーネント側）。
      cloudNotePre: "クラウド要約では、要約のために",
      cloudNoteStrong: (provider: string) => `文字起こし内容が${provider}へ送信`,
      cloudNotePost: "されます。API キーはこの Mac のキーチェーンにのみ保存されます。",
    },
    privacy: {
      // 長文説明（<strong> を挟むため 平文 / 強調 の交互キーに分割。JSX はコンポーネント側）。
      cloudIntro:
        "録音・文字起こしは Mac の中で処理します。外部サーバへ送信される経路は次のとおりです: (1) ",
      cloudByokStrong: "クラウド（BYOK）要約を選択中",
      cloudByokRest: (provider: string) =>
        `のため、要約のたびに文字起こし内容が ${provider} へ送信、 (2) `,
      cloudExportStrong: "連携で書き出した",
      cloudExportRest: "とき（Notion へ＝要約 + 文字起こし / Slack へ＝要約のみ）、 (3) ",
      cloudAiStrong: "生成 AI（ChatGPT / Claude）で開いた",
      cloudAiRest:
        "とき（文字起こしを含む）。BYOK 送信を止めるには要約エンジンを「ローカル」に戻してください。(2)(3) はあなたがボタンを押したときだけです。",
      localIntro:
        "録音・推論はこの Mac の中で処理します。外部サーバへ送信されるのは、あなたが次を操作したときだけです: (1) クラウド（BYOK）要約、 (2) ",
      localExportStrong: "連携で書き出し",
      localExportRest:
        "（Notion へ＝要約 + 文字起こし / Slack へ＝要約のみ。ローカル要約でも送信）、 (3) ",
      localAiStrong: "生成 AI（ChatGPT / Claude）で開く",
      localAiRest: "（文字起こしを含む）。いずれも自動では行いません。",
      storage: "録音・文字起こし・要約はこの Mac に保存されます。外部へ出るのは上に書いた操作をしたときだけで、そのとき書き出し先（Notion など）にも写しが残ります。使用状況（利用統計）は送信しません。",
    },
    general: {
      desc: "アプリ情報とフィードバック。",
      version: "バージョン",
      feedbackTitle: "フィードバックを送る",
      feedbackDesc: "ベータの感想や不具合をブラウザのフォームで。アプリ/OS 情報は自動入力されます。",
      feedbackOpened: "フィードバックフォームをブラウザで開きました",
      feedbackOpenFailed: (e: string) => `ブラウザを開けませんでした: ${e}`,
      autoRecordPrompt: {
        title: "会議開始時に録音を促す",
        desc: "カレンダーの予定が始まると通知で録音を提案します。録音は毎回あなたのクリックで開始します（カレンダー連携が必要）。",
      },
    },
    language: {
      title: "言語",
      uiLabel: "アプリの言語",
      uiDesc: "画面表示のほか、要約・話者ラベル・書き出しの見出しにも使われます",
      transcribeLabel: "文字起こしの言語",
      transcribeDesc: "音声認識に使う言語。会議の言語が決まっているなら指定が最も正確です",
      auto: "自動判定",
      autoJaEn: "自動判定（日本語 / 英語）",
      mixed: "日本語と英語が混ざる（遅め）",
      mixedDesc: "話の区切りごとに言語を判定し直します。間を置かずに切り替わると取りこぼすことがあります。",
      names: { ja: "日本語", en: "English" } as Record<"ja" | "en", string>,
    },
  },

  // 非 React 純関数（lib/types・share・print・templates）が dicts[lang] 経由で参照する
  // 汎用整形文言。UI コンポーネントは従来どおり useI18n() の t を使う。
  format: {
    /** speaker_id "S1" → 既定ラベル。 */
    speakerLabel: (n: string) => `話者${n}`,
    durationMin: (m: number) => `${m}分`,
    durationHour: (h: number) => `${h}時間`,
    durationHourMin: (h: number, m: number) => `${h}時間${m}分`,
    eventToday: (time: string) => `今日 ${time}`,
    eventTomorrow: (time: string) => `明日 ${time}`,
    weekdays: ["日", "月", "火", "水", "木", "金", "土"],
    /** 話者一覧など列挙の区切り。 */
    listSeparator: "、",
    /** タイトル未設定の録音の表示名（#105）。種類と日時で見分けられるようにする。 */
    untitledTitle: (kind: string, when: string) => `${kind}（${when}）`,
    sourceKind: { file: "音声ファイル", mic: "マイク録音", live: "会議" } as Record<"file" | "mic" | "live", string>,
  },

  // ⚠️ エクスポート/印刷/AI プロンプトの**出力**文言（アプリ UI ラベルとは別物）。
  // templateLabels は Rust の export::template_label と両言語とも一致必須
  // （ズレると Notion/Slack 追記との整合が壊れる。両側のテストで固定）。
  output: {
    /** title 未設定時の既定タイトル。 */
    fallbackTitle: "会議",
    transcriptHeading: "文字起こし",
    aiPromptHead: (title: string) =>
      `以下は「${title}」の文字起こしです。要点・決定事項・アクションアイテム（担当つき）を日本語でまとめてください。\n\n`,
    templateLabels: {
      minutes: "議事録",
      summary: "要約",
      action_items: "アクションアイテム",
    },
    /** 未知テンプレ id の出力見出し（Rust 側フォールバックと一致）。 */
    templateFallback: "メモ",
  },

  // Rust コマンドの Err キー（"error.<domain>.<cause>[: detail]"）→ 表示文言。
  // translateError（i18n/index.tsx）が参照する。未知キーは原文フォールバックされるので、
  // ここに無いキーを Rust 側が返しても壊れない（生キーがそのまま出るだけ）。
  // ja/en のキー一致は translateError.test.ts が検証する（Record 型のため tsc では検出できない）。
  errors: {
    "error.mic.busy": "すでに録音中です",
    "error.mic.no_input_device": "マイク（入力デバイス）が見つかりません",
    "error.mic.input_config": "マイクの入力設定を取得できませんでした",
    "error.recording.empty": "録音データが空です",
    "error.recording.copy_failed": "音声ファイルの取り込みに失敗しました",
    "error.recording.mic_start": "マイクの録音開始に失敗しました",
    "error.recording.meeting_silent":
      "会議の音声がほぼ無音でした（許可の失効や相手/自分のミュートを確認してください）",
    "error.recording.not_found": "録音が見つかりません",
    "error.system_audio.busy": "すでにシステム音声をキャプチャ中です",
    "error.system_audio.permission":
      "システム音声の権限を取得できませんでした（システム設定 > プライバシーとセキュリティ > 画面とシステムオーディオの収録 で mojiroku を許可してください）",
    "error.system_audio.no_display": "ディスプレイが見つかりません",
    "error.summarize.api_key_missing": "クラウド要約には API キーが必要です（設定 → 要約エンジン）",
    "error.summarize.sidecar_failed": "ローカル要約の実行に失敗しました",
    "error.title.not_generated": "タイトルを作れませんでした。もう一度試すか、手で入力してください",
    "error.title.model_missing":
      "要約モデルがまだありません。先に要約を一度作ると、タイトルも生成できるようになります",
    "error.model.download": "モデルのダウンロードに失敗しました（ネットワーク接続を確認してください）",
    // 証明書の検証失敗。回線ではなく、通信を検査する中間装置が原因のことが多い（Issue #31）。
    "error.model.download_tls":
      "モデルのダウンロード先の証明書を検証できませんでした。社内・学内ネットワークやセキュリティソフトが通信を検査していると起こります。別のネットワークでお試しください。",
    "error.model.download_incomplete":
      "モデルのダウンロードが途中で切断されました（ネットワーク接続を確認して再試行してください）",
    "error.model.checksum_mismatch":
      "ダウンロードしたモデルの検証に失敗しました（破損の可能性があります。再試行してください）",
    "error.export.notion_parent_missing": "Notion の書き出し先ページが未設定です（連携 → Notion）",
    "error.export.notion_not_connected": "Notion と連携していません（連携 → Notion）",
    "error.export.notion_unauthorized":
      "Notion のトークンが無効です（連携 → Notion で再連携してください）",
    "error.export.notion_page_access":
      "Notion のページにアクセスできません（ページがインテグレーションに共有されているか、ID/URL を確認してください）",
    "error.export.notion_api": "Notion への送信に失敗しました",
    "error.export.slack_not_connected": "Slack と連携していません（連携 → Slack）",
    "error.export.slack_webhook_invalid":
      "Slack の Webhook が無効です（連携 → Slack で再連携してください）",
    "error.export.slack_api": "Slack への送信に失敗しました",
    "error.export.slack_no_summary":
      "Slack へ送る要約がありません（先に議事録・要約を作成してください）",
    "error.calendar.not_connected": "カレンダーが未連携です（連携 → カレンダー）",
    "error.calendar.google_refresh":
      "Google トークンの更新に失敗しました（連携 → カレンダー で再連携してください）",
    "error.oauth.timeout": "連携がタイムアウトしました（ブラウザでの許可が完了しませんでした）",
    "error.oauth.denied": "連携が許可されませんでした（ブラウザで認可を完了してください）",
    "error.oauth.state_mismatch":
      "連携の応答を検証できませんでした（安全のため中断しました。もう一度お試しください）",
    "error.speaker.name_empty": "名前が空です",
    "error.speaker.unknown_for_recording": "この録音に存在しない話者です",
    "error.segment.not_found": "対象の発言が見つかりません（画面を開き直してください）",
    "error.secret.unknown_key": "不明なシークレット名です",
    "error.job.no_audio": "この録音の音声ファイルが見つかりません",
    "error.job.no_transcript": "先に文字起こしを実行してください",
    "error.job.no_pertrack": "この録音にはトラック別音声がありません",
    "error.recording.discard_failed": "録音は止めましたが、音声ファイルを削除できませんでした（次回の起動時に削除されます）",
    "error.job.canceled": "処理を中断しました",
    "error.job.no_speakers_found": "話者を検出できなかったため、今の話者の割り当てをそのまま残しました",
    "error.job.unknown_kind": "不明なジョブ種別です",
    "error.job.failed": "処理に失敗しました",
  } as Record<string, string>,
};

export default ja;
export type Dict = typeof ja;

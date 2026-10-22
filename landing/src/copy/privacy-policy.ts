// プライバシーポリシー本文（ja / en）。LP のマーケコピー（ja.ts / en.ts）とは分けて持つ。
//
// なぜ独立したページが要るか:
//   Google の OAuth 審査は「ホームページと同一ドメインで公開されたプライバシーポリシー」を
//   必須要件にしている（sensitive scope verification）。加えて、配布アプリとして
//   「何が端末から出るのか」を一箇所で読めるようにする意味がある。
//
// ⚠️ ここに書く事実はコードと一致していなければ意味がない。通信先を増やしたら必ずこの表も足す。
//    2026-08-29 時点の宛先はソースから機械的に洗い出した（crates/ と src-tauri/src の https:// 全件）。
//
// ja を形の正とし、en は同じ型で構造一致を tsc に守らせる。

export interface PolicySection {
  heading: string;
  /** 段落。空配列可 */
  body: string[];
  /** 箇条書き。空配列可 */
  bullets: string[];
}

export interface EgressRow {
  /** 宛先ホスト */
  host: string;
  /** いつ通信するか */
  when: string;
  /** 何が送られるか */
  what: string;
  /** 既定でオンか、ユーザーが選ぶか */
  trigger: string;
}

export interface PrivacyPolicyCopy {
  meta: { title: string; description: string };
  title: string;
  updated: string;
  lead: string[];
  summary: { heading: string; bullets: string[] };
  stored: PolicySection;
  egress: {
    heading: string;
    body: string[];
    columns: { host: string; when: string; what: string; trigger: string };
    rows: EgressRow[];
    note: string;
  };
  /** 声紋（生体データにあたりうる）の扱い。話者ライブラリが会議をまたいで照合するため必須 */
  voiceprint: PolicySection;
  /** 同席者＝利用者以外の人のデータ。録音アプリのポリシーとして欠かせない */
  others: PolicySection;
  /** 運営者が動かす唯一のサーバー（Slack/Notion の OAuth 中継）の説明 */
  broker: PolicySection;
  google: PolicySection;
  /** 運営者が受け取る情報と、その利用目的（個人情報保護法の「利用目的の公表」に対応） */
  purposes: PolicySection;
  notCollected: PolicySection;
  /** 保存期間と、行わないことの明示 */
  retention: PolicySection;
  /** 安全管理措置の概要（法32条1項4号。詳細は求めに応じて回答する方式） */
  security: PolicySection;
  /**
   * 開示等の請求と苦情の申出先（個人情報保護法 32条・33条）。
   *
   * GDPR の監督機関への苦情申立権は 2026-08-29 に**外部レビューを受けて削除した**。
   * 一文だけ置くと GDPR 対応を自認したように読めるが、法的根拠・全権利の列挙・
   * EU 域内代理人（27条）は用意していない。EU 向けに積極提供していないので、
   * 日本法と Google の要件に絞るほうが整合する。将来 EU を狙うなら節ごと作り直す。
   */
  rights: PolicySection;
  /** macOS の権限、未成年、準拠法など */
  misc: PolicySection;
  deletion: PolicySection;
  website: PolicySection;
  changes: PolicySection;
  contact: {
    heading: string;
    /** 事業者の氏名（法32条）。住所は保有個人データが実質無いため載せていない */
    operatorLabel: string;
    operator: string;
    emailLabel: string;
    email: string;
    body: string[];
    linkLabel: string;
    linkHref: string;
  };
  backLabel: string;
}

const APP_DIR = "~/Library/Application Support/com.daichi0812.mojiroku";
const ISSUES = "https://github.com/daichi-hotta/mojiroku/issues";
// 連絡先。公開 repo のコミット著者として既に公開されているアドレス
const EMAIL = "daichi8120@gmail.com";

export const ja: PrivacyPolicyCopy = {
  meta: {
    title: "プライバシーポリシー | mojiroku",
    description:
      "mojiroku が扱うデータと、端末の外へ出る場面をすべて明記します。録音・文字起こし・要約は既定でこの Mac の中だけに保存されます。",
  },
  title: "プライバシーポリシー",
  updated: "最終更新: 2026年8月29日",
  lead: [
    "mojiroku は、録音・文字起こし・要約をあなたの Mac の中だけで完結させるデスクトップアプリです。開発・運営は個人が行っています（以下「当方」）。",
    "**アプリから、会議の音声・文字起こし・要約が当方へ自動的に送信されることはありません。**受け取る仕組みを作っていないためです。",
    "当方が運用しているサーバーは 1 つだけで、Slack・Notion 連携のための中継（OAuth ブローカー）です。中継する情報と保存の有無は、以下で説明します。",
    "このページは、mojiroku が何を保存し、どういうときに何が端末の外へ出るのかを、実装に即して書いたものです。アプリのソースは AGPL-3.0 で公開しているので、ここに書いてあることは自分で確かめられます。",
  ],
  summary: {
    heading: "このポリシーの要点",
    bullets: [
      "アカウント登録もログイン機能もなく、アプリの利用に伴って氏名やメールアドレスを取得することはありません。",
      "録音・文字起こし・要約・話者情報・**声の特徴量**は、あなたの Mac の中にだけ保存されます。",
      "利用状況の計測（テレメトリ）とクラッシュレポートの送信は行っていません。",
      "会議の内容が端末の外へ出るのは、下の表にある場面だけです。ほとんどはあなたが操作したときだけ動きます。",
      "Slack・Notion 連携の認可だけは当方の中継サーバーを経由します。Google カレンダーは経由しません。",
      "会議には同席者の声も入ります。**録音してよいかの確認は、あなたの責任**です。",
    ],
  },
  stored: {
    heading: "端末に保存されるもの",
    body: [
      `録音した音声、文字起こし、要約、議事録、話者、アクションアイテム、処理ジョブは、${APP_DIR} の中の SQLite データベースと音声ファイルとして保存されます。文字起こしと要約に使う AI モデルも同じ場所に置かれます。`,
      "API キーと連携トークンは、設定ファイルではなく macOS のキーチェーンに保管します。",
    ],
    bullets: [
      "保存先はすべてあなたの Mac の中で、運営者は参照できません。",
      "バックアップやクラウド同期は行いません（Time Machine や iCloud の対象になるかは、あなたの macOS の設定次第です）。",
    ],
  },
  egress: {
    heading: "端末の外へ出る場面",
    body: [
      "mojiroku のアプリケーションコードが意図的に行う外部通信は、次のとおりです。2026年8月29日時点のソースコードを走査して網羅的に列挙しました。",
    ],
    columns: {
      host: "宛先",
      when: "発生する場面",
      what: "送られるもの",
      trigger: "実行の条件",
    },
    rows: [
      {
        host: "huggingface.co",
        when: "初回起動時のモデル取得",
        what: "会議のデータは送りません（モデルの取得のみ）",
        trigger: "自動",
      },
      {
        host: "mojiroku.com / github.com",
        when: "アプリの更新確認とダウンロード",
        what: "会議のデータは送りません（更新情報の取得のみ。バージョンの比較は端末内で行います）",
        trigger: "自動",
      },
      {
        host: "mojiroku.com（OAuth ブローカー）",
        when: "Slack または Notion の連携を有効にしたとき",
        what: "認可コードと、その交換で得た Slack の Webhook URL / Notion のアクセストークン。**議事録や会議の内容は経由しません**",
        trigger: "設定で有効にしたとき",
      },
      {
        host: "accounts.google.com / oauth2.googleapis.com / www.googleapis.com",
        when: "Google カレンダー連携を有効にしたとき",
        what: "認可の要求とトークンの更新。予定の取得（受信のみ）",
        trigger: "設定で有効にしたとき",
      },
      {
        host: "calendar.google.com",
        when: "旧方式の限定公開 iCal URL を設定しているとき",
        what: "その URL への取得要求（受信のみ）",
        trigger: "設定で有効にしたとき",
      },
      {
        host: "api.anthropic.com / api.openai.com",
        when: "クラウド要約（BYOK）を選んで要約を実行したとき",
        what: "要約の対象になる文字起こしの本文",
        trigger: "既定はオフ。自分の API キーを設定したときだけ",
      },
      {
        host: "api.notion.com / www.notion.so",
        when: "Notion へ書き出すボタンを押したとき",
        what: "書き出す議事録（要約と文字起こし）",
        trigger: "操作するたび",
      },
      {
        host: "hooks.slack.com",
        when: "Slack へ送るボタンを押したとき",
        what: "送信する議事録",
        trigger: "操作するたび",
      },
      {
        host: "chatgpt.com / claude.ai",
        when: "「ChatGPT で開く」「Claude で開く」を押したとき",
        what: "**文字起こしと要約を含むプロンプト**。クリップボードへコピーし、あなたが貼り付けて渡します。**URL には載せません**",
        trigger: "操作するたび",
      },
      {
        host: "docs.google.com",
        when: "フィードバックフォームを開いたとき",
        what: "アプリと macOS のバージョン、Mac の種別（URL の事前入力として載る）",
        trigger: "操作するたび。フォームの送信もあなたが押したときだけ",
      },
    ],
    note: "広告配信事業者・データブローカー・解析事業者へデータを渡すことはありません。なお、どのような HTTP 要求でも、接続元の IP アドレス・User-Agent・要求したファイル名は接続先から見えます（mojiroku に限らずインターネット通信一般の性質です）。また、配信に CDN やリダイレクトが使われるため、実際の接続先ホストが表の記載と一致しないことがあります。それぞれの宛先での取り扱いは各社のプライバシーポリシーに従い、いずれも日本国外の事業者です。",
  },
  voiceprint: {
    heading: "声の特徴量（声紋）について",
    body: [
      "mojiroku は「誰が話したか」を分けるために、話者ごとの**声の特徴量**を計算します。ラベル文字列ではなく数値のベクトルで、`speaker_embeddings` テーブルに保存されます。",
      "話者ライブラリを使うと、この特徴量を過去の録音と照合して同じ人物を推定します。**会議をまたいで個人を識別できる情報**であり、地域によっては生体データとして特別な扱いを受けることがあります。取り扱いに注意が必要な情報として、ここに明記します。",
      "特徴量は端末内でのみ計算・保存・照合され、当方を含むいかなるサーバーへも送信しません。書き出し（Notion・Slack・PDF）にも含まれません。",
    ],
    bullets: [
      "録音を削除すると、その録音の特徴量と照合結果も一緒に削除されます（データベースの外部キー制約による連鎖削除）。",
      "話者ライブラリに登録した名前も、あなたの Mac の中にだけ残ります。",
      "話者分離を使わなければ、特徴量は作られません。",
    ],
  },
  others: {
    heading: "同席者（あなた以外の人）のデータについて",
    body: [
      "会議の録音には、あなた以外の人の声が入ります。その音声・発言内容・話者ラベル・声の特徴量も、上と同じくあなたの Mac の中にだけ保存され、当方には届きません。",
      "**録音してよいかどうかの確認は、あなたの責任で行ってください。** 国や地域によっては、通話や会議の録音に同席者全員の同意が必要です。mojiroku は同意の取得を代行しません。",
      "書き出しやクラウド要約を使うと、同席者の発言もその宛先へ渡ります。誰の情報を、どこへ出すのかを確認したうえで操作してください。",
    ],
    bullets: [
      "同席者から削除を求められた場合、その録音をアプリの履歴から削除することで対応できます。当方が保持しているものはありません。",
    ],
  },
  broker: {
    heading: "Slack・Notion 連携の中継サーバーについて",
    body: [
      "Slack と Notion は、認可コードをトークンに交換する際に「アプリの秘密鍵」を必要とする方式（confidential client）しか認めていません。配布するアプリに秘密鍵を埋め込むと解析により取り出せてしまうため、mojiroku.com 上の中継（Cloudflare Worker）が交換だけを代行しています。",
      "この中継を通るのは、認可コードと、その交換で得た Slack の Webhook URL または Notion のアクセストークンだけです。**議事録・要約・文字起こし・音声は一切通りません。**書き出しの本文は、あなたの Mac から Slack / Notion へ直接送られます。",
      "中継のコードはデータベースもファイルストレージも持たず、通過した値を保存しません。受け取った結果はその場であなたの端末（127.0.0.1）へ転送され、以後は端末の macOS キーチェーンに保管されます。ただし基盤である Cloudflare 側には接続元 IP や時刻などのアクセスログが存在し、その取り扱いは Cloudflare のポリシーに従います。",
    ],
    bullets: [
      "**Google カレンダーはこの中継を経由しません。** 端末が Google と直接やり取りします（PKCE + loopback）。",
      "中継のソースコードも同じリポジトリで公開しています（`landing/worker/index.ts`）。",
      "連携を使わなければ、この中継に接続することはありません。",
    ],
  },
  google: {
    heading: "Google カレンダーのデータについて",
    body: [
      "mojiroku が Google に求める権限は `calendar.events.readonly`（予定の閲覧）ひとつだけです。予定の作成・変更・削除はできません。",
      "取得するのは、直近の予定のタイトルと開始・終了時刻です。用途は次の 2 つで、いずれもアプリの画面上で確認できる機能です。会議の開始時刻に「録音しますか」と通知を出すこと、および録音のタイトルに予定名を補完することです。",
      "取得した予定の内容自体はデータベースに保存しません。通知とタイトル補完のためにメモリ上で使うだけです。あなたが録音を作った場合に限り、その録音のタイトルとして予定名があなたの Mac の中に残ることがあります。",
      "**Google から取得したデータを当方のサーバーへ送信することはありません。**認可も予定の取得も、あなたの端末が Google と直接行います（PKCE + loopback リダイレクト）。前節の Slack・Notion 用の中継サーバーは、Google のデータには一切関与しません。当方が mojiroku を通じてあなたの予定を閲覧することはできません。",
      "⚠️ ただし、予定名が録音のタイトルとして補完された状態で議事録を Notion や Slack へ書き出すと、**そのタイトルは書き出し先へ渡ります**。自動では起きず、あなたが書き出しを実行したときだけです。",
    ],
    bullets: [
      "アクセストークンとリフレッシュトークンは macOS のキーチェーンに保管します。",
      "アプリの「連携を解除」を押すと、保存されたトークンは削除されます。Google アカウント側からも取り消せます。",
      "mojiroku による Google ユーザーデータの取り扱いは、Limited Use requirements を含む Google API Services User Data Policy に準拠します。取得したデータは、上に挙げたアプリ内機能の提供のためだけに使い、第三者へ移転せず、広告には一切使いません。",
    ],
  },
  purposes: {
    heading: "当方が取得・保存する情報と、その利用目的",
    body: [
      "アプリは端末内で完結するので、あなたの会議データが当方に届くことはありません。当方の手元に届くのは、あなたが自分から送ったものだけです。",
    ],
    bullets: [
      "**問い合わせのメール**。送信元のメールアドレスと本文、およびあなたが任意で書いた情報です。利用目的は、問い合わせへの回答と不具合の調査です。",
      "**フィードバックフォームの回答**（Google フォーム）。用途・Mac の種別・macOS のバージョン・ターミナルの利用頻度と、自由記述です。フォームでは氏名もメールアドレスも取得しておらず、Google アカウントでのログインも求めていないため、**通常、当方は回答者を識別できません**（自由記述にご自身で書かれた場合を除きます）。利用目的は、不具合の切り分けと機能の優先順位づけです。",
      "**GitHub の Issue やコメント**。あなたが自分で書き込んだ内容です。利用目的は、不具合の対応と機能の検討です。GitHub 上での取り扱いは GitHub のプライバシーポリシーに従います。",
      "**このサイトの閲覧数**（Cloudflare Web Analytics）。当方が確認できるのは、個人を識別しない集計データです。利用目的は、どのページが読まれているかを把握することです。",
    ],
  },
  retention: {
    heading: "保存期間と、当方が行わないこと",
    body: [
      "問い合わせのメールとフィードバックの回答は、対応と改善に必要な期間だけ保持し、必要がなくなった時点で削除します。GitHub の Issue は公開の記録として残ります。",
      "このほかに、Slack・Notion の連携では認可コードとトークンが当方の中継サーバーを**経由**しますが、そこには保存しません（前の節を参照）。",
    ],
    bullets: [
      "取得した情報を、広告・プロファイリング・第三者への販売に使うことはありません。",
      "本人の同意なく第三者へ提供することはありません（法令に基づく場合を除きます）。",
    ],
  },
  notCollected: {
    heading: "収集していないもの",
    body: [],
    bullets: [
      "アカウント情報（登録もログインもありません）",
      "利用状況の計測・行動ログ（テレメトリを送っていません）",
      "クラッシュレポートの自動送信",
      "位置情報、連絡先、広告 ID",
      "録音や文字起こしの内容（運営者が受け取る仕組みがありません）",
    ],
  },
  deletion: {
    heading: "データの削除",
    body: [
      "アプリが扱うデータはすべてあなたの端末にあるので、削除もあなたの手元で完結します。**アプリ本体を消すだけでは残るものがある**ので、順番に消してください。",
    ],
    bullets: [
      "**個別の録音・議事録**は、アプリの履歴から削除できます。音声ファイル、文字起こし、要約、話者、声の特徴量が一緒に消えます。",
      "**連携トークンと API キー**は、アプリの「連携を解除」または設定から削除できます。⚠️ これは**端末に保管したトークンを消すだけ**で、Slack・Notion・Google 側の認可は取り消されません。各サービスの設定画面から取り消してください。",
      `**すべて消す場合**は、①アプリ内で各連携を解除し API キーを削除 → ②アプリを削除 → ③ ${APP_DIR} を削除、の順に行ってください。②③ だけではキーチェーンの項目が残ります。`,
    ],
  },
  security: {
    heading: "安全管理措置",
    body: [
      "アプリ側の措置は次のとおりです。API キーと連携トークンは平文の設定ファイルではなく macOS のキーチェーンに保管します。録音と議事録は端末内の SQLite とファイルに保存され、外部へ同期しません。",
      "当方が受け取った情報（問い合わせのメール、フィードバックの回答）については、アクセスできる者を当方本人に限定し、利用する各サービスのアカウントに適切な認証を設定しています。",
      "具体的な内容は、下の連絡先へ問い合わせいただければ遅滞なく回答します。",
    ],
    bullets: [],
  },
  rights: {
    heading: "利用者の権利と請求の手続き",
    body: [
      "アプリが扱うデータはすべてあなたの端末にあるので、内容の確認も削除も当方を通さずに行えます（前の節を参照）。",
      "当方が取得した情報（問い合わせのメール、フィードバックの回答、Issue のやりとり）については、利用目的の通知・開示・訂正・追加・削除・利用停止・第三者提供の停止の請求を受け付けます。下の連絡先へ、請求の対象となる情報を特定できる形でご連絡ください。手数料はかかりません。",
      "ただしフィードバックの回答からは、通常、回答者を特定できません。そのため、回答の内容を指定していただけない限り、対象を絞り込めないことがあります。",
      "当方の住所は、個人が運営しているため公開していません。**本人からの求めに応じて遅滞なく回答します**ので、必要な場合は下の連絡先へご連絡ください。",
    ],
    bullets: [
      "このポリシーやデータの取り扱いについての苦情も、下の連絡先で受け付けます。",
    ],
  },
  misc: {
    heading: "そのほか",
    body: [],
    bullets: [
      "**macOS の権限**: 要求するのは 3 つです。マイク（録音のため）、画面とシステムオーディオの収録（会議モードで通話相手の音声を端末内で文字起こしするため）、通知（会議の開始時に録音を促すため。拒否しても通知が出ないだけで、ほかの機能は通常どおり動きます）。macOS のカレンダーへのアクセス権限は要求しません（予定は Google の API から取得します）。",
      "**未成年の利用**: 13 歳未満の方の利用を想定しておらず、意図的に情報を取得することはありません。",
      "**国外への移転**: このポリシーに挙げた通信先（Cloudflare・GitHub・Google・Anthropic・OpenAI・Hugging Face・Slack・Notion）はいずれも日本国外の事業者です。それぞれの国の制度と各社のポリシーに従って取り扱われます。",
      "**準拠法**: このポリシーは日本法に従って解釈されます。",
    ],
  },
  website: {
    heading: "このウェブサイトについて",
    body: [
      "mojiroku.com では、閲覧数の把握のために Cloudflare Web Analytics を使っています。Cookie を使わず、個人を識別する情報も、閲覧者を横断して追跡する識別子も収集しません。",
    ],
    bullets: [
      "広告は掲載していません。広告目的のトラッカーも入れていません。",
      "ダウンロードは GitHub Releases から配信されます。その際は GitHub のプライバシーポリシーが適用されます。",
    ],
  },
  changes: {
    heading: "このポリシーの変更",
    body: [
      "内容を変更した場合は、このページの最終更新日を改めます。通信先が増える変更を行うときは、上の表を必ず更新します。",
      "このページの変更履歴は、ソースリポジトリの Git 履歴として公開されています。",
    ],
    bullets: [],
  },
  contact: {
    heading: "運営者と連絡先",
    operatorLabel: "運営者",
    operator: "堀田 大智（Hotta Daichi）",
    emailLabel: "連絡先",
    email: EMAIL,
    body: [
      "mojiroku は個人が開発・運営しています。",
      "このポリシーやデータの扱いについての質問、開示等の請求、苦情は、上のアドレスで受け付けています。",
      "不具合の報告や機能の要望も、同じアドレスかアプリ内のフィードバックフォームからお寄せください。GitHub の Issue は誰でも閲覧できますが、**新規の作成は共同作業者に限っています**。",
    ],
    linkLabel: "GitHub の Issue を見る",
    linkHref: ISSUES,
  },
  backLabel: "トップへ戻る",
};

export const en: PrivacyPolicyCopy = {
  meta: {
    title: "Privacy Policy | mojiroku",
    description:
      "Everything mojiroku stores, and every case where data leaves your Mac. Recordings, transcripts and summaries stay on your device by default.",
  },
  title: "Privacy Policy",
  updated: "Last updated: 29 August 2026",
  lead: [
    "mojiroku is a desktop app that records, transcribes and summarises meetings entirely on your Mac. It is developed and operated by one individual (\"we\" below).",
    "**The app never automatically sends your meeting audio, transcripts or summaries to us.** We built no mechanism to receive them.",
    "We operate exactly one server: a relay (an OAuth broker) that exists only to make the Slack and Notion integrations work. What passes through it, and what it does not store, is described below.",
    "This page describes what mojiroku stores and exactly when data leaves your device, based on the actual implementation. The source is published under AGPL-3.0, so you can verify every statement here yourself.",
  ],
  summary: {
    heading: "In short",
    bullets: [
      "There is no account and no sign-in, and using the app never gives us your name or email address.",
      "Recordings, transcripts, summaries, speaker data and **voice embeddings** are stored only on your Mac.",
      "We collect no usage analytics and send no crash reports.",
      "Your meeting content leaves the device only in the cases listed in the table below, and most of them only when you act.",
      "Only the Slack and Notion authorisation passes through a relay we operate. Google Calendar does not.",
      "Meetings contain other people's voices. **Checking that recording is permitted is your responsibility.**",
    ],
  },
  stored: {
    heading: "What is stored on your device",
    body: [
      `Recorded audio, transcripts, summaries, minutes, speakers, action items and processing jobs are stored as a SQLite database and audio files under ${APP_DIR}. The AI models used for transcription and summarisation are kept in the same place.`,
      "API keys and integration tokens are stored in the macOS Keychain rather than in a settings file.",
    ],
    bullets: [
      "All of it stays on your Mac; we cannot access any of it.",
      "We perform no backup and no cloud sync (whether Time Machine or iCloud picks these files up depends on your own macOS settings).",
    ],
  },
  egress: {
    heading: "When data leaves your device",
    body: [
      "These are the only cases in which mojiroku talks to the network. The list was derived mechanically from the source code as of 29 August 2026.",
    ],
    columns: {
      host: "Destination",
      when: "When it happens",
      what: "What is sent",
      trigger: "Condition",
    },
    rows: [
      {
        host: "huggingface.co",
        when: "Downloading models on first launch",
        what: "No meeting data is sent (model download only)",
        trigger: "Automatic",
      },
      {
        host: "mojiroku.com / github.com",
        when: "Checking for and downloading app updates",
        what: "No meeting data is sent (the update manifest is fetched; the version comparison happens on your Mac)",
        trigger: "Automatic",
      },
      {
        host: "mojiroku.com (OAuth broker)",
        when: "When you enable the Slack or Notion integration",
        what: "The authorisation code, and the Slack webhook URL / Notion access token obtained by exchanging it. **No minutes or meeting content passes through**",
        trigger: "When you enable it in settings",
      },
      {
        host: "accounts.google.com / oauth2.googleapis.com / www.googleapis.com",
        when: "When you enable Google Calendar",
        what: "Authorisation requests and token refreshes; fetching events (receive only)",
        trigger: "When you enable it in settings",
      },
      {
        host: "calendar.google.com",
        when: "If you use the older secret iCal URL method",
        what: "A fetch request to that URL (receive only)",
        trigger: "When you enable it in settings",
      },
      {
        host: "api.anthropic.com / api.openai.com",
        when: "When you run a cloud summary (BYOK)",
        what: "The transcript text being summarised",
        trigger: "Off by default; only after you set your own API key",
      },
      {
        host: "api.notion.com / www.notion.so",
        when: "When you press the export-to-Notion button",
        what: "The minutes being exported (summary and transcript)",
        trigger: "Every time you act",
      },
      {
        host: "hooks.slack.com",
        when: "When you press the send-to-Slack button",
        what: "The minutes being sent",
        trigger: "Every time you act",
      },
      {
        host: "chatgpt.com / claude.ai",
        when: "When you press Open in ChatGPT / Open in Claude",
        what: "**A prompt containing the transcript and summary.** It is copied to your clipboard for you to paste. **It is never placed in the URL**",
        trigger: "Every time you act",
      },
      {
        host: "docs.google.com",
        when: "When you open the feedback form",
        what: "App and macOS version and Mac type, pre-filled into the form URL",
        trigger: "Every time you act; the form is only submitted if you submit it",
      },
    ],
    note: "We never pass data to advertising networks, data brokers or analytics vendors. Note that with any HTTP request, the connecting IP address, User-Agent and requested filename are visible to the destination server — a property of internet communication generally, not something specific to mojiroku. Because CDNs and redirects are involved, the host actually contacted may differ from the entry above. Each destination handles data under its own privacy policy, and all of them are companies outside Japan.",
  },
  voiceprint: {
    heading: "Voice embeddings",
    body: [
      "To tell speakers apart, mojiroku computes a **voice embedding** for each speaker. It is a numeric vector rather than a label, stored in the `speaker_embeddings` table.",
      "If you use the speaker library, that embedding is matched against past recordings to suggest the same person. It is therefore information that **can identify an individual across meetings**, and in some jurisdictions it is treated as biometric data with special protections. We state this explicitly because it warrants care.",
      "Embeddings are computed, stored and matched on your device only. They are never sent to us or to any server, and they are not included in exports (Notion, Slack, PDF).",
    ],
    bullets: [
      "Deleting a recording also deletes its embeddings and match results (cascading foreign keys in the database).",
      "Names you add to the speaker library also stay only on your Mac.",
      "If you do not use speaker diarization, no embedding is created.",
    ],
  },
  others: {
    heading: "Other people in the meeting",
    body: [
      "A meeting recording contains voices other than yours. Their audio, what they said, their speaker labels and their voice embeddings are stored only on your Mac, exactly as above, and never reach us.",
      "**Confirming that recording is permitted is your responsibility.** In some countries and regions, recording a call or meeting requires the consent of everyone present. mojiroku does not obtain that consent for you.",
      "If you use an export or a cloud summary, other people's words go to that destination too. Check whose information you are sending, and where, before you act.",
    ],
    bullets: [
      "If someone asks you to delete their data, deleting that recording from the app's history is sufficient. We hold nothing of it.",
    ],
  },
  broker: {
    heading: "The relay used for Slack and Notion",
    body: [
      "Slack and Notion only support an exchange flow that requires an application secret (a confidential client). Embedding such a secret in a distributed app would let anyone extract it, so a small relay (a Cloudflare Worker) on mojiroku.com performs the exchange instead.",
      "Only two things pass through the relay: the authorisation code, and the Slack webhook URL or Notion access token obtained from it. **No minutes, summaries, transcripts or audio ever pass through it.** Exported content goes directly from your Mac to Slack or Notion.",
      "The relay code has no database and no file storage, and it retains nothing that passes through. The result is redirected straight back to your device (127.0.0.1) and is then kept in your macOS Keychain. Cloudflare, which hosts it, does keep edge access logs (source IP, timestamps) under its own policy.",
    ],
    bullets: [
      "**Google Calendar does not use this relay.** Your device talks to Google directly (PKCE with a loopback redirect).",
      "The relay's source is published in the same repository (`landing/worker/index.ts`).",
      "If you never use those integrations, your device never contacts the relay.",
    ],
  },
  google: {
    heading: "Google Calendar data",
    body: [
      "mojiroku requests a single Google permission: `calendar.events.readonly` (view events). It cannot create, modify or delete anything.",
      "It reads the titles and start and end times of your upcoming events. There are two uses, both visible in the app's interface: showing a notification asking whether to start recording when a meeting begins, and pre-filling the recording's title with the event name.",
      "The events themselves are never written to the database. They are held in memory only, for the notification and the title. If you do start a recording, the event name may remain on your Mac as that recording's title.",
      "**We never transmit data obtained from Google to a server of ours.** Both the authorisation and the event fetch happen directly between your device and Google (PKCE with a loopback redirect). The Slack and Notion relay described above plays no part in handling Google data. We cannot read your events through mojiroku.",
      "⚠️ However, if an event name has been used as a recording title and you then export those minutes to Notion or Slack, **that title goes to the export destination**. This never happens automatically — only when you run an export.",
    ],
    bullets: [
      "Access and refresh tokens are stored in the macOS Keychain.",
      "Pressing Disconnect in the app deletes the stored tokens. You can also revoke access from your Google Account.",
      "mojiroku's use of information received from Google APIs adheres to the Google API Services User Data Policy, including the Limited Use requirements. Data obtained is used solely to provide the in-app features described above, is never transferred to third parties, and is never used for advertising.",
    ],
  },
  purposes: {
    heading: "What we collect, and why",
    body: [
      "The app runs entirely on your device, so your meeting data never reaches us. What arrives on our side is only what you send us yourself.",
    ],
    bullets: [
      "**Emails you send us**: your sending address, the body, and anything you choose to include. We use them to answer you and to investigate bugs.",
      "**Feedback form responses** (Google Forms): intended use, Mac type, macOS version, how often you use a terminal, plus free text. The form collects no name and no email address, and does not ask you to sign in with Google, so **we normally cannot identify who responded** (unless you write it in the free text yourself). We use them to diagnose bugs and to decide what to build next.",
      "**GitHub issues and comments** that you write yourself. We use them to fix bugs and consider features. What GitHub does with them is governed by GitHub's own privacy policy.",
      "**Page views on this site** (Cloudflare Web Analytics): what we can see is aggregate data that identifies no one. We use it to see which pages get read.",
    ],
  },
  retention: {
    heading: "Retention, and what we never do",
    body: [
      "Emails and feedback responses are kept only as long as needed to respond and improve, and deleted once they are no longer needed. GitHub issues remain as a public record.",
      "Separately, the Slack and Notion authorisation codes and tokens **pass through** our relay, but nothing is retained there (see above).",
    ],
    bullets: [
      "None of this is used for advertising, profiling, or sale to third parties.",
      "We do not disclose it to third parties without your consent, except where required by law.",
    ],
  },
  notCollected: {
    heading: "What we do not collect",
    body: [],
    bullets: [
      "Account details (there is no sign-up and no sign-in)",
      "Usage analytics or behavioural logs (no telemetry is sent)",
      "Automatic crash reports",
      "Location, contacts or advertising identifiers",
      "The contents of your recordings or transcripts (no mechanism exists for us to receive them)",
    ],
  },
  deletion: {
    heading: "Deleting your data",
    body: [
      "Everything the app handles lives on your device, so deletion happens entirely in your hands. **Removing the app alone leaves some things behind**, so work through these in order.",
    ],
    bullets: [
      "**Individual recordings and minutes** can be deleted from the app's history. The audio, transcript, summary, speakers and voice embeddings go with them.",
      "**Integration tokens and API keys** can be removed with Disconnect, or from Settings. ⚠️ This only deletes the tokens held on your device — it does **not** revoke the authorisation on Slack, Notion or Google. Revoke those from each service's own settings.",
      `**To remove everything**: (1) disconnect each integration and delete your API keys in the app, (2) delete the app, (3) delete ${APP_DIR}. Steps 2 and 3 alone leave Keychain entries behind.`,
    ],
  },
  security: {
    heading: "How we protect information",
    body: [
      "On the app side: API keys and integration tokens live in the macOS Keychain rather than a plaintext settings file. Recordings and minutes are stored in a local SQLite database and files, with no external sync.",
      "For what we do receive (emails and feedback responses), access is limited to the operator alone, and the accounts of the services involved are protected with appropriate authentication.",
      "If you would like the specifics, ask at the contact address below and we will answer without delay.",
    ],
    bullets: [],
  },
  rights: {
    heading: "Your rights, and how to exercise them",
    body: [
      "Everything the app handles lives on your device, so you can inspect or delete it without going through us (see the section above).",
      "For the information we do collect (emails, feedback responses and issue threads), you may request notification of purpose, access, correction, addition, deletion, suspension of use, or that we stop sharing it. Write to the address below in a way that identifies the information your request concerns. There is no fee.",
      "Note that we normally cannot identify who submitted a feedback response. Unless you can tell us what the response said, we may be unable to locate it.",
      "We do not publish a postal address, as mojiroku is run by an individual. **We will provide it without delay on request** — just ask at the contact address below.",
    ],
    bullets: [
      "Complaints about this policy or about how data is handled are welcome at the same address.",
    ],
  },
  misc: {
    heading: "Other points",
    body: [],
    bullets: [
      "**macOS permissions**: we request three. The microphone (for recording), screen and system audio recording (to transcribe the other party's audio on your Mac in meeting mode), and notifications (to prompt you to record when a meeting starts — declining only means no notifications; everything else works normally). We do not request macOS calendar permission (events come from Google's API instead).",
      "**Minors**: mojiroku is not intended for children under 13, and we do not knowingly collect their information.",
      "**Transfers outside Japan**: every destination listed in this policy (Cloudflare, GitHub, Google, Anthropic, OpenAI, Hugging Face, Slack, Notion) is a company outside Japan, and handles data under the laws of its country and its own policy.",
      "**Governing law**: this policy is interpreted under the laws of Japan.",
    ],
  },
  website: {
    heading: "About this website",
    body: [
      "mojiroku.com uses Cloudflare Web Analytics to count page views. It sets no cookies and collects no personally identifying information and no identifier that follows visitors across sites.",
    ],
    bullets: [
      "There is no advertising on this site, and no advertising trackers.",
      "Downloads are served from GitHub Releases; GitHub's privacy policy applies to those requests.",
    ],
  },
  changes: {
    heading: "Changes to this policy",
    body: [
      "If this policy changes, the last-updated date above changes with it. Any change that adds a network destination will also update the table above.",
      "The revision history of this page is public as the Git history of the source repository.",
    ],
    bullets: [],
  },
  contact: {
    heading: "Who runs mojiroku, and how to reach us",
    operatorLabel: "Operator",
    operator: "Hotta Daichi (堀田 大智)",
    emailLabel: "Contact",
    email: EMAIL,
    body: [
      "mojiroku is developed and operated by an individual.",
      "Questions about this policy, requests regarding your information, and complaints are all welcome at the address above.",
      "Bug reports and feature requests are welcome at the same address, or through the in-app feedback form. GitHub issues are readable by anyone, but **only collaborators can create new ones**.",
    ],
    linkLabel: "Browse the GitHub issues",
    linkHref: ISSUES,
  },
  backLabel: "Back to home",
};

const dict: Record<string, PrivacyPolicyCopy> = { ja, en };

/** Astro.currentLocale を安全に解決する。未知値は ja。 */
export function getPrivacyCopy(locale: string | undefined): PrivacyPolicyCopy {
  return dict[locale ?? "ja"] ?? ja;
}

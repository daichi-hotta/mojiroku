// ── mojiroku landing copy (English) ──
// Not a literal translation of ja.ts — rewritten as native US-market SaaS copy
// (short headlines, active voice, privacy-first framing), while keeping the same
// house rules: shipped features only, cloud sending is strictly opt-in, no
// competitor is named. The shape must match `Copy` (= typeof ja); tsc enforces it.
// NOTE: mentions English transcription/summaries — ship this page together with
// (or after) the app release that includes English language support.
import type { Copy } from "./index";

const en: Copy = {
  layout: {
    ogImageAlt:
      "mojiroku — turn meetings into notes, entirely on your Mac. A free, on-device meeting notes app.",
    skipLink: "Skip to content",
  },

  hero: {
    badge: "On-device · Free to use",
    headline: "Turn meetings into notes, entirely on your Mac.",
    sub: "mojiroku records, transcribes, summarizes, and labels who said what — all with local AI on Apple Silicon. Free to use. A built-in MCP server lets Claude search your meeting notes. It only touches the network when you choose cloud summaries or integrations.",
    primaryCta: { label: "Download for macOS", href: "/download" },
    secondaryCta: { label: "See what it does", href: "#features" },
    sysreq: "Apple Silicon Mac (M1 or later) · macOS 11+",
    note: "Free · No account · ~12 MB · Models download automatically on first run",
    screenshot: {
      label: "Detail view — transcript, summary, speakers",
      alt: "mojiroku's detail view: a speaker-labeled transcript alongside the summary and meeting notes.",
    },
  },

  meta: {
    title: "mojiroku — Free, on-device meeting notes for Mac",
    description:
      "Transcribe and summarize recordings and audio files entirely on your Mac. Speaker diarization, full-text search, and Notion/Slack export all run on-device — nothing is uploaded unless you opt into cloud summaries. Includes an MCP server so Claude can search your meeting notes. Free, for Apple Silicon.",
  },

  nav: {
    ariaLabel: "Main navigation",
    links: [
      { label: "Features", href: "#features" },
      { label: "Claude (MCP)", href: "#mcp" },
      { label: "Compare", href: "#comparison" },
      { label: "Privacy", href: "#privacy" },
      { label: "FAQ", href: "#faq" },
    ],
    cta: { label: "Download", href: "/download" },
    langSwitch: { label: "日本語", href: "/" },
  },

  problem: {
    eyebrow: "Sound familiar?",
    heading: "Meeting notes are still manual work.",
    items: [
      {
        title: "The meeting ends. The typing begins.",
        body: "You replay the recording, pull out the key points, and sort out who said what. A meeting that was supposed to be over turns into another evening of manual work — on a step that should have been automatic all along.",
      },
      {
        title: "Pricing that grows with every minute",
        body: "Cloud note-takers bill by transcription minutes and seats. The more meetings you have, the more you pay — until “just record everything” stops being an easy decision.",
      },
      {
        title: "Where does that audio actually go?",
        body: "Meeting recordings carry confidential details and half-formed thoughts. Handing them to someone else's server is a real hesitation — convenience in exchange for losing sight of where your voice ends up.",
      },
    ],
  },

  valueProps: {
    eyebrow: "Why mojiroku",
    heading: "Your meetings never have to leave your Mac.",
    items: [
      {
        title: "On-device, private by default",
        body: "Transcription and summarization run as local AI on Apple Silicon. Recordings and notes are stored on this Mac (in a local SQLite database), and your voice never leaves the device on its own. Local features — transcription, summaries, search — work even in airplane mode. Only when you choose cloud summaries does anything get sent, to the provider you pick, with your own API key.",
      },
      {
        title: "Free to use, zero running costs",
        body: "The app, local transcription, and local summaries are free. No server of ours ever holds your meeting data, so it costs $0 to operate — no subscription, no storage caps. If you want higher-quality cloud summaries, that's an opt-in you pay for directly with your own API key.",
      },
      {
        title: "Recording to minutes, in one flow",
        body: "Import audio files or record from the mic, let mojiroku label who said what, and get AI meeting notes, summaries, and action items. Full-text search brings any meeting back instantly, and Notion, Slack, and PDF are one click away. With MCP built in, you can even ask Claude about your own meetings.",
      },
    ],
  },

  features: [
    {
      id: "core",
      eyebrow: "Core features",
      heading: "Recording to meeting notes, all inside this Mac.",
      body: "Import an audio file or record on the spot — mojiroku takes it from there: transcript, summary, meeting notes, action items. By default, both transcription and summarization run on-device, and your voice never leaves the Mac. For summaries you can optionally bring your own key (Anthropic Claude / OpenAI), and only then is the transcript sent to the provider you chose. The app and local AI are free to use, and the whole thing runs on $0 of infrastructure.",
      bullets: [
        "One pipeline from import to minutes. mp3, wav, m4a and other audio files — or mic recordings — transcribed locally with whisper.cpp on Apple Metal, in English and Japanese. Voice-activity detection keeps silent stretches from turning into hallucinated text.",
        "Three templates to match the moment: AI meeting notes, summary, or action items — generated on-device by default with a local model (Qwen3.5-9B or another, picked to fit your Mac). Cloud summaries happen only if you opt in, with your own API key.",
        "Speaker diarization sorts out who said what, powered by sherpa-onnx (pyannote seg-3.0, no torch required).",
        "Built-in MCP server: search and reference your meeting notes from Claude Desktop or Claude Code. FTS5 full-text search spans titles and transcripts across every meeting.",
      ],
      image: "/screenshots/detail.png",
      screenshotCaption:
        "From recording and transcript to meeting notes and action items — mojiroku's main view, entirely on-device.",
      label: "Detail view",
      badge: null as string | null,
    },
    {
      id: "meeting",
      eyebrow: "Meeting mode",
      heading: "No bots in your meetings.",
      body: "Capture your online meeting's system audio and your own mic at the same time, with a live transcript as it happens. There's no stranger bot to invite into the call. The audio is processed on your Mac, and the notes stay right there on your device.",
      bullets: [
        "Records the other side (the meeting's system audio) together with your mic, using ScreenCaptureKit to capture the audio playing on your screen.",
        "Real-time transcription during the call — whisper.cpp on Apple Metal, entirely on your Mac.",
        "Speaker separation included: when the meeting ends, you already have a log organized by speaker.",
      ],
      image: "/screenshots/meeting.png",
      screenshotCaption:
        "Meeting mode during a call: system audio and mic recorded together and transcribed in real time, then organized by speaker after the meeting ends.",
      label: "Meeting mode",
      badge: "NEW" as string | null,
    },
    {
      id: "speakers",
      eyebrow: "Diarization + speaker library",
      heading: "Who said what, automatically.",
      body: "Recorded meetings are split by speaker, on-device. Voiceprints never leave your Mac. On top of that, the speaker library gradually learns to recognize the same people across meetings — so matching names to voices gets easier with every call.",
      bullets: [
        "Utterances are split by speaker automatically, so the notes read like a conversation — who said what, at a glance.",
        "On-device voiceprints loosely recognize the same person across meetings (accuracy still being calibrated). Name someone once and reuse it.",
        "Audio and voiceprints are stored on your device. Nothing is uploaded to any cloud.",
      ],
      image: "/screenshots/speakers.png",
      screenshotCaption:
        "A transcript color-coded by speaker. The same person is loosely linked across meetings (accuracy still being calibrated).",
      label: "Speaker library",
      badge: "NEW" as string | null,
    },
    {
      id: "history",
      eyebrow: "History & full-text search",
      heading: "Find any meeting in one line.",
      body: "Every past meeting lives on your Mac. SQLite full-text search (FTS5) sweeps titles and transcripts in an instant. That one remark, that one decision — no more digging through folders on memory alone. It's search that runs entirely on-device, with nothing entrusted to a cloud.",
      bullets: [
        "Fast search across all your notes, titles and transcripts alike — results narrow the moment you type.",
        "Search and data both stay on-device. Your meeting notes never leave the Mac.",
        "Edit titles and delete what you don't need, right there — your history, kept your way.",
      ],
      image: "/screenshots/history.png",
      screenshotCaption:
        "Type a keyword and matching notes line up instantly, across titles and transcripts. All searched on-device.",
      label: "History & search",
      badge: null as string | null,
    },
    {
      id: "integrations",
      eyebrow: "Integrations & export",
      heading: "Send your notes where they already live.",
      body: "One click turns a finished meeting into a Notion page, or posts the summary to Slack. Pull events in from Google Calendar and recording titles fill themselves in. Want a local copy? Export Markdown, plain text, SRT, or PDF. Your team's flow stays untouched — the notes simply show up where they're needed.",
      bullets: [
        "Create Notion pages and post summaries to Slack — both connected via OAuth, then it's one click.",
        "Import events from Google Calendar to auto-fill recording titles. No retyping meeting names.",
        "Export as Markdown, plain text, SRT, or PDF — share and archive in the formats you already use.",
        "API keys and tokens are stored in the macOS Keychain, never in plain-text config files.",
      ],
      image: "/screenshots/integrations.png",
      screenshotCaption:
        "One-click export from your notes to Notion and Slack. Tokens are kept safely in the Keychain.",
      label: "Integrations",
      badge: "NEW" as string | null,
    },
  ],

  mcp: {
    eyebrow: "Ask Claude about your meetings",
    heading: "“What did we decide last week?” Just ask.",
    body: "mojiroku ships with a built-in MCP server. MCP (Model Context Protocol) is how AI assistants connect to outside sources of information — in this case, the meeting notes stored on your device. Claude Desktop and Claude Code can search and read them directly. No pasting documents back and forth: ask Claude “what did we conclude about last month's budget?” and it works through your past meetings and answers. Access is read-only — the AI can never rewrite your notes. It's a workflow that only makes sense because everything stays local.",
    bullets: [
      "Search across meetings: “who owned that task again?” gets answered from multiple sets of notes (search_meetings).",
      "Read-only by design. Claude can view and quote your records — never edit or delete them.",
      "Data stays on-device. MCP connects to the notes inside your Mac; recordings and transcripts go nowhere.",
    ],
    demo: {
      user: "Summarize what we decided at last week's kickoff — three points.",
      toolCall: 'search_meetings("kickoff")',
      toolResult: "2 hits — “Product kickoff 6/24” · “Sales sync 6/26”",
      assistant:
        "From the 6/24 kickoff: (1) the first release stays limited to an internal beta, (2) public launch targets August, and (3) Sam owns the distribution setup.",
    },
    demoCaption:
      "Example: asking Claude about your own meeting notes via the built-in MCP server",
  },

  comparison: {
    eyebrow: "On-device vs cloud",
    heading: "Choose by where your audio is processed.",
    body: "What really separates meeting tools isn't the feature count — it's where your voice gets processed. mojiroku transcribes and summarizes entirely on your Mac. The only thing that can ever leave the device is transcript text, and only if you choose cloud summaries with your own API key (the audio itself stays put). Here's how that compares with the typical cloud setup, across six dimensions.",
    cols: {
      a: { name: "mojiroku", sub: "on-device" },
      b: { name: "Cloud tools", sub: "typical pattern" },
    },
    rows: [
      { k: "Processing location", a: "On your Mac", b: "The vendor's servers", highlight: false },
      { k: "Transcript uploads", a: "None by default (opt-in sends transcript text only)", b: "Uploading is the premise", highlight: true },
      { k: "Usage limits", a: "No caps on recordings or history", b: "Free tiers typically cap minutes or files", highlight: false },
      { k: "Meeting capture", a: "System audio captured on-device (no bot joins)", b: "Usually a bot joins the call", highlight: false },
      { k: "Fully offline (airplane mode)", a: "Every local feature works", b: "Generally not possible", highlight: true },
      { k: "Search notes from Claude (MCP)", a: "Built in", b: "Generally unsupported", highlight: true },
    ],
    notes:
      "The “Cloud tools” column describes patterns commonly seen across this category of services; it does not refer to any specific product. For actual data handling, limits, and capture methods, please check each vendor's official documentation.",
  },

  privacy: {
    eyebrow: "Privacy",
    heading: "Your voice never leaves your Mac.",
    body: "Recording, transcription, summarization — all of it happens inside this Mac. Transcripts and notes are generated by local AI models running on your own hardware. Your data lives only on your device, as SQLite and audio files under ~/Library/Application Support, and uninstalling removes it. Every local feature works in airplane mode. Your meeting content leaves the device only when you choose cloud summaries or an integration. Meeting notes shouldn't be something you deposit with a vendor — they should be something you own.",
    bullets: [
      "Stored on-device by default. Recordings, transcripts, summaries, and models live only on your Mac — nothing reaches an external server unless you choose cloud summaries or an export.",
      "Local features work in airplane mode. Apart from fetching models and checking for updates, networking happens only when you choose it — cloud summaries (BYOK), calendar import, or export.",
      "Sending is opt-in. Only if you choose BYOK does mojiroku send transcript text to the provider you picked (Claude / OpenAI), with your own API key. Local summaries are the default.",
      "Secrets go in the Keychain. API keys and integration tokens are stored in the macOS Keychain, not in plain-text config files.",
    ],
    source: {
      text: "You don't have to take any of this on trust. The full source is published, so you can check for yourself that nothing leaves your Mac.",
      label: "View the source on GitHub",
      note: "AGPL-3.0",
      href: "https://github.com/daichi-hotta/mojiroku",
    },
  },

  howItWorks: {
    eyebrow: "How it works",
    heading: "Three steps to meeting notes.",
    items: [
      {
        title: "Download and open",
        body: "Install a ~12 MB .dmg and launch. No account, no sign-in. Runs on Apple Silicon (M1 or later) with macOS 11 or newer.",
      },
      {
        title: "Models download on first run",
        body: "The first time you open it, mojiroku automatically fetches the transcription and summarization models (a few GB in total). Once they're in, everything runs on-device — your voice never leaves the Mac.",
      },
      {
        title: "Record or import — get your notes",
        body: "Record from the mic or drop in an audio file. mojiroku transcribes locally, separates the speakers, and writes up the notes. Everything is saved on-device.",
      },
    ],
  },

  pricing: {
    eyebrow: "Free to use · $0 to run",
    heading: "Free. No subscription, no storage caps.",
    body: "The app, transcription, and summaries all run on-device by default. Recordings and notes are stored in SQLite, on your Mac alone. No server of ours ever holds your meeting data, so it costs $0 to operate — which is exactly why it can be free. Your meeting content reaches the cloud only when you choose BYOK summaries (Anthropic / OpenAI) or export to Notion / Slack. Every local feature works in airplane mode.",
    freeLabel: "Free to use",
    price: "$0",
    priceNote: "/ zero running costs",
    bullets: [
      "The app and local AI are free. Choose cloud summaries and you pay the provider directly, with your own API key.",
      "Data stays under ~/Library on your device. Only if you pick cloud summaries is that content sent — to the provider you chose.",
      "The built-in MCP server lets Claude Desktop / Claude Code search and reference your notes (read-only).",
    ],
    cta: { label: "Download for macOS", href: "/download" },
  },

  faq: {
    heading: "Frequently asked questions",
    items: [
      {
        q: "Can I open the app right after downloading it?",
        a: "Yes. mojiroku is signed with an Apple Developer ID and notarized by Apple. Drag it from the .dmg into your Applications folder and it opens with a double-click — no warnings. On first launch, macOS will ask for permissions such as microphone access; that's standard behavior. Signing doesn't change the price: the app and local AI remain free.",
      },
      {
        q: "What are the system requirements?",
        a: "An Apple Silicon Mac (M1 or later) running macOS 11 or newer. Transcription and summarization run locally on Apple's Metal, so every local feature works even without a network connection. Intel Macs are not supported.",
      },
      {
        q: "How long does the first-run model download take?",
        a: "On first launch, mojiroku downloads whisper for transcription (~547 MB), a local summary model (~4.7-5.7 GB, chosen to fit your Mac's memory), and Silero VAD for silence detection (~864 KB) — a few GB in total. Depending on your connection it takes a little while, but only once. The models are stored on-device and work offline from then on.",
      },
      {
        q: "Does it work without internet?",
        a: "Yes. Transcription, local summaries, speaker diarization, full-text search, meeting mode — all of it runs on-device. Apart from fetching models and checking for updates, the network is used only when you choose cloud summaries (Claude / OpenAI), calendar import, or export to Notion / Slack. Every other local feature works even in airplane mode.",
      },
      {
        q: "Where is my data stored?",
        a: "Only on your Mac (~/Library/Application Support/com.daichi0812.mojiroku/). Recordings, transcripts, summaries, and models live in a local SQLite database and files — none of it is sent to our servers, because there aren't any. Uninstall the app and the data goes with it.",
      },
      {
        q: "Do cloud summaries cost money?",
        a: "The app and local summaries (on-device) are free. If you opt into cloud summaries from Claude or OpenAI for higher quality, you use your own API key and pay the provider directly. Only in that case is the transcript sent to the provider you chose. Skip the cloud, and there's no cost and no data transfer.",
      },
      {
        q: "Is a Windows version planned?",
        a: "mojiroku is currently macOS-only (Apple Silicon). Transcription and summarization are built on Apple's Metal, and there's no Windows timeline we can share yet. For now, we're focused on making the Mac experience excellent.",
      },
    ],
  },

  download: {
    eyebrow: "Download",
    heading: "Start free, right now.",
    body: "Recording, transcription, summaries, meeting notes — all generated on-device by local AI. Your data stays in your macOS user folder (sent out only if you choose cloud summaries with your own key). The app and local AI are free. No account required. Download the ~12 MB .dmg; the models you need are fetched automatically on first launch.",
    cta: { label: "Download for macOS", href: "/download" },
    meta: "Apple Silicon Mac (M1 or later) · macOS 11+ · ~12 MB",
    stepsHeading: "Install (first time only)",
    steps: [
      {
        n: 1,
        title: "Open the .dmg and drag to Applications",
        body: "Open the downloaded .dmg and drag mojiroku into your Applications folder.",
        code: null as string | null,
      },
      {
        n: 2,
        title: "Double-click to launch",
        body: "mojiroku is Developer ID–signed and notarized, so there's no “damaged” warning — it just opens.",
        code: null as string | null,
      },
      {
        n: 3,
        title: "Models download on first launch",
        body: "The first launch fetches the transcription and summary models (a few GB), once. When you start recording, macOS asks for microphone access — click Allow.",
        code: null as string | null,
      },
    ],
  },

  footer: {
    ariaLabel: "Footer",
    tagline:
      "A meeting-notes app that records, transcribes, and summarizes entirely on-device. Free to use, $0 to run. Your meeting content leaves the device only when you choose to.",
    links: [
      {
        title: "Download",
        body: "Signed & notarized .dmg (~12 MB)",
        href: "/download",
      },
      {
        title: "GitHub Releases",
        body: "Releases and changelog",
        href: "https://github.com/daichi-hotta/mojiroku-releases/releases",
      },
      {
        title: "Source code",
        body: "Published under AGPL-3.0",
        href: "https://github.com/daichi-hotta/mojiroku",
      },
    ],
    year: 2026,
    copyright: "mojiroku — free, on-device meeting notes",
    privacy: { label: "Privacy Policy", href: "/en/privacy/" },
  },

  screenshotFrame: {
    defaultLabel: "App screen",
  },
};

export default en;

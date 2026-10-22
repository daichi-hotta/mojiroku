// 共有 UI プリミティブ（ダーク Studio）。全ビューはここを組み合わせて作る。
import {
  useEffect,
  useRef,
  useState,
  type ButtonHTMLAttributes,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { useI18n } from "@/i18n";
import { cx } from "@/lib/cx";
import { ChevronDownIcon, XIcon } from "./icons";

// ── Button ──────────────────────────────────────────────────────────────
type ButtonVariant = "primary" | "secondary" | "ghost" | "danger" | "destructive";
type ButtonSize = "sm" | "md";

const BTN_SIZE: Record<ButtonSize, string> = {
  sm: "h-8 px-3 text-[13px] gap-1.5",
  md: "h-10 px-4 text-[13px] gap-2",
};

const BTN_VARIANT: Record<ButtonVariant, string> = {
  primary: "bg-brand-gradient text-white shadow-cta hover:brightness-110",
  secondary: "bg-surface-2 border border-border-2 text-ink hover:bg-hover",
  ghost: "text-sub hover:bg-hover hover:text-ink",
  danger: "text-red-light hover:bg-red/12",
  destructive: "bg-danger text-white shadow-[0_10px_26px] shadow-danger/30 hover:brightness-110",
};

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  icon?: ReactNode;
}

export function Button({
  variant = "secondary",
  size = "md",
  icon,
  className,
  children,
  style,
  ...rest
}: ButtonProps) {
  return (
    <button
      {...rest}
      style={style}
      className={cx(
        "inline-flex items-center justify-center rounded-btn font-medium transition-colors",
        "disabled:cursor-not-allowed disabled:opacity-45",
        BTN_SIZE[size],
        BTN_VARIANT[variant],
        className,
      )}
    >
      {icon}
      {children}
    </button>
  );
}

export interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  label: string;
}

export function IconButton({ label, className, children, ...rest }: IconButtonProps) {
  return (
    <button
      {...rest}
      aria-label={label}
      title={label}
      className={cx(
        "inline-flex h-8 w-8 items-center justify-center rounded-btn text-sub",
        "transition-colors hover:bg-hover hover:text-ink disabled:opacity-45",
        className,
      )}
    >
      {children}
    </button>
  );
}

// ── Badge ───────────────────────────────────────────────────────────────
type Tone = "indigo" | "green" | "cyan" | "amber" | "red" | "neutral" | "purple";

const BADGE_TONE: Record<Tone, string> = {
  indigo: "text-brand-lighter bg-brand/14",
  green: "text-green bg-green/13",
  cyan: "text-cyan bg-cyan/13",
  amber: "text-amber bg-amber/14",
  red: "text-red-light bg-red/13",
  purple: "text-purple bg-purple/14",
  neutral: "text-sub bg-hover",
};

export function Badge({
  tone = "neutral",
  className,
  children,
}: {
  tone?: Tone;
  className?: string;
  children: ReactNode;
}) {
  return (
    <span
      className={cx(
        "inline-flex items-center gap-1 rounded-md px-2 py-0.5 text-[11px] font-medium",
        BADGE_TONE[tone],
        className,
      )}
    >
      {children}
    </span>
  );
}

// ── StatusBadge（設定・連携の状態表示。枠付きの小バッジ） ──────────────────
type StatusBadgeTone = "green" | "amber" | "neutral";

const STATUS_BADGE_TONE: Record<StatusBadgeTone, string> = {
  green: "border-green/25 bg-green/10 text-green",
  amber: "border-amber/25 bg-amber/10 text-amber",
  neutral: "border-border-2 bg-surface-2 text-sub",
};

export function StatusBadge({
  tone,
  children,
}: {
  tone: StatusBadgeTone;
  children: ReactNode;
}) {
  return (
    <span
      className={cx(
        "rounded-md border px-1.5 py-0.5 text-[11px]",
        STATUS_BADGE_TONE[tone],
      )}
    >
      {children}
    </span>
  );
}

// ── Chip（選択可能なフィルタ等） ─────────────────────────────────────────
export function Chip({
  active = false,
  onClick,
  className,
  children,
}: {
  active?: boolean;
  onClick?: () => void;
  className?: string;
  children: ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      className={cx(
        "rounded-full border px-3 py-1 text-[13px] transition-colors",
        active
          ? "border-brand/60 bg-brand/16 text-brand-lighter"
          : "border-border-2 text-sub hover:bg-hover hover:text-body",
        className,
      )}
    >
      {children}
    </button>
  );
}

// ── Toggle（スイッチ） ───────────────────────────────────────────────────
export function Toggle({
  checked,
  onChange,
  disabled,
  label,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
  disabled?: boolean;
  label?: string;
}) {
  return (
    <button
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cx(
        "relative h-6 w-11 shrink-0 rounded-full transition-colors disabled:opacity-50",
        checked ? "bg-brand" : "bg-border-3",
      )}
    >
      <span
        className={cx(
          "absolute top-0.5 h-5 w-5 rounded-full bg-white shadow transition-all",
          checked ? "left-[22px]" : "left-0.5",
        )}
      />
    </button>
  );
}

// ── Card / Panel ─────────────────────────────────────────────────────────
export function Card({
  className,
  children,
}: {
  className?: string;
  children: ReactNode;
}) {
  return (
    <div className={cx("rounded-card border border-border bg-surface-2", className)}>
      {children}
    </div>
  );
}

// ── Radio ────────────────────────────────────────────────────────────────
export function Radio({
  checked,
  onChange,
  title,
  desc,
  className,
}: {
  checked: boolean;
  onChange: () => void;
  title: ReactNode;
  desc?: ReactNode;
  className?: string;
}) {
  return (
    <button
      onClick={onChange}
      className={cx(
        "flex w-full items-start gap-3 rounded-card border px-4 py-3 text-left transition-colors",
        checked
          ? "border-brand/60 bg-selected"
          : "border-border-2 bg-surface-2 hover:bg-hover",
        className,
      )}
    >
      <span
        className={cx(
          "mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded-full border",
          checked ? "border-brand" : "border-border-3",
        )}
      >
        {checked && <span className="h-2 w-2 rounded-full bg-brand" />}
      </span>
      <span className="min-w-0">
        <span className="block text-[13px] font-medium text-ink">{title}</span>
        {desc && <span className="mt-0.5 block text-[12px] text-muted">{desc}</span>}
      </span>
    </button>
  );
}

// ── ProgressBar ──────────────────────────────────────────────────────────
export function ProgressBar({
  value,
  tone = "indigo",
  className,
}: {
  value: number; // 0..1
  tone?: "indigo" | "green";
  className?: string;
}) {
  const pct = Math.max(0, Math.min(1, value)) * 100;
  return (
    <div className={cx("h-1.5 w-full overflow-hidden rounded-full bg-hover", className)}>
      <div
        className={cx(
          "h-full rounded-full transition-all",
          tone === "green" ? "bg-green" : "bg-brand",
        )}
        style={{ width: `${pct}%` }}
      />
    </div>
  );
}

// ── Spinner ──────────────────────────────────────────────────────────────
export function Spinner({ size = 18, className }: { size?: number; className?: string }) {
  return (
    <span
      className={cx("inline-block animate-mjspin rounded-full", className)}
      style={{
        width: size,
        height: size,
        border: "2px solid var(--color-border-3)",
        borderTopColor: "var(--color-brand-light)",
      }}
    />
  );
}

// ── Kbd ──────────────────────────────────────────────────────────────────
export function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="rounded border border-border-3 bg-surface px-1.5 py-0.5 font-mono text-[11px] text-sub">
      {children}
    </kbd>
  );
}

// ── SectionLabel ─────────────────────────────────────────────────────────
export function SectionLabel({ children }: { children: ReactNode }) {
  return (
    <div className="text-[11px] font-medium uppercase tracking-[0.08em] text-dim">
      {children}
    </div>
  );
}

// ── Modal（中央・backdrop blur） ─────────────────────────────────────────
export function Modal({
  open,
  onClose,
  children,
  width = 452,
}: {
  open: boolean;
  onClose: () => void;
  children: ReactNode;
  width?: number;
}) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;
  return createPortal(
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-6 backdrop-blur-sm"
      onClick={onClose}
    >
      <div
        role="dialog"
        aria-modal="true"
        className="animate-mjfade w-full rounded-win border border-border-3 bg-surface shadow-modal"
        style={{ maxWidth: width }}
        onClick={(e) => e.stopPropagation()}
      >
        {children}
      </div>
    </div>,
    document.body,
  );
}

export function ModalHeader({
  title,
  onClose,
}: {
  title: ReactNode;
  onClose: () => void;
}) {
  const { t } = useI18n();
  return (
    <div className="flex items-center justify-between border-b border-border px-5 py-3.5">
      <h3 className="text-[15px] font-bold text-ink">{title}</h3>
      <IconButton label={t.common.close} onClick={onClose}>
        <XIcon size={16} />
      </IconButton>
    </div>
  );
}

// ── ConfirmDialog（取り消し不可な操作の確認） ───────────────────────────────
// 削除など一度きりの操作の前に一段挟む。busy 中は backdrop/Esc で閉じない。
export function ConfirmDialog({
  open,
  title,
  body,
  confirmLabel,
  cancelLabel,
  busy = false,
  tone = "danger",
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: ReactNode;
  body?: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  busy?: boolean;
  /** 確定ボタンの色。取り消せない操作は danger（既定）、それ以外は primary。 */
  tone?: "danger" | "primary";
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const { t } = useI18n();
  // 既定文言は描画時に辞書から引く（destructuring 既定値ではフックを参照できない）。
  const confirmText = confirmLabel ?? t.ui.confirmDelete;
  const cancelText = cancelLabel ?? t.common.cancel;
  return (
    <Modal open={open} onClose={busy ? () => {} : onCancel} width={400}>
      <div className="px-5 py-5">
        <h3 className="text-[15px] font-bold text-ink">{title}</h3>
        {body && <p className="mt-2 text-[13px] leading-relaxed text-muted">{body}</p>}
        <div className="mt-5 flex justify-end gap-2">
          <Button variant="secondary" size="sm" onClick={onCancel} disabled={busy}>
            {cancelText}
          </Button>
          <button
            onClick={onConfirm}
            disabled={busy}
            className={cx(
              "inline-flex h-8 items-center justify-center gap-1.5 rounded-btn px-3.5 text-[13px] font-semibold text-white",
              tone === "danger" ? "bg-danger" : "bg-brand",
              "transition-[filter] hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-60",
            )}
          >
            {busy && <Spinner size={13} />}
            {confirmText}
          </button>
        </div>
      </div>
    </Modal>
  );
}

// ── Drawer（右からスライド） ─────────────────────────────────────────────
export function Drawer({
  open,
  onClose,
  children,
  width = 420,
}: {
  open: boolean;
  onClose: () => void;
  children: ReactNode;
  width?: number;
}) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;
  return createPortal(
    <div className="fixed inset-0 z-50 flex justify-end bg-black/50" onClick={onClose}>
      <div
        role="dialog"
        aria-modal="true"
        className="animate-mjdrawer flex h-full flex-col border-l border-border-3 bg-surface shadow-modal"
        style={{ width }}
        onClick={(e) => e.stopPropagation()}
      >
        {children}
      </div>
    </div>,
    document.body,
  );
}

// ── Popover（トリガーにアンカーするメニュー） ──────────────────────────────
export function Popover({
  trigger,
  children,
  align = "right",
  width = 312,
}: {
  /** 開閉状態を受け取って描画するトリガー。 */
  trigger: (args: { open: boolean; toggle: () => void }) => ReactNode;
  children: ReactNode | ((close: () => void) => ReactNode);
  align?: "left" | "right";
  width?: number;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const close = () => setOpen(false);
  return (
    <div ref={ref} className="relative inline-flex">
      {trigger({ open, toggle: () => setOpen((v) => !v) })}
      {open && (
        <div
          className={cx(
            "animate-mjfade absolute top-full z-40 mt-2 rounded-card border border-border-3 bg-popover p-1.5 shadow-pop",
            align === "right" ? "right-0" : "left-0",
          )}
          style={{ width }}
        >
          {typeof children === "function" ? children(close) : children}
        </div>
      )}
    </div>
  );
}

/** Popover 内の 1 行アクション。 */
export function MenuItem({
  icon,
  children,
  onClick,
  hint,
}: {
  icon?: ReactNode;
  children: ReactNode;
  onClick?: () => void;
  hint?: ReactNode;
}) {
  return (
    <button
      onClick={onClick}
      className="flex w-full items-center gap-2.5 rounded-ctl px-2.5 py-2 text-left text-[13px] text-body transition-colors hover:bg-popover-2"
    >
      {icon && <span className="text-sub">{icon}</span>}
      <span className="min-w-0 flex-1 truncate">{children}</span>
      {hint && <span className="text-muted">{hint}</span>}
    </button>
  );
}

export function DropdownCaret() {
  return <ChevronDownIcon size={15} className="text-sub" />;
}

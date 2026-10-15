import { ArrowClockwise, CheckCircle, FloppyDisk, Info, WarningCircle } from "@phosphor-icons/react";
import { useEffect, useRef, useState } from "react";
import { useT } from "../../i18n/useT";
import { cx } from "../../lib/cx";
import { deviceErrorMessage, errorCode } from "../../lib/deviceErrors";
import { profileTitle, snapDpi } from "../../lib/device";
import { refreshDevice, saveProfileToMouse } from "../../lib/ipc";
import type { DeviceDetail, ProfileInfo } from "../../lib/types";
import { Button, Panel } from "../../ui";

type Result =
  | { kind: "ok"; outcome: "written" | "unchanged"; backupPath: string | null; reloaded: boolean }
  | { kind: "error"; text: string }
  | null;

interface Draft {
  /** Raw text per level; slider and input both edit it. */
  texts: string[];
  defaultIndex: number;
  shiftIndex: number;
  rateHz: number | null;
}

function baseDraft(profile: ProfileInfo): Draft {
  return {
    texts: profile.dpiLevels.map(String),
    defaultIndex: profile.defaultDpiIndex,
    shiftIndex: profile.shiftDpiIndex,
    rateHz: profile.reportRateHz,
  };
}

/** Changes whenever the values stored in the mouse change, so the draft can follow them. */
function baseSignature(profile: ProfileInfo): string {
  return [profile.number, profile.dpiLevels.join(","), profile.defaultDpiIndex, profile.shiftDpiIndex, profile.reportRateHz].join("|");
}

/**
 * Editor for the active MyHUB profile stored in the mouse flash. Edits only stage a local draft. The one
 * action that writes is the "Save to mouse" button; nothing is sent on slider drag or input change.
 */
export function FlashProfileEditor({
  detail,
  profile,
  onDetail,
}: {
  detail: DeviceDetail;
  profile: ProfileInfo;
  onDetail: (next: DeviceDetail) => void;
}) {
  const t = useT();
  const dpi = detail.dpi;
  const rate = detail.reportRate;
  const locked = detail.readOnly;
  const signature = baseSignature(profile);

  const [draft, setDraft] = useState<Draft>(() => baseDraft(profile));
  const [pending, setPending] = useState(false);
  const [result, setResult] = useState<Result>(null);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  // Follow the mouse when its stored values change (refresh, save, other profile).
  useEffect(() => {
    setDraft(baseDraft(profile));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [signature]);

  const parsed = draft.texts.map((text) => (/^\d+$/.test(text.trim()) ? Number(text.trim()) : null));
  const levelValid = parsed.map((n) => n !== null && !!dpi && n >= dpi.min && n <= dpi.max);
  const allValid = levelValid.every(Boolean);
  const snapped = parsed.map((n) => (n !== null && dpi ? snapDpi(n, dpi.min, dpi.max, dpi.step) : null));

  const levelsChanged = snapped.some((n, i) => n !== profile.dpiLevels[i]);
  const defaultChanged = draft.defaultIndex !== profile.defaultDpiIndex;
  const shiftChanged = draft.shiftIndex !== profile.shiftDpiIndex;
  const rateChanged = draft.rateHz !== profile.reportRateHz;
  const dirty = levelsChanged || defaultChanged || shiftChanged || rateChanged;
  const canSave = !locked && !pending && !!dpi && allValid && dirty;

  function patch(next: Partial<Draft>) {
    setDraft((d) => ({ ...d, ...next }));
    setResult(null);
  }

  function setText(index: number, text: string) {
    patch({ texts: draft.texts.map((old, i) => (i === index ? text : old)) });
  }

  function snapText(index: number) {
    const n = snapped[index];
    if (n !== null && n !== undefined) setText(index, String(n));
  }

  function revert() {
    if (pending) return;
    setDraft(baseDraft(profile));
    setResult(null);
  }

  function save() {
    if (!canSave) return;
    const levels = snapped.filter((n): n is number => n !== null);
    setDraft((d) => ({ ...d, texts: levels.map(String) }));
    setPending(true);
    setResult(null);
    // Only the fields that differ are sent; the backend keeps everything else as read.
    saveProfileToMouse({
      id: detail.id,
      ...(levelsChanged ? { dpiLevels: levels } : {}),
      ...(defaultChanged ? { defaultDpiIndex: draft.defaultIndex } : {}),
      ...(shiftChanged ? { shiftDpiIndex: draft.shiftIndex } : {}),
      ...(rateChanged && draft.rateHz !== null ? { reportRateHz: draft.rateHz } : {}),
    }).then(
      (res) => {
        if (!alive.current) return;
        setPending(false);
        setResult({ kind: "ok", outcome: res.outcome, backupPath: res.backupPath, reloaded: res.reloaded });
        onDetail(res.detail);
      },
      (error: unknown) => {
        if (!alive.current) return;
        setPending(false);
        const code = errorCode(error);
        const { key, vars } = deviceErrorMessage(error);
        // A timeout means the state is unknown, never "nothing written". Re-read in these cases.
        const timeout = code === "deviceError:timeout";
        setResult({ kind: "error", text: timeout ? t("err.saveTimeout") : t(key, vars) });
        if (timeout || code.startsWith("savedReselectFailed") || code.startsWith("flashWriteFailed:")) {
          refreshDevice(detail.id).then(
            (next) => {
              if (next && alive.current) onDetail(next);
            },
            () => {},
          );
        }
      },
    );
  }

  const busy = pending || locked;
  const activeNumber = detail.onboard?.activeProfile ?? null;
  return (
    <Panel
      title={activeNumber != null ? t("flash.titleActive", { n: activeNumber }) : t("flash.title")} icon={<FloppyDisk size={18} />}>
      <div className="flex flex-col gap-4 px-4 pt-2 pb-4" aria-busy={pending}>
        <p className="text-sm text-text-muted">{t("flash.hint", { n: profile.number })}</p>

        {dpi ? (
          <div className="flex items-baseline gap-2" aria-live="polite">
            <span className="text-xs text-text-muted">{t("flash.currentDpi")}</span>
            <span className="font-mono text-lg font-light tabular-nums text-text">{dpi.current}</span>
          </div>
        ) : null}

        {dpi ? (
          <div role="group" aria-label={t("perf.stages")} className="flex flex-col gap-1">
            <div className="grid grid-cols-[3.5rem_1fr_5rem_3.5rem_3.5rem] items-center gap-x-3 text-xs text-text-faint">
              <span />
              <span />
              <span />
              <span className="text-center">{t("flash.default")}</span>
              <span className="text-center">{t("flash.shift")}</span>
            </div>
            {draft.texts.map((text, index) => {
              const n = index + 1;
              // The marker follows the values stored in the mouse, not the draft.
              const isCurrent = !!dpi && profile.dpiLevels.indexOf(dpi.current) === index;
              const valid = levelValid[index] ?? false;
              const value = snapped[index] ?? profile.dpiLevels[index] ?? dpi.min;
              return (
                <div
                  key={index}
                  aria-current={isCurrent ? "true" : undefined}
                  className={cx(
                    "relative -mx-2 grid grid-cols-[3.5rem_1fr_5rem_3.5rem_3.5rem] items-center gap-x-3 rounded-control px-2 py-1 transition-colors duration-150 ease-[var(--ease-out)] motion-reduce:transition-none",
                    isCurrent && "bg-accent/10",
                  )}
                >
                  <span
                    aria-hidden="true"
                    className={cx(
                      "absolute inset-y-1 left-0 w-0.5 rounded-pill bg-accent transition-opacity duration-150 ease-[var(--ease-out)] motion-reduce:transition-none",
                      isCurrent ? "opacity-100" : "opacity-0",
                    )}
                  />
                  <span className="flex items-center gap-1.5">
                    <span className={cx("font-mono text-sm tabular-nums", isCurrent ? "text-accent" : "text-text-faint")}>{n}</span>
                    <span
                      className={cx(
                        "text-xs font-medium text-accent transition-opacity duration-150 ease-[var(--ease-out)] motion-reduce:transition-none",
                        isCurrent ? "opacity-100" : "opacity-0",
                      )}
                    >
                      <span aria-hidden="true">{t("flash.now")}</span>
                      {isCurrent ? <span className="sr-only">{t("flash.nowFull", { n: dpi?.current ?? 0 })}</span> : null}
                    </span>
                  </span>
                  <input
                    type="range"
                    aria-label={t("flash.level", { n })}
                    min={dpi.min}
                    max={dpi.max}
                    step={dpi.step}
                    value={value}
                    disabled={busy}
                    onChange={(event) => setText(index, event.currentTarget.value)}
                    className="h-5 w-full min-w-0 accent-accent disabled:opacity-50"
                  />
                  <input
                    type="text"
                    inputMode="numeric"
                    autoComplete="off"
                    aria-label={t("flash.level", { n })}
                    aria-invalid={!valid}
                    value={text}
                    disabled={busy}
                    onChange={(event) => setText(index, event.currentTarget.value)}
                    onBlur={() => snapText(index)}
                    onKeyDown={(event) => {
                      // Enter only normalizes the value; saving stays an explicit button click.
                      if (event.key === "Enter") {
                        event.preventDefault();
                        snapText(index);
                      }
                    }}
                    className={cx(
                      "h-8 w-full rounded-control border bg-surface-2 px-2 text-right font-mono text-sm tabular-nums text-text disabled:opacity-50",
                      valid ? "border-border" : "border-danger",
                    )}
                  />
                  <Radio
                    name={`${detail.id}-default`}
                    label={t("flash.defaultFor", { n })}
                    checked={draft.defaultIndex === index}
                    disabled={busy}
                    onSelect={() => patch({ defaultIndex: index })}
                  />
                  <Radio
                    name={`${detail.id}-shift`}
                    label={t("flash.shiftFor", { n })}
                    checked={draft.shiftIndex === index}
                    disabled={busy}
                    onSelect={() => patch({ shiftIndex: index })}
                  />
                </div>
              );
            })}
            {!allValid ? <p className="text-sm text-danger">{t("flash.invalid", { min: dpi.min, max: dpi.max })}</p> : null}
          </div>
        ) : null}

        {rate ? (
          <div className="flex flex-col gap-2">
            <h3 className="text-sm font-medium text-text-muted">{t("perf.rate")}</h3>
            <ul aria-label={t("perf.rate")} className="inline-grid auto-cols-fr grid-flow-col self-start rounded-control border border-border bg-surface-2 p-0.5">
              {rate.optionsHz.map((hz) => {
                const selected = draft.rateHz === hz;
                return (
                  <li key={hz}>
                    <button
                      type="button"
                      aria-pressed={selected}
                      disabled={busy}
                      onClick={() => patch({ rateHz: hz })}
                      className={cx(
                        "pressable flex h-7 w-full items-center justify-center rounded-[6px] border px-3 font-mono text-sm tabular-nums whitespace-nowrap disabled:opacity-50",
                        selected
                          ? "border-accent/40 bg-accent/15 text-accent"
                          : "border-transparent text-text-muted hover-fine:text-text",
                      )}
                    >
                      {hz} Hz
                    </button>
                  </li>
                );
              })}
            </ul>
          </div>
        ) : null}

        <div className="flex flex-wrap items-center gap-3">
          <Button variant="primary" aria-busy={pending} disabled={!canSave} onClick={save}>
            {pending ? (
              <>
                <ArrowClockwise size={16} weight="regular" aria-hidden="true" className="animate-spin-slow" />
                {t("flash.saving")}
              </>
            ) : (
              t("flash.save")
            )}
          </Button>
          <Button disabled={!dirty || pending} onClick={revert}>
            {t("flash.revert")}
          </Button>
        </div>

        {/* Live regions stay mounted so screen readers announce the result when it appears. */}
        <div className="min-h-5 text-sm">
          <div role="status" className={cx(result?.kind === "ok" ? "" : "hidden")}>
            <p className="flex items-center gap-1.5 text-accent">
              <CheckCircle size={16} weight="regular" aria-hidden="true" className="shrink-0" />
              <span>{result?.kind === "ok" ? t(result.outcome === "written" ? "flash.saved" : "flash.unchanged") : ""}</span>
            </p>
            {result?.kind === "ok" && result.outcome === "written" && !result.reloaded ? (
              <p className="mt-1 text-xs text-text-muted">{t("flash.notReloaded")}</p>
            ) : null}
            {result?.kind === "ok" && result.backupPath ? (
              <p className="mt-1 text-xs break-all text-text-faint">{t("flash.backup", { path: result.backupPath })}</p>
            ) : null}
          </div>
          <p role="alert" className={cx("flex items-center gap-1.5 text-danger", result?.kind === "error" ? "" : "hidden")}>
            <WarningCircle size={16} weight="regular" aria-hidden="true" className="shrink-0" />
            <span>{result?.kind === "error" ? result.text : ""}</span>
          </p>
        </div>
      </div>
    </Panel>
  );
}

function Radio({
  name,
  label,
  checked,
  disabled,
  onSelect,
}: {
  name: string;
  label: string;
  checked: boolean;
  disabled: boolean;
  onSelect: () => void;
}) {
  return (
    <span className="flex justify-center">
      <input
        type="radio"
        name={name}
        aria-label={label}
        checked={checked}
        disabled={disabled}
        onChange={onSelect}
        className="size-4 accent-accent disabled:opacity-50"
      />
    </span>
  );
}

/** Shown instead of the editor when the active profile cannot be written: what is active and why saving is off. */
export function FlashUnavailableNote({ detail }: { detail: DeviceDetail }) {
  const t = useT();
  const onboard = detail.onboard;
  const active = onboard?.profiles.find((p) => !p.rom && p.number === onboard.activeProfile) ?? null;
  const which =
    onboard?.mode === "host"
      ? t("flash.notWritableHost")
      : active
        ? t("flash.notWritableActive", { profile: profileTitle(active, t) })
        : t("flash.noActive");
  return (
    <p className="flex items-start gap-2 text-sm text-text-muted">
      <Info size={16} weight="regular" aria-hidden="true" className="mt-px shrink-0" />
      <span className="min-w-0">
        {t("flash.notWritable")} {which}
      </span>
    </p>
  );
}

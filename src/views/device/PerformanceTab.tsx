import { CheckCircle, Lock, WarningCircle } from "@phosphor-icons/react";
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { useT } from "../../i18n/useT";
import { cx } from "../../lib/cx";
import { deviceErrorMessage } from "../../lib/deviceErrors";
import { activeUserProfile, snapDpi } from "../../lib/device";
import { setDpi, setReportRate } from "../../lib/ipc";
import type { DeviceDetail } from "../../lib/types";
import { useSettings } from "../../state/settings";
import { ModeSwitch } from "../../components/device/ModeSwitch";
import { FlashProfileEditor, FlashUnavailableNote } from "./FlashProfileEditor";
import { Badge, Button, Panel, SegmentedControl } from "../../ui";
import { DpiIcon, ReportRateIcon } from "../../ui/icons";

interface PerformanceTabProps {
  detail: DeviceDetail;
  /** Receives the fresh detail returned by a successful write. */
  onDetail: (next: DeviceDetail) => void;
  onOpenSettings: () => void;
}

type Feedback = { kind: "pending" | "ok" | "error"; text: string } | null;

/**
 * Performance tab. A plain switch picks the mode: the profile in the mouse (flash profile editor) or
 * control from the app (host mode, DPI and report rate set by the app).
 */
export function PerformanceTab({ detail, onDetail, onOpenSettings }: PerformanceTabProps) {
  const t = useT();
  const { dpi, reportRate } = detail;
  const profile = activeUserProfile(detail);
  const editable = !detail.readOnly;
  const host = detail.onboard?.mode === "host";
  const writable = detail.onboard?.writableProfile ?? null;
  const flashProfile =
    writable !== null ? (detail.onboard?.profiles.find((p) => !p.rom && p.number === writable) ?? null) : null;
  const hostEditors = host && editable;

  return (
    <div className="flex flex-col gap-4">
      <ModeSwitch detail={detail} onDetail={onDetail} />

      {hostEditors ? (
        <>
          <Panel title={t("perf.dpi")} icon={<DpiIcon size={18} />}>
            <div className="flex flex-col gap-4 px-4 pt-2 pb-4">
              {dpi ? (
                <DpiEditor
                  key={detail.id}
                  id={detail.id}
                  dpi={dpi}
                  stages={profile?.dpiLevels ?? []}
                  defaultIndex={profile?.defaultDpiIndex ?? -1}
                  profileNumber={profile?.number ?? null}
                  onDetail={onDetail}
                />
              ) : (
                <p className="text-base text-text-muted">--</p>
              )}
            </div>
          </Panel>
          <Panel title={t("perf.rate")} icon={<ReportRateIcon size={18} />}>
            <div className="px-4 pt-2 pb-4">
              {reportRate ? (
                <RateEditor key={detail.id} id={detail.id} rate={reportRate} onDetail={onDetail} />
              ) : (
                <p className="text-base text-text-muted">--</p>
              )}
            </div>
          </Panel>
        </>
      ) : !host && flashProfile ? (
        <FlashProfileEditor key={`${detail.id}-${flashProfile.number}`} detail={detail} profile={flashProfile} onDetail={onDetail} />
      ) : !host && detail.onboard ? (
        <>
          <FlashUnavailableNote detail={detail} />
          <ReadOnlyValues detail={detail} />
        </>
      ) : (
        <ReadOnlyValues detail={detail} />
      )}

      {editable ? null : <ReadOnlyReason onOpenSettings={onOpenSettings} />}
    </div>
  );
}

/** Read-only view of the current DPI, stages and report rate. */
function ReadOnlyValues({ detail }: { detail: DeviceDetail }) {
  const t = useT();
  const { dpi, reportRate } = detail;
  const profile = activeUserProfile(detail);
  return (
    <>
      <Panel title={t("perf.dpi")} icon={<DpiIcon size={18} />}>
        <div className="flex flex-col gap-4 px-4 pt-2 pb-4">
          {dpi ? (
            <>
              <div className="flex flex-wrap items-end justify-between gap-x-4 gap-y-1">
                <div>
                  <div className="text-xs text-text-muted">{t("perf.dpiCurrent")}</div>
                  <div className="font-mono text-2xl font-light tabular-nums text-text">{dpi.current}</div>
                </div>
                <div className="font-mono text-sm tabular-nums text-text-muted">
                  {t("perf.dpiRange", { min: dpi.min, max: dpi.max, step: dpi.step })}
                </div>
              </div>
              <DpiMeter min={dpi.min} max={dpi.max} current={dpi.current} />
            </>
          ) : (
            <p className="text-base text-text-muted">--</p>
          )}
          <div className="flex flex-col gap-2">
            <h3 className="text-sm font-medium text-text-muted">{t("perf.stages")}</h3>
            {profile ? (
              <ul className="flex flex-wrap gap-2">
                {profile.dpiLevels.map((level, index) => (
                  <Stage
                    key={index}
                    value={level}
                    isDefault={index === profile.defaultDpiIndex}
                    isShift={index === profile.shiftDpiIndex}
                  />
                ))}
              </ul>
            ) : (
              <p className="text-sm text-text-muted">{t("perf.stagesNone")}</p>
            )}
          </div>
        </div>
      </Panel>

      <Panel title={t("perf.rate")} icon={<ReportRateIcon size={18} />}>
        <div className="px-4 pt-2 pb-4">
          {reportRate ? (
            <ul
              aria-label={t("perf.rate")}
              className="inline-grid auto-cols-fr grid-flow-col rounded-control border border-border bg-surface-2 p-0.5"
            >
              {reportRate.optionsHz.map((hz) => {
                const current = hz === reportRate.currentHz;
                return (
                  <li
                    key={hz}
                    aria-current={current ? "true" : undefined}
                    className={cx(
                      "flex h-7 items-center justify-center rounded-[6px] border px-3 font-mono text-sm tabular-nums whitespace-nowrap",
                      current ? "border-accent/40 bg-accent/15 text-accent" : "border-transparent text-text-faint",
                    )}
                  >
                    {hz} Hz
                  </li>
                );
              })}
            </ul>
          ) : (
            <p className="text-base text-text-muted">--</p>
          )}
        </div>
      </Panel>
    </>
  );
}

/** Why the controls are read-only: writes off in settings, mouse disconnected, or not in host mode. */
function ReadOnlyReason({ onOpenSettings }: { onOpenSettings: () => void }) {
  const t = useT();
  const { settings } = useSettings();
  const writesAllowed = settings?.allowDeviceWrites ?? false;
  const text = !writesAllowed
    ? t("perf.reason.writesDisabled")
    : t("perf.reason.disconnected");

  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
      <p className="flex items-start gap-2 text-sm text-text-muted">
        <Lock size={16} weight="regular" aria-hidden="true" className="mt-px shrink-0" />
        <span className="min-w-0">{text}</span>
      </p>
      {!writesAllowed ? (
        <Button size="sm" onClick={onOpenSettings}>
          {t("perf.reason.enable")}
        </Button>
      ) : null}
    </div>
  );
}

function FeedbackLine({ feedback }: { feedback: Feedback }) {
  // The live regions stay mounted so screen readers announce the text when it appears.
  return (
    <div className="min-h-5 text-sm">
      <p role="status" className={cx("flex items-center gap-1.5 text-accent", feedback?.kind === "ok" ? "" : "hidden")}>
        <CheckCircle size={16} weight="regular" aria-hidden="true" className="shrink-0" />
        <span className="font-mono tabular-nums">{feedback?.kind === "ok" ? feedback.text : ""}</span>
      </p>
      <p role="alert" className={cx("flex items-center gap-1.5 text-danger", feedback?.kind === "error" ? "" : "hidden")}>
        <WarningCircle size={16} weight="regular" aria-hidden="true" className="shrink-0" />
        <span>{feedback?.kind === "error" ? feedback.text : ""}</span>
      </p>
    </div>
  );
}

function DpiEditor({
  id,
  dpi,
  stages,
  defaultIndex,
  profileNumber,
  onDetail,
}: {
  id: string;
  /** Number of the profile whose levels the chips show, or null when unknown. */
  profileNumber: number | null;
  dpi: { min: number; max: number; step: number; current: number };
  stages: number[];
  defaultIndex: number;
  onDetail: (next: DeviceDetail) => void;
}) {
  const t = useT();
  const [text, setText] = useState(String(dpi.current));
  const [pending, setPending] = useState(false);
  const [feedback, setFeedback] = useState<Feedback>(null);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  // Follow the device when its value changes from outside (refresh, restore on connect).
  useEffect(() => {
    setText(String(dpi.current));
  }, [dpi.current]);

  const parsed = /^\d+$/.test(text.trim()) ? Number(text.trim()) : null;
  const valid = parsed !== null && parsed >= dpi.min && parsed <= dpi.max;
  const target = valid ? snapDpi(parsed, dpi.min, dpi.max, dpi.step) : null;
  const canApply = !pending && target !== null && target !== dpi.current;
  const sliderValue = target ?? dpi.current;

  function edit(next: string) {
    setText(next);
    setFeedback(null);
  }

  function snapText() {
    if (target !== null) setText(String(target));
  }

  function apply() {
    if (!canApply || target === null) return;
    setText(String(target));
    setPending(true);
    setFeedback(null);
    setDpi(id, target).then(
      (next) => {
        if (!alive.current) return;
        setPending(false);
        setFeedback({ kind: "ok", text: t("perf.dpiApplied", { n: target }) });
        onDetail(next);
      },
      (error: unknown) => {
        if (!alive.current) return;
        setPending(false);
        const { key, vars } = deviceErrorMessage(error);
        setFeedback({ kind: "error", text: t(key, vars) });
      },
    );
  }

  function onKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key !== "Enter") return;
    event.preventDefault();
    apply();
  }

  return (
    <>
      <p className="text-sm text-text-muted">
        {profileNumber !== null ? t("perf.hostDpiInfo", { n: profileNumber }) : t("perf.hostDpiInfoUnknown")}
      </p>
      <div className="flex flex-wrap items-end justify-between gap-x-4 gap-y-1">
        <div>
          <div className="text-xs text-text-muted">{t("perf.dpiCurrent")}</div>
          <div className="font-mono text-2xl font-light tabular-nums text-text">{dpi.current}</div>
        </div>
        <div className="font-mono text-sm tabular-nums text-text-muted">
          {t("perf.dpiRange", { min: dpi.min, max: dpi.max, step: dpi.step })}
        </div>
      </div>

      {/* The slider only edits the draft. Nothing reaches the mouse until Apply. */}
      <input
        type="range"
        aria-label={t("perf.dpiSlider")}
        min={dpi.min}
        max={dpi.max}
        step={dpi.step}
        value={sliderValue}
        disabled={pending}
        onChange={(event) => edit(event.currentTarget.value)}
        className="h-5 w-full accent-accent disabled:opacity-50"
      />

      <div className="flex flex-wrap items-center gap-3">
        <input
          type="text"
          inputMode="numeric"
          autoComplete="off"
          aria-label={t("perf.dpiInput")}
          aria-invalid={!valid}
          aria-describedby={valid ? undefined : `${id}-dpi-invalid`}
          value={text}
          disabled={pending}
          onChange={(event) => edit(event.currentTarget.value)}
          onBlur={snapText}
          onKeyDown={onKeyDown}
          className={cx(
            "h-9 w-28 rounded-control border bg-surface-2 px-3 text-right font-mono text-base tabular-nums text-text disabled:opacity-50",
            valid ? "border-border" : "border-danger",
          )}
        />
        <Button variant="primary" aria-busy={pending} disabled={!canApply} onClick={apply}>
          {t("perf.apply")}
        </Button>
        <FeedbackLine feedback={feedback} />
      </div>
      {!valid ? (
        <p id={`${id}-dpi-invalid`} className="-mt-2 text-sm text-danger">
          {t("perf.dpiInvalid", { min: dpi.min, max: dpi.max })}
        </p>
      ) : null}

      <h3 className="-mb-2 text-sm font-medium text-text-muted">{t("perf.stages")}</h3>
      {stages.length > 0 ? (
        <ul aria-label={t("perf.stages")} className="flex flex-wrap gap-2">
          {stages.map((level, index) => {
            const selected = target === level;
            return (
              <li key={index}>
                <button
                  type="button"
                  aria-pressed={selected}
                  disabled={pending}
                  onClick={() => edit(String(snapDpi(level, dpi.min, dpi.max, dpi.step)))}
                  className={cx(
                    "pressable h-8 rounded-pill border px-3 font-mono text-sm tabular-nums disabled:opacity-50",
                    selected
                      ? "border-accent/40 bg-accent/15 text-accent"
                      : "border-border bg-surface-2 text-text-muted hover-fine:border-text-faint hover-fine:text-text",
                  )}
                >
                  {level}
                  {index === defaultIndex ? <span className="sr-only">, {t("perf.stageDefault")}</span> : null}
                </button>
              </li>
            );
          })}
        </ul>
      ) : (
        <p className="text-sm text-text-muted">{t("perf.stagesNone")}</p>
      )}
    </>
  );
}

function RateEditor({
  id,
  rate,
  onDetail,
}: {
  id: string;
  rate: { optionsHz: number[]; currentHz: number };
  onDetail: (next: DeviceDetail) => void;
}) {
  const t = useT();
  const [pendingHz, setPendingHz] = useState<number | null>(null);
  const [feedback, setFeedback] = useState<Feedback>(null);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  function choose(hz: number) {
    if (pendingHz !== null || hz === rate.currentHz) return;
    setPendingHz(hz);
    setFeedback(null);
    setReportRate(id, hz).then(
      (next) => {
        if (!alive.current) return;
        setPendingHz(null);
        setFeedback({ kind: "ok", text: t("perf.rateApplied", { n: hz }) });
        onDetail(next);
      },
      (error: unknown) => {
        if (!alive.current) return;
        // The selection falls back to the value the mouse still has.
        setPendingHz(null);
        const { key, vars } = deviceErrorMessage(error);
        setFeedback({ kind: "error", text: t(key, vars) });
      },
    );
  }

  const options = rate.optionsHz.map((hz) => ({ value: String(hz), label: `${hz} Hz` }));
  return (
    <div className="flex flex-wrap items-center gap-3" aria-busy={pendingHz !== null}>
      <div className={cx("font-mono tabular-nums", pendingHz !== null && "opacity-70")}>
        <SegmentedControl
          label={t("perf.rate")}
          value={String(pendingHz ?? rate.currentHz)}
          options={options}
          onChange={(value) => choose(Number(value))}
        />
      </div>
      <FeedbackLine feedback={feedback} />
    </div>
  );
}

/** Non-interactive position of the current DPI inside the supported range. Transform only. */
function DpiMeter({ min, max, current }: { min: number; max: number; current: number }) {
  const ratio = max > min ? Math.min(1, Math.max(0, (current - min) / (max - min))) : 0;
  return (
    <div aria-hidden="true" className="h-1.5 w-full overflow-hidden rounded-pill bg-surface-2">
      <div className="h-full w-full origin-left rounded-pill bg-text-faint" style={{ transform: `scaleX(${ratio})` }} />
    </div>
  );
}

function Stage({ value, isDefault, isShift }: { value: number; isDefault: boolean; isShift: boolean }) {
  const t = useT();
  return (
    <li
      className={cx(
        "flex min-w-[84px] flex-col items-center gap-1 rounded-control border px-3 py-2",
        isDefault ? "border-accent/40 bg-accent/10" : "border-border bg-surface-2",
      )}
    >
      <span className={cx("font-mono text-base font-medium tabular-nums", isDefault ? "text-accent" : "text-text")}>
        {value}
      </span>
      <span className="flex min-h-4 items-center gap-1">
        {isDefault ? <span className="text-xs text-accent">{t("perf.stageDefault")}</span> : null}
        {isShift ? <Badge>{t("perf.stageShift")}</Badge> : null}
      </span>
    </li>
  );
}

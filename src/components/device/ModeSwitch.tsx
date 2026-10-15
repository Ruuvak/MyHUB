import { useEffect, useRef, useState } from "react";
import { useT } from "../../i18n/useT";
import { deviceErrorMessage } from "../../lib/deviceErrors";
import { enterTemporaryMode, leaveTemporaryMode } from "../../lib/ipc";
import type { DeviceDetail } from "../../lib/types";
import { SegmentedControl } from "../../ui";

/**
 * Plain switch between the two equal modes: the profile in the mouse, or control from the app (HOST).
 * Disabled while a switch is pending and when device changes are off. Errors show inline.
 */
export function ModeSwitch({
  detail,
  onDetail,
  showInfo = true,
}: {
  detail: DeviceDetail;
  onDetail: (next: DeviceDetail) => void;
  showInfo?: boolean;
}) {
  const t = useT();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const mode = detail.onboard?.mode;
  if (mode !== "host" && mode !== "onboard") return null;
  const disabled = pending || detail.readOnly;

  function change(next: "host" | "onboard") {
    if (disabled || next === mode) return;
    setPending(true);
    setError(null);
    (next === "host" ? enterTemporaryMode : leaveTemporaryMode)(detail.id).then(
      (fresh) => {
        if (!alive.current) return;
        setPending(false);
        onDetail(fresh);
      },
      (e: unknown) => {
        if (!alive.current) return;
        setPending(false);
        const { key, vars } = deviceErrorMessage(e);
        setError(t(key, vars));
      },
    );
  }

  return (
    <div className="flex flex-col gap-2" aria-busy={pending}>
      <div className={disabled ? "pointer-events-none self-start opacity-60" : "self-start"}>
        <SegmentedControl
          label={t("device.mode.label")}
          value={mode}
          options={[
            { value: "onboard", label: t("device.mode.onboard") },
            { value: "host", label: t("device.mode.host") },
          ]}
          onChange={change}
        />
      </div>
      {showInfo ? <p className="text-sm text-text-muted">{t(mode === "host" ? "mode.infoHost" : "mode.infoOnboard")}</p> : null}
      {error ? (
        <p role="alert" className="text-sm text-danger">
          {error}
        </p>
      ) : null}
    </div>
  );
}

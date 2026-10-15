import { Info, Lock } from "@phosphor-icons/react";
import { useT } from "../../i18n/useT";
import { ReadOnlyBadge } from "../../ui";

/** One-line sentence. The shield badge is for read-only footers, the lock for notes beside controls, the info icon for notes about temporary changes. */
export function ReadOnlyNote({ icon = "check", text }: { icon?: "check" | "lock" | "info"; text?: string }) {
  const t = useT();
  return (
    <p className="flex items-start gap-2 text-sm text-text-muted">
      {icon === "check" ? (
        <ReadOnlyBadge size={18} className="-mt-px" />
      ) : icon === "info" ? (
        <Info size={16} weight="regular" aria-hidden="true" className="mt-px shrink-0" />
      ) : (
        <Lock size={16} weight="regular" aria-hidden="true" className="mt-px shrink-0" />
      )}
      <span className="min-w-0">{text ?? t("note.readOnly")}</span>
    </p>
  );
}
